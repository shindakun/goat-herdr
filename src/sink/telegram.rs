//! Telegram Bot API. `sendMessage` in HTML parse mode with agent output in a
//! `<pre>` block. With topics on, each alert goes to a forum topic named for
//! its agent, workspace, and host; topics are created on first use, closed
//! when the last pane behind them closes, and reopened when one returns.

use crate::alert::{Alert, Status};
use crate::bridge::{Bridge, Inbound};
use crate::config::{Config, TelegramConfig, TopicMode};
use crate::http::Client;
use crate::sink::{Delivery, Sink};
use crate::state::State;

/// Telegram's hard limit on message text.
const MAX_TEXT: usize = 4096;
/// Telegram's limit on a forum topic name.
const MAX_TOPIC_NAME: usize = 128;
/// Longest 429 back-off honored before giving up.
const MAX_RETRY_AFTER_SECS: u64 = 30;

pub struct Telegram {
    api_url: String,
    token: String,
    chat_id: i64,
    topics: TopicMode,
    bridge: bool,
    allowed_user_ids: Vec<i64>,
    state: State,
    client: Client,
    /// Separate client for getUpdates: the long poll outlives the default timeout.
    poll_client: Client,
}

/// Seconds Telegram holds a getUpdates call open when nothing arrives.
const POLL_TIMEOUT_SECS: u64 = 25;

/// An API call that failed, with the error text Telegram returned so the
/// caller can react to topic state.
struct ApiError {
    method: String,
    status: u16,
    description: String,
}

impl ApiError {
    fn topic_closed(&self) -> bool {
        self.description.contains("TOPIC_CLOSED")
    }

    fn thread_missing(&self) -> bool {
        // "Bad Request: message thread not found" after a topic is deleted.
        self.description.contains("thread not found")
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "telegram {} {}: {}",
            self.method, self.status, self.description
        )
    }
}

impl Telegram {
    pub fn new(cfg: &TelegramConfig, config: &Config, state: State) -> Result<Self, String> {
        let token = config.secret(
            cfg.bot_token.as_deref(),
            cfg.bot_token_env.as_deref(),
            "telegram bot token",
        )?;
        Ok(Self {
            api_url: cfg.api_url.trim_end_matches('/').to_string(),
            token,
            chat_id: cfg.chat_id,
            topics: cfg.topics,
            bridge: cfg.bridge,
            allowed_user_ids: cfg.allowed_user_ids.clone(),
            state,
            client: Client::new(),
            poll_client: Client::with_timeout(std::time::Duration::from_secs(
                POLL_TIMEOUT_SECS + 15,
            )),
        })
    }

    /// One Bot API call. Retries once on 429. Returns the `result` value.
    fn call(
        &self,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        self.call_with(&self.client, method, params)
    }

    fn call_with(
        &self,
        client: &Client,
        method: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        let url = format!("{}/bot{}/{method}", self.api_url, self.token);
        // The URL carries the token; no error text may echo it.
        let transport = |err: String| ApiError {
            method: method.to_string(),
            status: 0,
            description: err.replace(&self.token, "<token>"),
        };
        let mut response = client.post_json(&url, params).map_err(transport)?;
        if response.status == 429 {
            let wait = retry_after(&response.body).unwrap_or(1);
            if wait > MAX_RETRY_AFTER_SECS {
                return Err(ApiError {
                    method: method.to_string(),
                    status: 429,
                    description: format!("retry_after {wait}s is over the limit"),
                });
            }
            std::thread::sleep(std::time::Duration::from_secs(wait));
            response = client.post_json(&url, params).map_err(transport)?;
        }
        let body: serde_json::Value = serde_json::from_str(&response.body).unwrap_or_default();
        if (200..300).contains(&response.status) && body["ok"] == true {
            return Ok(body["result"].clone());
        }
        Err(ApiError {
            method: method.to_string(),
            status: response.status,
            description: body["description"]
                .as_str()
                .map(str::to_string)
                .unwrap_or_else(|| response.body.trim().to_string()),
        })
    }

