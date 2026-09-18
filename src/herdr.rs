//! The environment Herdr injects into plugin commands, and the JSON shapes
//! inside it. Field names follow herdr's `src/api/schema` as of 0.9.1.

use std::path::PathBuf;

use serde::Deserialize;

pub struct PluginEnv {
    pub config_dir: PathBuf,
    pub event_json: Option<String>,
    pub context_json: Option<String>,
}

impl PluginEnv {
    pub fn from_env() -> Result<Self, String> {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        Ok(Self {
            config_dir: var("HERDR_PLUGIN_CONFIG_DIR")
                .map(PathBuf::from)
                .ok_or("HERDR_PLUGIN_CONFIG_DIR is not set; run under herdr")?,
            event_json: var("HERDR_PLUGIN_EVENT_JSON"),
            context_json: var("HERDR_PLUGIN_CONTEXT_JSON"),
        })
    }
}

/// `HERDR_PLUGIN_EVENT_JSON`: `{ "event": "...", "data": { ... } }`.
#[derive(Debug, Deserialize)]
pub struct EventEnvelope {
    pub event: String,
    pub data: EventData,
}

/// Union of the fields this plugin reads from any hooked event. Unused fields
/// for a given event are absent and default.
#[derive(Debug, Default, Deserialize)]
pub struct EventData {
    #[serde(default)]
    pub pane_id: Option<String>,
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub display_agent: Option<String>,
}

/// `HERDR_PLUGIN_CONTEXT_JSON`, the subset this plugin reads.
#[derive(Debug, Default, Deserialize)]
pub struct Context {
    #[serde(default)]
    pub workspace_id: Option<String>,
    #[serde(default)]
    pub workspace_label: Option<String>,
    #[serde(default)]
    pub workspace_cwd: Option<String>,
    #[serde(default)]
    pub focused_pane_id: Option<String>,
    #[serde(default)]
    pub focused_pane_agent: Option<String>,
}

pub fn parse_event(json: &str) -> Result<EventEnvelope, String> {
    serde_json::from_str(json).map_err(|err| format!("invalid HERDR_PLUGIN_EVENT_JSON: {err}"))
}

pub fn parse_context(json: &str) -> Result<Context, String> {
    serde_json::from_str(json).map_err(|err| format!("invalid HERDR_PLUGIN_CONTEXT_JSON: {err}"))
}
