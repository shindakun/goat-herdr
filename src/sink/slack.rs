//! Slack incoming webhook: one POST per alert. The webhook URL is the
//! credential and sits in the path, so errors never include it.

use crate::alert::{Alert, Status};
use crate::config::{Config, SlackConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

/// Tail budget. Slack accepts far more, but a phone screen does not.
const MAX_TAIL_CHARS: usize = 3000;

pub struct Slack {
    webhook_url: String,
    client: Client,
}

impl Slack {
    pub fn new(cfg: &SlackConfig, config: &Config) -> Result<Self, String> {
        let webhook_url = config.secret(
            cfg.webhook_url.as_deref(),
            cfg.webhook_url_env.as_deref(),
            "slack webhook url",
        )?;
        Ok(Self {
            webhook_url,
            client: Client::new(),
        })
    }
}

impl Sink for Slack {
    fn name(&self) -> &str {
        "slack"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let body = serde_json::json!({ "text": render(alert) });
        let mut response = self.client.post_json(&self.webhook_url, &body)?;
        if response.status == 429 {
            // Slack allows about one post per second per webhook.
            std::thread::sleep(std::time::Duration::from_secs(1));
            response = self.client.post_json(&self.webhook_url, &body)?;
        }
        if (200..300).contains(&response.status) {
            Ok(Delivery::Sent)
        } else {
            Err(format!(
                "slack {}: {}",
                response.status,
                response.body.trim()
            ))
        }
    }
}

/// mrkdwn for one alert: bold headline, pane line, tail in a code block
/// cut from the top when long.
fn render(alert: &Alert) -> String {
    let mut text = format!(
        "{} *{}* {}\npane {}",
        alert.status.emoji(),
        alert.status.as_str().to_uppercase(),
        escape(&format!(
            "{} · {} · {}",
            alert.agent, alert.workspace, alert.host
        )),
        escape(&alert.pane_id),
    );
    if let Some(tail) = alert.tail.as_deref().filter(|t| !t.trim().is_empty()) {
        let mut tail = escape(tail).replace("```", "'''");
        if tail.chars().count() > MAX_TAIL_CHARS {
            let skip = tail.chars().count() - MAX_TAIL_CHARS;
            let cut: String = tail.chars().skip(skip).collect();
            tail = match cut.find('\n') {
                Some(idx) if idx + 1 < cut.len() => format!("…\n{}", &cut[idx + 1..]),
                _ => format!("…\n{cut}"),
            };
        }
        text.push_str("\n```\n");
        text.push_str(&tail);
        text.push_str("\n```");
    }
    text
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::mock::Server;

    fn alert(tail: Option<&str>) -> Alert {
        Alert {
            host: "box".into(),
            workspace: "ws <1>".into(),
            agent: "claude".into(),
            pane_id: "w1:p1".into(),
            status: Status::Blocked,
            tail: tail.map(str::to_string),
        }
    }

    #[test]
    fn posts_mrkdwn_text() {
        let server = Server::respond(200, "ok");
        let sink = Slack {
            webhook_url: format!("{}/services/T/B/X", server.url),
            client: Client::new(),
        };
        assert_eq!(
            sink.send(&alert(Some("run <cmd>?"))).unwrap(),
            Delivery::Sent
        );
        let req = server.request();
        assert_eq!(req.path, "/services/T/B/X");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(
            body["text"],
            "🟥 *BLOCKED* claude · ws &lt;1&gt; · box\npane w1:p1\n```\nrun &lt;cmd&gt;?\n```"
        );
    }

    #[test]
    fn error_names_status_and_body_only() {
        let server = Server::respond(403, "invalid_token");
        let sink = Slack {
            webhook_url: format!("{}/services/SECRET", server.url),
            client: Client::new(),
        };
        let err = sink.send(&alert(None)).unwrap_err();
        assert_eq!(err, "slack 403: invalid_token");
        server.request();
    }

    #[test]
    fn long_tail_is_cut_and_fences_are_neutralized() {
        let lines: Vec<String> = (0..300).map(|i| format!("line {i:03} ```")).collect();
        let text = render(&alert(Some(&lines.join("\n"))));
        assert!(text.contains("```\n…\nline "));
        assert!(!text.contains("line 000"));
        assert!(text.ends_with("line 299 '''\n```"));
    }

    #[test]
    fn closed_is_skipped() {
        let mut a = alert(None);
        a.status = Status::Closed;
        let sink = Slack {
            webhook_url: "http://127.0.0.1:1/x".into(),
            client: Client::new(),
        };
        assert_eq!(sink.send(&a).unwrap(), Delivery::Skipped);
    }
}
