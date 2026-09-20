//! Pushover: one message per alert. Needs an application token and the
//! user key. Blocked alerts go out at high priority.

use crate::alert::{Alert, Status};
use crate::config::{Config, PushoverConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

/// Pushover's limits on message and title.
const MAX_MESSAGE: usize = 1024;
const MAX_TITLE: usize = 250;

pub struct Pushover {
    app_token: String,
    user_key: String,
    api_url: String,
    client: Client,
}

impl Pushover {
    pub fn new(cfg: &PushoverConfig, config: &Config) -> Result<Self, String> {
        let app_token = config.secret(
            cfg.app_token.as_deref(),
            cfg.app_token_env.as_deref(),
            "pushover app token",
        )?;
        let user_key = config.secret(
            cfg.user_key.as_deref(),
            cfg.user_key_env.as_deref(),
            "pushover user key",
        )?;
        Ok(Self {
            app_token,
            user_key,
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            client: Client::new(),
        })
    }
}

impl Sink for Pushover {
    fn name(&self) -> &str {
        "pushover"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let priority = match alert.status {
            Status::Blocked => 1,
            Status::Done => 0,
            _ => -1,
        };
        let title: String = alert.headline().chars().take(MAX_TITLE).collect();
        let message = cut_from_top(&super::plain_text(alert), MAX_MESSAGE);
        let body = serde_json::json!({
            "token": self.app_token,
            "user": self.user_key,
            "title": title,
            "message": message,
            "priority": priority,
            "monospace": 1,
        });
        let response = self
            .client
            .post_json(&format!("{}/1/messages.json", self.api_url), &body)?;
        let parsed: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        if (200..300).contains(&response.status) && parsed["status"] == 1 {
            Ok(Delivery::Sent)
        } else {
            let detail = parsed["errors"]
                .as_array()
                .map(|errs| {
                    errs.iter()
                        .filter_map(|e| e.as_str())
                        .collect::<Vec<_>>()
                        .join("; ")
                })
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| response.body.trim().chars().take(200).collect());
            Err(format!("pushover {}: {detail}", response.status))
        }
    }
}

/// Keeps the last `max` chars, starting on a whole line when one is inside.
fn cut_from_top(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_string();
    }
    let marker = "…\n";
    let keep = max.saturating_sub(marker.chars().count());
    let cut: String = text.chars().skip(count - keep).collect();
    match cut.find('\n') {
        Some(idx) if idx + 1 < cut.len() => format!("{marker}{}", &cut[idx + 1..]),
        _ => format!("{marker}{cut}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::mock::Server;

    fn alert() -> Alert {
        Alert {
            host: "box".into(),
            workspace: "ws".into(),
            agent: "claude".into(),
            pane_id: "w1:p1".into(),
            status: Status::Blocked,
            tail: Some("Proceed? (y/n)".into()),
        }
    }

    fn sink(server: &Server) -> Pushover {
        Pushover {
            app_token: "app".into(),
            user_key: "user".into(),
            api_url: server.url.clone(),
            client: Client::new(),
        }
    }

    #[test]
    fn posts_message_with_priority() {
        let server = Server::respond(200, r#"{"status":1,"request":"abc"}"#);
        assert_eq!(sink(&server).send(&alert()).unwrap(), Delivery::Sent);
        let req = server.request();
        assert_eq!(req.path, "/1/messages.json");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["token"], "app");
        assert_eq!(body["user"], "user");
        assert_eq!(body["priority"], 1);
        assert_eq!(body["monospace"], 1);
        assert_eq!(body["title"], "BLOCKED claude · ws · box");
        assert!(body["message"].as_str().unwrap().contains("Proceed? (y/n)"));
    }

    #[test]
    fn errors_list_pushover_reasons() {
        let server = Server::respond(
            400,
            r#"{"status":0,"errors":["application token is invalid"]}"#,
        );
        assert_eq!(
            sink(&server).send(&alert()).unwrap_err(),
            "pushover 400: application token is invalid"
        );
        server.request();
    }

    #[test]
    fn status_zero_with_200_is_an_error() {
        let server = Server::respond(200, r#"{"status":0,"errors":["user key is invalid"]}"#);
        assert!(sink(&server)
            .send(&alert())
            .unwrap_err()
            .contains("user key is invalid"));
        server.request();
    }

    #[test]
    fn long_message_is_cut_from_top() {
        let lines: Vec<String> = (0..200).map(|i| format!("line {i:03}")).collect();
        let cut = cut_from_top(&lines.join("\n"), 100);
        assert!(cut.chars().count() <= 100);
        assert!(cut.starts_with("…\nline "));
        assert!(cut.ends_with("line 199"));
    }
}
