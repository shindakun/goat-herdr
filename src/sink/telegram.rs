//! Telegram Bot API `sendMessage`. HTML parse mode; agent output goes in a
//! `<pre>` block. Forum topics come in the next milestone.

use crate::alert::{Alert, Status};
use crate::config::{Config, TelegramConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

/// Telegram's hard limit on message text.
const MAX_TEXT: usize = 4096;
/// Longest 429 back-off honored before giving up.
const MAX_RETRY_AFTER_SECS: u64 = 30;

pub struct Telegram {
    api_url: String,
    token: String,
    chat_id: i64,
    client: Client,
}

impl Telegram {
    pub fn new(cfg: &TelegramConfig, config: &Config) -> Result<Self, String> {
        let token = config.secret(
            cfg.bot_token.as_deref(),
            cfg.bot_token_env.as_deref(),
            "telegram bot token",
        )?;
        Ok(Self {
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            token,
            chat_id: cfg.chat_id,
            client: Client::new(),
        })
    }

    fn call(&self, method: &str, params: &serde_json::Value) -> Result<(), String> {
        let url = format!("{}/bot{}/{method}", self.api_url, self.token);
        let response = self.client.post_json(&url, params)?;
        if response.status == 429 {
            let wait = retry_after(&response.body).unwrap_or(1);
            if wait > MAX_RETRY_AFTER_SECS {
                return Err(format!(
                    "telegram 429: retry_after {wait}s is over the limit"
                ));
            }
            std::thread::sleep(std::time::Duration::from_secs(wait));
            let response = self.client.post_json(&url, params)?;
            return check(&response.status, &response.body, method);
        }
        check(&response.status, &response.body, method)
    }
}

fn check(status: &u16, body: &str, method: &str) -> Result<(), String> {
    if (200..300).contains(status) {
        Ok(())
    } else {
        let description = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| v["description"].as_str().map(str::to_string))
            .unwrap_or_else(|| body.trim().to_string());
        Err(format!("telegram {method} {status}: {description}"))
    }
}

fn retry_after(body: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .pointer("/parameters/retry_after")?
        .as_u64()
}

impl Sink for Telegram {
    fn name(&self) -> &str {
        "telegram"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let params = serde_json::json!({
            "chat_id": self.chat_id,
            "text": render(alert),
            "parse_mode": "HTML",
            "disable_web_page_preview": true,
        });
        self.call("sendMessage", &params)?;
        Ok(Delivery::Sent)
    }
}

/// HTML for one alert, within Telegram's length limit. When the tail does
/// not fit, its oldest lines go first.
fn render(alert: &Alert) -> String {
    let head = format!(
        "{} <b>{}</b> {} · {} · {}\npane {}",
        alert.status.emoji(),
        alert.status.as_str().to_uppercase(),
        escape(&alert.agent),
        escape(&alert.workspace),
        escape(&alert.host),
        escape(&alert.pane_id),
    );
    let Some(tail) = alert.tail.as_deref().filter(|t| !t.trim().is_empty()) else {
        return head;
    };
    let wrapper = "\n<pre>\n</pre>";
    let budget = MAX_TEXT.saturating_sub(head.chars().count() + wrapper.chars().count());
    let mut body = escape(tail);
    if body.chars().count() > budget {
        let marker = "…\n";
        let keep = budget.saturating_sub(marker.chars().count());
        let skip = body.chars().count() - keep;
        let cut: String = body.chars().skip(skip).collect();
        // Start on a whole line when one is available inside the kept range.
        let cut = match cut.find('\n') {
            Some(idx) if idx + 1 < cut.len() => cut[idx + 1..].to_string(),
            _ => cut,
        };
        body = format!("{marker}{cut}");
    }
    format!("{head}\n<pre>{body}</pre>")
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

    fn sink(server: &Server) -> Telegram {
        Telegram {
            api_url: server.url.clone(),
            token: "123:abc".into(),
            chat_id: -100,
            client: Client::new(),
        }
    }

    #[test]
    fn sends_html_message_to_chat() {
        let server = Server::respond(200, r#"{"ok":true,"result":{}}"#);
        sink(&server).send(&alert(Some("run <cmd>?"))).unwrap();
        let req = server.request();
        assert_eq!(req.path, "/bot123:abc/sendMessage");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["chat_id"], -100);
        assert_eq!(body["parse_mode"], "HTML");
        assert_eq!(
            body["text"],
            "🟥 <b>BLOCKED</b> claude · ws &lt;1&gt; · box\npane w1:p1\n<pre>run &lt;cmd&gt;?</pre>"
        );
    }

    #[test]
    fn api_error_surfaces_description() {
        let server = Server::respond(
            400,
            r#"{"ok":false,"description":"Bad Request: chat not found"}"#,
        );
        let err = sink(&server).send(&alert(None)).unwrap_err();
        assert_eq!(err, "telegram sendMessage 400: Bad Request: chat not found");
        server.request();
    }

    #[test]
    fn closed_alerts_are_skipped() {
        let mut a = alert(None);
        a.status = Status::Closed;
        let sink = Telegram {
            api_url: "http://127.0.0.1:1".into(),
            token: "t".into(),
            chat_id: 1,
            client: Client::new(),
        };
        assert_eq!(sink.send(&a).unwrap(), Delivery::Skipped);
    }

    #[test]
    fn long_tail_is_cut_from_the_top() {
        let lines: Vec<String> = (0..500)
            .map(|i| format!("line {i:04} {}", "x".repeat(20)))
            .collect();
        let text = render(&alert(Some(&lines.join("\n"))));
        assert!(text.chars().count() <= MAX_TEXT, "{}", text.chars().count());
        assert!(text.contains("<pre>…\nline "));
        assert!(text.ends_with("line 0499 xxxxxxxxxxxxxxxxxxxx</pre>"));
        assert!(!text.contains("line 0000"));
    }

    #[test]
    fn retry_after_is_parsed() {
        assert_eq!(
            retry_after(r#"{"ok":false,"error_code":429,"parameters":{"retry_after":7}}"#),
            Some(7)
        );
        assert_eq!(retry_after("nope"), None);
    }
}