    fn topic_key(&self, alert: &Alert) -> Option<String> {
        match self.topics {
            TopicMode::None => None,
            TopicMode::PerAgent => Some(format!(
                "{}|{}|{}",
                alert.host,
                alert.workspace_name(),
                alert.agent
            )),
            TopicMode::PerWorkspace => Some(format!("{}|{}", alert.host, alert.workspace_name())),
        }
    }

    fn topic_name(&self, alert: &Alert) -> String {
        let name = match self.topics {
            TopicMode::PerWorkspace => format!("{} · {}", alert.workspace_name(), alert.host),
            _ => format!(
                "{} · {} · {}",
                alert.agent,
                alert.workspace_name(),
                alert.host
            ),
        };
        name.chars().take(MAX_TOPIC_NAME).collect()
    }

    fn create_topic(&self, name: &str) -> Result<i64, String> {
        let result = self
            .call(
                "createForumTopic",
                &serde_json::json!({ "chat_id": self.chat_id, "name": name }),
            )
            .map_err(|err| err.to_string())?;
        result["message_thread_id"]
            .as_i64()
            .ok_or_else(|| "telegram createForumTopic: no message_thread_id in result".to_string())
    }

    /// The thread to post into, reopened first when this plugin closed it.
    fn thread_for(&self, key: &str, alert: &Alert) -> Result<i64, String> {
        let handle = self.state.topic_thread(key, &alert.pane_id, || {
            self.create_topic(&self.topic_name(alert))
        })?;
        if handle.closed {
            self.topic_call("reopenForumTopic", handle.thread_id)?;
            self.state.topic_set_closed(key, false)?;
        }
        Ok(handle.thread_id)
    }

    /// closeForumTopic or reopenForumTopic. Already in that state is success.
    fn topic_call(&self, method: &str, thread_id: i64) -> Result<(), String> {
        match self.call(
            method,
            &serde_json::json!({ "chat_id": self.chat_id, "message_thread_id": thread_id }),
        ) {
            Ok(_) => Ok(()),
            Err(err) if err.description.contains("TOPIC_NOT_MODIFIED") => Ok(()),
            Err(err) => Err(err.to_string()),
        }
    }

    fn send_message(
        &self,
        text: &str,
        thread_id: Option<i64>,
        keyboard: Option<serde_json::Value>,
    ) -> Result<(), ApiError> {
        let mut params = serde_json::json!({
            "chat_id": self.chat_id,
            "text": text,
            "parse_mode": "HTML",
            "link_preview_options": { "is_disabled": true },
        });
        if let Some(id) = thread_id {
            params["message_thread_id"] = id.into();
        }
        if let Some(keyboard) = keyboard {
            params["reply_markup"] = keyboard;
        }
        self.call("sendMessage", &params).map(|_| ())
    }

    /// Buttons under a blocked alert. Callback data is read by the bridge:
    /// `k|<pane>|<key>...` sends keys, `t|<pane>` posts the tail.
    fn keyboard(&self, alert: &Alert) -> Option<serde_json::Value> {
        if !self.bridge || alert.status != Status::Blocked {
            return None;
        }
        let pane = &alert.pane_id;
        let button =
            |label: &str, data: String| serde_json::json!({ "text": label, "callback_data": data });
        Some(serde_json::json!({ "inline_keyboard": [
            [
                button("y", format!("k|{pane}|y|Enter")),
                button("n", format!("k|{pane}|n|Enter")),
                button("Enter", format!("k|{pane}|Enter")),
                button("Esc", format!("k|{pane}|esc")),
            ],
            [button("Tail", format!("t|{pane}"))],
        ] }))
    }

    fn release_topic(&self, alert: &Alert) -> Result<(), String> {
        let Some(release) = self.state.topic_pane_closed(&alert.pane_id)? else {
            return Ok(());
        };
        if release.last_pane {
            // A topic that is gone is not worth failing the hook over.
            match self.topic_call("closeForumTopic", release.thread_id) {
                Ok(()) => self.state.topic_set_closed(&release.key, true)?,
                Err(err) => eprintln!("goat-herdr: {err}"),
            }
        }
        Ok(())
    }
}

