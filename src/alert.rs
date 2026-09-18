//! One alert, normalized. Sinks render this; nothing downstream reads Herdr
//! JSON directly.

use crate::config::Config;
use crate::herdr::{self, PluginEnv};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Alert {
    pub host: String,
    pub workspace: String,
    pub agent: String,
    pub pane_id: String,
    pub status: Status,
    /// Recent pane output, present on blocked alerts.
    pub tail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Working,
    Blocked,
    Done,
    Unknown,
    /// The pane closed; sinks that track per-agent state clean up.
    Closed,
}

impl Status {
    pub fn parse(raw: &str) -> Self {
        match raw {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            _ => Self::Unknown,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Unknown => "unknown",
            Self::Closed => "closed",
        }
    }
}

impl Alert {
    /// Builds an alert from the hook environment. Returns `Ok(None)` when the
    /// event is one the config says to ignore.
    pub fn from_event(env: &PluginEnv, config: &Config) -> Result<Option<Self>, String> {
        let event_json = env
            .event_json
            .as_deref()
            .ok_or("HERDR_PLUGIN_EVENT_JSON is not set; this is an event hook")?;
        let event = herdr::parse_event(event_json)?;
        let context = match env.context_json.as_deref() {
            Some(json) => herdr::parse_context(json)?,
            None => herdr::Context::default(),
        };
        Ok(Self::build(&event, &context, config))
    }

    fn build(
        event: &herdr::EventEnvelope,
        context: &herdr::Context,
        config: &Config,
    ) -> Option<Self> {
        let status = match event.event.as_str() {
            "pane.closed" => Status::Closed,
            "pane.agent_status_changed" => Status::parse(event.data.agent_status.as_deref()?),
            _ => return None,
        };
        if status != Status::Closed && !config.alerts.statuses.contains(&status.as_str().into()) {
            return None;
        }
        let data = &event.data;
        let pane_id = data
            .pane_id
            .clone()
            .or_else(|| context.focused_pane_id.clone())?;
        let agent = data
            .display_agent
            .clone()
            .or_else(|| data.agent.clone())
            .or_else(|| context.focused_pane_agent.clone())
            .unwrap_or_else(|| "agent".to_string());
        let workspace = context
            .workspace_label
            .clone()
            .or_else(|| context.workspace_cwd.clone())
            .or_else(|| data.workspace_id.clone())
            .or_else(|| context.workspace_id.clone())
            .unwrap_or_default();
        Some(Self {
            host: config.alerts.host_label.clone(),
            workspace,
            agent,
            pane_id,
            status,
            tail: None,
        })
    }

    pub fn test(config: &Config) -> Self {
        Self {
            host: config.alerts.host_label.clone(),
            workspace: "goat-herdr".to_string(),
            agent: "test".to_string(),
            pane_id: "0".to_string(),
            status: Status::Blocked,
            tail: Some("this is a test alert".to_string()),
        }
    }

    /// The first line of every rendered alert: host, workspace, agent.
    pub fn headline(&self) -> String {
        format!(
            "{} {} · {} · {}",
            self.status.as_str().to_uppercase(),
            self.agent,
            self.workspace,
            self.host
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        let mut config = Config::default();
        config.alerts.host_label = "box".to_string();
        config
    }

    #[test]
    fn blocked_status_change_becomes_alert() {
        let event = herdr::parse_event(
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"pane-3","workspace_id":"ws-1","agent_status":"blocked","agent":"claude","display_agent":"Claude Code"}}"#,
        )
        .unwrap();
        let context =
            herdr::parse_context(r#"{"workspace_id":"ws-1","workspace_label":"goat-herdr"}"#)
                .unwrap();
        let alert = Alert::build(&event, &context, &config()).unwrap();
        assert_eq!(alert.status, Status::Blocked);
        assert_eq!(alert.agent, "Claude Code");
        assert_eq!(alert.workspace, "goat-herdr");
        assert_eq!(alert.pane_id, "pane-3");
        assert_eq!(alert.headline(), "BLOCKED Claude Code · goat-herdr · box");
    }

    #[test]
    fn working_status_is_filtered_by_default() {
        let event = herdr::parse_event(
            r#"{"event":"pane.agent_status_changed","data":{"pane_id":"pane-3","workspace_id":"ws-1","agent_status":"working"}}"#,
        )
        .unwrap();
        let context = herdr::Context::default();
        assert!(Alert::build(&event, &context, &config()).is_none());
    }

    #[test]
    fn pane_closed_becomes_closed_alert() {
        let event = herdr::parse_event(
            r#"{"event":"pane.closed","data":{"pane_id":"pane-3","workspace_id":"ws-1"}}"#,
        )
        .unwrap();
        let context = herdr::Context::default();
        let alert = Alert::build(&event, &context, &config()).unwrap();
        assert_eq!(alert.status, Status::Closed);
        assert_eq!(alert.workspace, "ws-1");
    }
}
