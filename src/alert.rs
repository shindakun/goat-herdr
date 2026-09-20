//! One alert, normalized. Sinks render this; nothing downstream reads Herdr
//! JSON directly.

use crate::config::Config;
use crate::herdr;

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

    pub fn emoji(self) -> &'static str {
        match self {
            Self::Idle => "⬜",
            Self::Working => "🟦",
            Self::Blocked => "🟥",
            Self::Done => "🟩",
            Self::Unknown => "⬛",
            Self::Closed => "✖",
        }
    }
}

impl Alert {
    /// An alert from a hook event, or `None` when the config ignores it.
    pub fn from_event(
        event: &herdr::EventEnvelope,
        context: &herdr::Context,
        config: &Config,
    ) -> Option<Self> {
        let status = match event.event.as_str() {
            // A pane whose process ends fires pane_exited, not pane_closed.
            "pane_closed" | "pane_exited" => Status::Closed,
            "pane_agent_status_changed" => Status::parse(event.data.agent_status.as_deref()?),
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

    /// Workspace name without Herdr's `[n]` ordinal. The ordinal changes on
    /// reorder and must not split one project across topics.
    pub fn workspace_name(&self) -> &str {
        let label = self.workspace.as_str();
        let Some(rest) = label.strip_prefix('[') else {
            return label;
        };
        match rest.split_once("] ") {
            Some((ordinal, name))
                if !ordinal.is_empty() && ordinal.bytes().all(|b| b.is_ascii_digit()) =>
            {
                name
            }
            _ => label,
        }
    }

    /// The first line of every rendered alert: status, agent, workspace, host.
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

    const STATUS_EVENT: &str = include_str!("../tests/fixtures/agent_status_changed.event.json");
    const STATUS_CONTEXT: &str =
        include_str!("../tests/fixtures/agent_status_changed.context.json");
    const CLOSED_EVENT: &str = include_str!("../tests/fixtures/pane_closed.event.json");
    const CLOSED_CONTEXT: &str = include_str!("../tests/fixtures/pane_closed.context.json");

    fn config() -> Config {
        let mut config = Config::default();
        config.alerts.host_label = "box".to_string();
        config
    }

    #[test]
    fn blocked_status_change_becomes_alert() {
        let event = herdr::parse_event(STATUS_EVENT).unwrap();
        let context = herdr::parse_context(STATUS_CONTEXT).unwrap();
        let alert = Alert::from_event(&event, &context, &config()).unwrap();
        assert_eq!(alert.status, Status::Blocked);
        assert_eq!(alert.agent, "claude");
        assert_eq!(alert.workspace, "[1] goat-herdr");
        assert_eq!(alert.pane_id, "w3:pD");
        assert_eq!(alert.headline(), "BLOCKED claude · [1] goat-herdr · box");
    }

    #[test]
    fn workspace_name_drops_the_ordinal() {
        let mut a = Alert::test(&config());
        a.workspace = "[12] goat-herdr".into();
        assert_eq!(a.workspace_name(), "goat-herdr");
        a.workspace = "[x] not an ordinal".into();
        assert_eq!(a.workspace_name(), "[x] not an ordinal");
        a.workspace = "plain".into();
        assert_eq!(a.workspace_name(), "plain");
    }

    #[test]
    fn display_agent_wins_over_agent() {
        let event = herdr::parse_event(&STATUS_EVENT.replace(
            r#""agent":"claude""#,
            r#""agent":"claude","display_agent":"Claude Code""#,
        ))
        .unwrap();
        let context = herdr::parse_context(STATUS_CONTEXT).unwrap();
        let alert = Alert::from_event(&event, &context, &config()).unwrap();
        assert_eq!(alert.agent, "Claude Code");
    }

    #[test]
    fn unlisted_status_is_filtered() {
        let event =
            herdr::parse_event(&STATUS_EVENT.replace(r#""blocked""#, r#""working""#)).unwrap();
        let context = herdr::parse_context(STATUS_CONTEXT).unwrap();
        assert!(Alert::from_event(&event, &context, &config()).is_none());
    }

    #[test]
    fn pane_exited_is_treated_as_closed() {
        let event =
            herdr::parse_event(&CLOSED_EVENT.replace("pane_closed", "pane_exited")).unwrap();
        let context = herdr::parse_context(CLOSED_CONTEXT).unwrap();
        let alert = Alert::from_event(&event, &context, &config()).unwrap();
        assert_eq!(alert.status, Status::Closed);
    }

    #[test]
    fn pane_closed_becomes_closed_alert() {
        let event = herdr::parse_event(CLOSED_EVENT).unwrap();
        let context = herdr::parse_context(CLOSED_CONTEXT).unwrap();
        let alert = Alert::from_event(&event, &context, &config()).unwrap();
        assert_eq!(alert.status, Status::Closed);
        assert_eq!(alert.pane_id, "w3:pD");
        assert_eq!(alert.workspace, "w3");
    }
}