impl Sink for Telegram {
    fn name(&self) -> &str {
        "telegram"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            if self.topics != TopicMode::None {
                self.release_topic(alert)?;
            }
            return Ok(Delivery::Skipped);
        }
        let text = render(alert);
        let keyboard = self.keyboard(alert);
        let Some(key) = self.topic_key(alert) else {
            return self
                .send_message(&text, None, keyboard)
                .map(|()| Delivery::Sent)
                .map_err(|err| err.to_string());
        };
        let thread_id = self.thread_for(&key, alert)?;
        match self.send_message(&text, Some(thread_id), keyboard.clone()) {
            Ok(()) => Ok(Delivery::Sent),
            Err(err) if err.topic_closed() => {
                self.topic_call("reopenForumTopic", thread_id)?;
                self.send_message(&text, Some(thread_id), keyboard)
                    .map(|()| Delivery::Sent)
                    .map_err(|err| err.to_string())
            }
            Err(err) if err.thread_missing() => {
                self.state.topic_forget(&key)?;
                let thread_id = self.thread_for(&key, alert)?;
                self.send_message(&text, Some(thread_id), keyboard)
                    .map(|()| Delivery::Sent)
                    .map_err(|err| err.to_string())
            }
            Err(err) => Err(err.to_string()),
        }
    }
}

impl Bridge for Telegram {
    fn name(&self) -> &str {
        "telegram"
    }

    /// One `getUpdates` long poll. The offset lives in state so a restart
    /// does not replay old input.
    fn poll(&self) -> Result<Vec<Inbound>, String> {
        let mut params = serde_json::json!({
            "timeout": POLL_TIMEOUT_SECS,
            "allowed_updates": ["message", "callback_query"],
        });
        if let Some(offset) = self.state.bridge_cursor("telegram") {
            params["offset"] = offset.into();
        }
        let result = self
            .call_with(&self.poll_client, "getUpdates", &params)
            .map_err(|err| err.to_string())?;
        let updates = result.as_array().cloned().unwrap_or_default();
        let mut inbounds = Vec::new();
        let mut last_id = None;
        for update in &updates {
            last_id = update["update_id"].as_i64().or(last_id);
            if let Some(inbound) = self.inbound_from(update) {
                inbounds.push(inbound);
            }
        }
        if let Some(id) = last_id {
            self.state.set_bridge_cursor("telegram", id + 1)?;
        }
        Ok(inbounds)
    }

    fn reply(&self, inbound: &Inbound, text: &str, pre: bool) -> Result<(), String> {
        let text = if pre {
            format!("<pre>{}</pre>", escape(text))
        } else {
            escape(text)
        };
        let text: String = text.chars().take(MAX_TEXT).collect();
        self.send_message(&text, inbound.thread_id, None)
            .map_err(|err| err.to_string())
    }

    fn ack(&self, inbound: &Inbound, text: &str) -> Result<(), String> {
        let Some(id) = &inbound.callback_id else {
            return Ok(());
        };
        self.call(
            "answerCallbackQuery",
            &serde_json::json!({ "callback_query_id": id, "text": text }),
        )
        .map(|_| ())
        .map_err(|err| err.to_string())
    }

    fn allowed_user(&self, user: i64) -> bool {
        self.allowed_user_ids.contains(&user)
    }
}

impl Telegram {
    /// Messages and button presses in the configured chat. Everything else
    /// (other chats, joins, edits) is dropped.
    fn inbound_from(&self, update: &serde_json::Value) -> Option<Inbound> {
        if let Some(cb) = update.get("callback_query") {
            let message = cb.get("message")?;
            if message["chat"]["id"].as_i64()? != self.chat_id {
                return None;
            }
            return Some(Inbound {
                thread_id: message["message_thread_id"].as_i64(),
                from_user: cb["from"]["id"].as_i64()?,
                text: cb["data"].as_str()?.to_string(),
                callback_id: Some(cb["id"].as_str()?.to_string()),
            });
        }
        let message = update.get("message")?;
        if message["chat"]["id"].as_i64()? != self.chat_id {
            return None;
        }
        Some(Inbound {
            thread_id: message["message_thread_id"].as_i64(),
            from_user: message["from"]["id"].as_i64()?,
            text: message["text"].as_str()?.to_string(),
            callback_id: None,
        })
    }
}

