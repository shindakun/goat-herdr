//! Discord: one POST per alert to a channel, through a webhook URL or a bot
//! token plus channel id. Markdown body with the tail in a code block.

use crate::alert::{Alert, Status};
use crate::config::{Config, DiscordConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

/// Discord's limit on message content.
const MAX_CONTENT: usize = 2000;
/// Longest 429 back-off honored before giving up.
const MAX_RETRY_AFTER_SECS: f64 = 30.0;

enum Auth {
    Webhook {
        url: String,
    },
    Bot {
        token: String,
        channel_id: String,
        api_url: String,
    },
}

pub struct Discord {
    auth: Auth,
    client: Client,
}

impl Discord {
    pub fn new(cfg: &DiscordConfig, config: &Config) -> Result<Self, String> {
        let has_webhook = cfg.webhook_url.is_some() || cfg.webhook_url_env.is_some();
        let has_bot = cfg.bot_token.is_some() || cfg.bot_token_env.is_some();
        let auth = match (has_webhook, has_bot, &cfg.channel_id) {
            (true, false, _) => Auth::Webhook {
                url: config.secret(
                    cfg.webhook_url.as_deref(),
                    cfg.webhook_url_env.as_deref(),
                    "discord webhook url",
                )?,
            },
            (false, true, Some(channel_id)) => Auth::Bot {
                token: config.secret(
                    cfg.bot_token.as_deref(),
                    cfg.bot_token_env.as_deref(),
                    "discord bot token",
                )?,
                channel_id: channel_id.clone(),
                api_url: cfg.api_url.trim_end_matches('/').to_string(),
            },
            (false, true, None) => {
                return Err("discord: bot_token needs channel_id".to_string());
            }
            (true, true, _) => {
                return Err("discord: set a webhook url or a bot token, not both".to_string());
            }
            (false, false, _) => {
                return Err(
                    "discord: set webhook_url(_env) or bot_token(_env) with channel_id".to_string(),
                );
            }
        };
        Ok(Self {
            auth,
            client: Client::new(),
        })
    }

    fn post(&self, body: &serde_json::Value) -> Result<crate::http::Response, String> {
        match &self.auth {
            // wait=true makes Discord answer 200 with the message instead of 204.
            Auth::Webhook { url } => self.client.post_json(&format!("{url}?wait=true"), body),
            Auth::Bot {
                token,
                channel_id,
                api_url,
            } => {
                let url = format!("{api_url}/channels/{channel_id}/messages");
                let text = serde_json::to_string(body).map_err(|err| err.to_string())?;
                self.client.post_text(
                    &url,
                    &[
                        ("Authorization", format!("Bot {token}")),
                        ("Content-Type", "application/json".to_string()),
                    ],
                    &text,
                )
            }
        }
    }
}

impl Sink for Discord {
    fn name(&self) -> &str {
        "discord"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let body = serde_json::json!({ "content": render(alert) });
        let mut response = self.post(&body)?;
        if response.status == 429 {
            let wait = retry_after(&response.body).unwrap_or(1.0);
            if wait > MAX_RETRY_AFTER_SECS {
                return Err(format!(
                    "discord 429: retry_after {wait}s is over the limit"
                ));
            }
            std::thread::sleep(std::time::Duration::from_secs_f64(wait));
            response = self.post(&body)?;
        }
        if (200..300).contains(&response.status) {
            Ok(Delivery::Sent)
        } else {
            let detail = serde_json::from_str::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|v| v["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| response.body.trim().chars().take(200).collect());
            Err(format!("discord {}: {detail}", response.status))
        }
    }
}

/// `{"retry_after": 1.5}` on a 429, in seconds.
fn retry_after(body: &str) -> Option<f64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .get("retry_after")?
        .as_f64()
}

/// Markdown for one alert within Discord's content limit: bold headline,
/// pane line, tail in a code block cut from the top when long.
fn render(alert: &Alert) -> String {
    let head = format!(
        "{} **{}** {}\npane {}",
        alert.status.emoji(),
        alert.status.as_str().to_uppercase(),
        escape(&format!(
            "{} · {} · {}",
            alert.agent, alert.workspace, alert.host
        )),
        escape(&alert.pane_id),
    );
    let Some(tail) = alert.tail.as_deref().filter(|t| !t.trim().is_empty()) else {
        return head;
    };
    let wrapper = "\n```\n\n```";
    let budget = MAX_CONTENT.saturating_sub(head.chars().count() + wrapper.chars().count());
    let mut body = tail.replace("```", "'''");
    if body.chars().count() > budget {
        let marker = "…\n";
        let keep = budget.saturating_sub(marker.chars().count());
        let skip = body.chars().count() - keep;
        let cut: String = body.chars().skip(skip).collect();
        let cut = match cut.find('\n') {
            Some(idx) if idx + 1 < cut.len() => cut[idx + 1..].to_string(),
            _ => cut,
        };
        body = format!("{marker}{cut}");
    }
    format!("{head}\n```\n{body}\n```")
}

/// Markdown characters that would restyle the headline.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '*' | '_' | '`' | '~' | '|' | '>') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::mock::Server;

    fn alert(tail: Option<&str>) -> Alert {
        Alert {
            host: "box".into(),
            workspace: "ws_1".into(),
            agent: "claude".into(),
            pane_id: "w1:p1".into(),
            status: Status::Blocked,
            tail: tail.map(str::to_string),
        }
    }

    #[test]
    fn webhook_posts_content_with_wait() {
        let server = Server::respond(200, "{}");
        let sink = Discord {
            auth: Auth::Webhook {
                url: format!("{}/api/webhooks/1/x", server.url),
            },
            client: Client::new(),
        };
        assert_eq!(sink.send(&alert(Some("run it?"))).unwrap(), Delivery::Sent);
        let req = server.request();
        assert_eq!(req.path, "/api/webhooks/1/x?wait=true");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(
            body["content"],
            "🟥 **BLOCKED** claude · ws\\_1 · box\npane w1:p1\n```\nrun it?\n```"
        );
    }

    #[test]
    fn bot_posts_to_channel_with_auth_header() {
        let server = Server::respond(200, "{}");
        let sink = Discord {
            auth: Auth::Bot {
                token: "tok".into(),
                channel_id: "42".into(),
                api_url: server.url.clone(),
            },
            client: Client::new(),
        };
        assert_eq!(sink.send(&alert(None)).unwrap(), Delivery::Sent);
        let req = server.request();
        assert_eq!(req.path, "/channels/42/messages");
        let header = |k: &str| {
            req.headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("authorization"), Some("Bot tok"));
        assert!(header("content-type")
            .unwrap()
            .starts_with("application/json"));
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert!(body["content"]
            .as_str()
            .unwrap()
            .starts_with("🟥 **BLOCKED**"));
    }

    #[test]
    fn api_error_uses_discord_message() {
        let server = Server::respond(403, r#"{"message":"Missing Access","code":50001}"#);
        let sink = Discord {
            auth: Auth::Webhook {
                url: server.url.clone(),
            },
            client: Client::new(),
        };
        assert_eq!(
            sink.send(&alert(None)).unwrap_err(),
            "discord 403: Missing Access"
        );
        server.request();
    }

    #[test]
    fn long_tail_fits_the_content_limit() {
        let lines: Vec<String> = (0..300).map(|i| format!("line {i:03} ```")).collect();
        let text = render(&alert(Some(&lines.join("\n"))));
        assert!(
            text.chars().count() <= MAX_CONTENT,
            "{}",
            text.chars().count()
        );
        assert!(text.contains("```\n…\nline "));
        assert!(text.ends_with("line 299 '''\n```"));
    }

    #[test]
    fn retry_after_is_seconds() {
        assert_eq!(
            retry_after(r#"{"retry_after": 1.5, "global": false}"#),
            Some(1.5)
        );
        assert_eq!(retry_after("nope"), None);
    }
}
