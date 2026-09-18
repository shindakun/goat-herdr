//! The environment Herdr injects into plugin commands, the JSON shapes inside
//! it, and calls back into Herdr through `HERDR_BIN_PATH`. Field names follow
//! herdr's `src/api/schema` as of 0.9.1; see tests/fixtures for captures.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

pub struct PluginEnv {
    pub config_dir: PathBuf,
    pub state_dir: PathBuf,
    pub bin_path: PathBuf,
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
            state_dir: var("HERDR_PLUGIN_STATE_DIR")
                .map(PathBuf::from)
                .ok_or("HERDR_PLUGIN_STATE_DIR is not set; run under herdr")?,
            bin_path: var("HERDR_BIN_PATH")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("herdr")),
            event_json: var("HERDR_PLUGIN_EVENT_JSON"),
            context_json: var("HERDR_PLUGIN_CONTEXT_JSON"),
        })
    }

    /// The last `lines` of a pane's terminal, ANSI stripped.
    pub fn read_tail(&self, pane_id: &str, lines: u32) -> Result<String, String> {
        let output = Command::new(&self.bin_path)
            .args([
                "pane",
                "read",
                pane_id,
                "--lines",
                &lines.to_string(),
                "--format",
                "text",
            ])
            .output()
            .map_err(|err| format!("run {} pane read: {err}", self.bin_path.display()))?;
        if !output.status.success() {
            return Err(format!(
                "herdr pane read {pane_id}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }
}

/// `HERDR_PLUGIN_EVENT_JSON`: `{ "event": "pane_agent_status_changed", "data": { ... } }`.
/// The `event` field is the snake_case kind, not the dotted hook name.
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
