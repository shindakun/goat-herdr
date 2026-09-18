//! ntfy: one POST per alert. The URL is the topic. This is the reference for
//! every webhook-style sink.

use crate::alert::{Alert, Status};
use crate::config::{Config, NtfyConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

pub struct Ntfy {
    url: String,
    token: Option<String>,
    client: Client,
}

impl Ntfy {
    pub fn new(cfg: &NtfyConfig, config: &Config) -> Result<Self, String> {
        let token = match (&cfg.token, &cfg.token_env) {
            (None, None) => None,
            (inline, env) => {
                Some(config.secret(inline.as_deref(), env.as_deref(), "ntfy token")?)
            }
        };
        Ok(Self {
            url: cfg.url.clone(),
            token,
            client: Client::new(),
        })
    }
}

impl Sink for Ntfy {
    fn name(&self) -> &str {
        "ntfy"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let priority = match alert.status {
            Status::Blocked => "urgent",
            Status::Done => "default",
            _ => "low",
        };
        // ntfy headers are Latin-1; the headline may not be, so it goes in the body.
        let mut headers = vec![
            (
                "Title",
                format!("{} {} needs you", alert.status.as_str(), alert.agent),
            ),
            ("Priority", priority.to_string()),
            ("Tags", tag(alert.status).to_string()),
        ];
        if let Some(token) = &self.token {
            headers.push(("Authorization", format!("Bearer {token}")));
        }
        let body = super::plain_text(alert);
        let response = self.client.post_text(&self.url, &headers, &body)?;
        if (200..300).contains(&response.status) {
            Ok(Delivery::Sent)
        } else {
            Err(format!(
                "ntfy {}: {}",
                response.status,
                response.body.trim()
            ))
        }
    }
}

fn tag(status: Status) -> &'static str {
    match status {
        Status::Blocked => "red_square",
        Status::Done => "green_square",
        Status::Working => "blue_square",
        Status::Idle => "white_square",
        Status::Unknown | Status::Closed => "black_square",
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

    #[test]
    fn posts_body_and_headers() {
        let server = Server::respond(200, "{}");
        let sink = Ntfy {
            url: format!("{}/goats", server.url),
            token: Some("tok".into()),
            client: Client::new(),
        };
        sink.send(&alert()).unwrap();
        let req = server.request();
        assert_eq!(req.path, "/goats");
        let header = |k: &str| {
            req.headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("title"), Some("blocked claude needs you"));
        assert_eq!(header("priority"), Some("urgent"));
        assert_eq!(header("tags"), Some("red_square"));
        assert_eq!(header("authorization"), Some("Bearer tok"));
        assert!(req
            .body
            .starts_with("🟥 BLOCKED claude · ws · box\npane w1:p1\n---\nProceed?"));
    }

    #[test]
    fn non_2xx_is_an_error() {
        let server = Server::respond(403, "forbidden");
        let sink = Ntfy {
            url: server.url.clone(),
            token: None,
            client: Client::new(),
        };
        let err = sink.send(&alert()).unwrap_err();
        assert!(err.contains("403"), "{err}");
        server.request();
    }
}
