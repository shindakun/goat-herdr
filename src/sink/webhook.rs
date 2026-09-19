//! Generic webhook: one JSON POST per alert to any URL. The body is the
//! alert itself, so anything that accepts JSON can consume it.

use crate::alert::{Alert, Status};
use crate::config::{Config, WebhookConfig};
use crate::http::Client;
use crate::sink::{Delivery, Sink};

pub struct Webhook {
    url: String,
    client: Client,
}

impl Webhook {
    pub fn new(cfg: &WebhookConfig, config: &Config) -> Result<Self, String> {
        let url = config.secret(cfg.url.as_deref(), cfg.url_env.as_deref(), "webhook url")?;
        Ok(Self {
            url,
            client: Client::new(),
        })
    }
}

impl Sink for Webhook {
    fn name(&self) -> &str {
        "webhook"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let response = self.client.post_json(&self.url, &payload(alert))?;
        if (200..300).contains(&response.status) {
            Ok(Delivery::Sent)
        } else {
            Err(format!(
                "webhook {}: {}",
                response.status,
                response.body.trim().chars().take(200).collect::<String>()
            ))
        }
    }
}

/// The wire format. `text` is the same plain rendering the stdout and
/// ntfy sinks use; the other fields are there for receivers that route.
fn payload(alert: &Alert) -> serde_json::Value {
    serde_json::json!({
        "source": "goat-herdr",
        "status": alert.status.as_str(),
        "host": alert.host,
        "workspace": alert.workspace,
        "agent": alert.agent,
        "pane_id": alert.pane_id,
        "headline": alert.headline(),
        "tail": alert.tail,
        "text": super::plain_text(alert),
    })
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
    fn posts_alert_as_json() {
        let server = Server::respond(200, "");
        let sink = Webhook {
            url: format!("{}/hook", server.url),
            client: Client::new(),
        };
        assert_eq!(sink.send(&alert()).unwrap(), Delivery::Sent);
        let req = server.request();
        assert_eq!(req.path, "/hook");
        let ct = req
            .headers
            .iter()
            .find(|(k, _)| k == "content-type")
            .map(|(_, v)| v.as_str());
        assert!(ct.unwrap().starts_with("application/json"), "{ct:?}");
        let body: serde_json::Value = serde_json::from_str(&req.body).unwrap();
        assert_eq!(body["source"], "goat-herdr");
        assert_eq!(body["status"], "blocked");
        assert_eq!(body["pane_id"], "w1:p1");
        assert_eq!(body["headline"], "BLOCKED claude · ws · box");
        assert_eq!(body["tail"], "Proceed? (y/n)");
        assert!(body["text"]
            .as_str()
            .unwrap()
            .starts_with("🟥 BLOCKED claude"));
    }

    #[test]
    fn non_2xx_is_an_error() {
        let server = Server::respond(500, "boom");
        let sink = Webhook {
            url: server.url.clone(),
            client: Client::new(),
        };
        assert_eq!(sink.send(&alert()).unwrap_err(), "webhook 500: boom");
        server.request();
    }
}