fn retry_after(body: &str) -> Option<u64> {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()?
        .pointer("/parameters/retry_after")?
        .as_u64()
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

    const OK: &str = r#"{"ok":true,"result":{}}"#;

    fn alert(tail: Option<&str>) -> Alert {
        Alert {
            host: "box".into(),
            workspace: "[1] ws <1>".into(),
            agent: "claude".into(),
            pane_id: "w1:p1".into(),
            status: Status::Blocked,
            tail: tail.map(str::to_string),
        }
    }

    fn temp_state(name: &str) -> (State, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "goat-herdr-tg-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (State::new(&dir), dir)
    }

    fn sink(server: &Server, topics: TopicMode, state: State) -> Telegram {
        Telegram {
            api_url: server.url.clone(),
            token: "123:abc".into(),
            chat_id: -100,
            topics,
            bridge: false,
            allowed_user_ids: vec![],
            state,
            client: Client::new(),
            poll_client: Client::new(),
        }
    }

    fn json(req: &crate::http::mock::Request) -> serde_json::Value {
        serde_json::from_str(&req.body).unwrap()
    }

    #[test]
    fn sends_html_message_to_chat_root() {
        let server = Server::respond(200, OK);
        let (state, dir) = temp_state("root");
        sink(&server, TopicMode::None, state)
            .send(&alert(Some("run <cmd>?")))
            .unwrap();
        let req = server.request();
        assert_eq!(req.path, "/bot123:abc/sendMessage");
        let body = json(&req);
        assert_eq!(body["chat_id"], -100);
        assert_eq!(body["parse_mode"], "HTML");
        assert!(body.get("message_thread_id").is_none());
        assert_eq!(
            body["text"],
            "🟥 <b>BLOCKED</b> claude · [1] ws &lt;1&gt; · box\npane w1:p1\n<pre>run &lt;cmd&gt;?</pre>"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn creates_topic_once_then_posts_into_it() {
        let server = Server::respond_seq(vec![
            (
                200,
                r#"{"ok":true,"result":{"message_thread_id":555,"name":"x"}}"#,
            ),
            (200, OK),
            (200, OK),
        ]);
        let (state, dir) = temp_state("topic");
        let tg = sink(&server, TopicMode::PerAgent, state);
        tg.send(&alert(None)).unwrap();
        tg.send(&alert(None)).unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[0].path, "/bot123:abc/createForumTopic");
        assert_eq!(json(&reqs[0])["name"], "claude · ws <1> · box");
        assert_eq!(reqs[1].path, "/bot123:abc/sendMessage");
        assert_eq!(json(&reqs[1])["message_thread_id"], 555);
        assert_eq!(reqs[2].path, "/bot123:abc/sendMessage");
        assert_eq!(json(&reqs[2])["message_thread_id"], 555);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn topic_closed_by_plugin_is_reopened_before_posting() {
        let server = Server::respond_seq(vec![
            (200, r#"{"ok":true,"result":{"message_thread_id":7}}"#),
            (200, OK),
            (200, OK), // closeForumTopic
            (200, OK), // reopenForumTopic
            (200, OK),
        ]);
        let (state, dir) = temp_state("reopen");
        let tg = sink(&server, TopicMode::PerAgent, state);
        tg.send(&alert(None)).unwrap();
        let mut closed = alert(None);
        closed.status = Status::Closed;
        tg.send(&closed).unwrap();
        tg.send(&alert(None)).unwrap();
        let paths: Vec<String> = server
            .requests()
            .iter()
            .map(|r| r.path.rsplit('/').next().unwrap().to_string())
            .collect();
        assert_eq!(
            paths,
            [
                "createForumTopic",
                "sendMessage",
                "closeForumTopic",
                "reopenForumTopic",
                "sendMessage"
            ]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn already_closed_topic_is_not_an_error() {
        let server = Server::respond_seq(vec![
            (200, r#"{"ok":true,"result":{"message_thread_id":7}}"#),
            (200, OK),
            (
                400,
                r#"{"ok":false,"description":"Bad Request: TOPIC_NOT_MODIFIED"}"#,
            ),
        ]);
        let (state, dir) = temp_state("notmod");
        let tg = sink(&server, TopicMode::PerAgent, state);
        tg.send(&alert(None)).unwrap();
        let mut closed = alert(None);
        closed.status = Status::Closed;
        assert_eq!(tg.send(&closed).unwrap(), Delivery::Skipped);
        server.requests();
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn deleted_topic_is_recreated() {
        let server = Server::respond_seq(vec![
            (200, r#"{"ok":true,"result":{"message_thread_id":7}}"#),
            (
                400,
                r#"{"ok":false,"description":"Bad Request: message thread not found"}"#,
            ),
            (200, r#"{"ok":true,"result":{"message_thread_id":8}}"#),
            (200, OK),
        ]);
        let (state, dir) = temp_state("recreate");
        sink(&server, TopicMode::PerAgent, state)
            .send(&alert(None))
            .unwrap();
        let reqs = server.requests();
        assert_eq!(reqs[2].path, "/bot123:abc/createForumTopic");
        assert_eq!(json(&reqs[3])["message_thread_id"], 8);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn last_pane_close_closes_the_topic() {
        let server = Server::respond_seq(vec![
            (200, r#"{"ok":true,"result":{"message_thread_id":9}}"#),
            (200, OK),
            (200, OK),
        ]);
        let (state, dir) = temp_state("close");
        let tg = sink(&server, TopicMode::PerAgent, state);
        tg.send(&alert(None)).unwrap();
        let mut closed = alert(None);
        closed.status = Status::Closed;
        assert_eq!(tg.send(&closed).unwrap(), Delivery::Skipped);
        let reqs = server.requests();
        assert_eq!(reqs[2].path, "/bot123:abc/closeForumTopic");
        assert_eq!(json(&reqs[2])["message_thread_id"], 9);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn bridge_adds_keyboard_to_blocked_alerts() {
        let server = Server::respond(200, OK);
        let (state, dir) = temp_state("kb");
        let mut tg = sink(&server, TopicMode::None, state);
        tg.bridge = true;
        tg.send(&alert(None)).unwrap();
        let body = json(&server.request());
        let rows = body["reply_markup"]["inline_keyboard"].as_array().unwrap();
        assert_eq!(rows[0][0]["callback_data"], "k|w1:p1|y|Enter");
        assert_eq!(rows[1][0]["callback_data"], "t|w1:p1");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn poll_maps_updates_and_advances_cursor() {
        let server = Server::respond(
            200,
            r#"{"ok":true,"result":[
              {"update_id":10,"message":{"chat":{"id":-100},"from":{"id":42},"message_thread_id":5,"text":"/tail 5"}},
              {"update_id":11,"message":{"chat":{"id":-999},"from":{"id":42},"text":"other chat"}},
              {"update_id":12,"callback_query":{"id":"cb1","from":{"id":42},"data":"t|w1:p1","message":{"chat":{"id":-100},"message_thread_id":5}}}
            ]}"#,
        );
        let (state, dir) = temp_state("poll");
        let tg = sink(&server, TopicMode::PerAgent, state);
        let inbounds = tg.poll().unwrap();
        assert_eq!(inbounds.len(), 2);
        assert_eq!(inbounds[0].text, "/tail 5");
        assert_eq!(inbounds[0].thread_id, Some(5));
        assert_eq!(inbounds[1].callback_id.as_deref(), Some("cb1"));
        assert_eq!(tg.state.bridge_cursor("telegram"), Some(13));
        let req = server.request();
        assert_eq!(req.path, "/bot123:abc/getUpdates");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn api_error_surfaces_description() {
        let server = Server::respond(
            400,
            r#"{"ok":false,"description":"Bad Request: chat not found"}"#,
        );
        let (state, dir) = temp_state("err");
        let err = sink(&server, TopicMode::None, state)
            .send(&alert(None))
            .unwrap_err();
        assert_eq!(err, "telegram sendMessage 400: Bad Request: chat not found");
        server.request();
        std::fs::remove_dir_all(dir).unwrap();
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
