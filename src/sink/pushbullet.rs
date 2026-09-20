//! Pushbullet: one note push per alert to every device on the account.

use crate::alert::{Alert, Status};
use crate::config::{Config, PushbulletConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

pub struct Pushbullet {
    token: String,
    api_url: String,
    client: Client,
}

impl Pushbullet {
    pub fn new(cfg: &PushbulletConfig, config: &Config) -> Result<Self, String> {
        let token = config.secret(
            cfg.token.as_deref(),
            cfg.token_env.as_deref(),
            "pushbullet token",
        )?;
        Ok(Self {
            token,
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            client: Client::new(),
        })
    }
}

impl Sink for Pushbullet {
    fn name(&self) -> &str {
        "pushbullet"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let body = serde_json::json!({
            "type": "note",
            "title": alert.headline(),
            "body": super::plain_text(alert),
        });
        let text = serde_json::to_string(&body).map_err(|err| err.to_string())?;
        let response = self.client.post_text(
            &format!("{}/v2/pushes", self.api_url),
            &[
                ("Access-Token", self.token.clone()),
                ("Content-Type", "application/json".to_string()),
            ],
            &text,
        )?;
        if (200..300).contains(&response.status) {
            Ok(Delivery::Sent)
        } else {
            let detail = serde_json::from_str::<serde_json::Value>(&response.body)
                .ok()
                .and_then(|v| v.pointer("/error/message")?.as_str().map(str::to_string))
                .unwrap_or_else(|| response.body.trim().chars().take(200).collect());
            Err(format!("pushbullet {}: {detail}", response.status))
        }
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
    fn pushes_a_note() {
        let server = Server::respond(200, "{}");
        let sink = Pushbullet {
            token: "tok".into(),
            api_url: server.url.clone(),
            client: Client::new(),
        };
        assert_eq!(sink.send(&alert()).unwrap(), Delivery::Sent);
        let req = server.request();
        assert_eq!(req.path, "/v2/pushes");
        let header = |k: &str| {
            req.headers
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(header("access-token"), Some("tok"));
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["type"], "note");
        assert_eq!(body["title"], "BLOCKED claude · ws · box");
        assert!(body["body"].as_str().unwrap().contains("Proceed? (y/n)"));
    }

    #[test]
    fn api_error_uses_message() {
        let server = Server::respond(
            401,
            r#"{"error":{"message":"Access token is missing or invalid."}}"#,
        );
        let sink = Pushbullet {
            token: "bad".into(),
            api_url: server.url.clone(),
            client: Client::new(),
        };
        assert_eq!(
            sink.send(&alert()).unwrap_err(),
            "pushbullet 401: Access token is missing or invalid."
        );
        server.request();
    }
}
