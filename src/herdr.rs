//! The environment Herdr injects into plugin commands, the JSON shapes inside
//! it, and calls back into Herdr through `HERDR_BIN_PATH`. Field names follow
//! herdr's `src/api/schema` as of 0.9.1; see tests/fixtures for captures.

use std::path::PathBuf;
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone)]
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
        self.run(&[
            "pane",
            "read",
            pane_id,
            "--lines",
            &lines.to_string(),
            "--format",
            "text",
        ])
    }

    /// Live agent panes.
    pub fn agents(&self) -> Result<Vec<AgentInfo>, String> {
        let out = self.run(&["agent", "list"])?;
        let envelope: AgentListEnvelope = serde_json::from_str(&out)
            .map_err(|err| format!("herdr agent list: unexpected output: {err}"))?;
        Ok(envelope.result.agents)
    }

    /// Gives text to an agent. `agent prompt` knows each agent's submit
    /// rules but refuses a blocked agent ("requires interactive input") and
    /// panes Herdr has not classified as a named agent; those get the text
    /// typed in followed by Enter, which is what a blocked prompt wants.
    pub fn agent_prompt(&self, pane_id: &str, text: &str) -> Result<&'static str, String> {
        match self.run(&["agent", "prompt", pane_id, text]) {
            Ok(_) => Ok("prompted"),
            Err(err) if err.contains("agent_blocked") || err.contains("agent_not_ready") => {
                self.run(&["pane", "send-text", pane_id, text])?;
                self.send_keys(pane_id, &["Enter"])?;
                Ok("typed")
            }
            Err(err) => Err(err),
        }
    }

    /// Key presses to any pane. `pane send-keys` works whether or not
    /// Herdr classifies the pane as a named agent.
    pub fn send_keys(&self, pane_id: &str, keys: &[&str]) -> Result<(), String> {
        let mut args = vec!["pane", "send-keys", pane_id];
        args.extend_from_slice(keys);
        self.run(&args).map(|_| ())
    }

    /// Runs the Herdr CLI and returns stdout. A failing exit is an error
    /// carrying stderr.
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = Command::new(&self.bin_path)
            .args(args)
            .output()
            .map_err(|err| format!("run {} {}: {err}", self.bin_path.display(), args[0]))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(format!(
                "herdr {}: {}",
                args.join(" "),
                cli_error(stderr.trim())
                    .or_else(|| cli_error(stdout.trim()))
                    .unwrap_or_else(|| stderr.trim().to_string())
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string())
    }
}

/// `{"error":{"code":"x","message":"y"}}` -> `x: y`.
fn cli_error(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let error = value.get("error")?;
    let code = error["code"].as_str()?;
    let message = error["message"].as_str().unwrap_or("");
    Some(format!("{code}: {message}"))
}

#[derive(Debug, Deserialize)]
struct AgentListEnvelope {
    result: AgentListResult,
}

#[derive(Debug, Deserialize)]
struct AgentListResult {
    agents: Vec<AgentInfo>,
}

/// One row of `herdr agent list`, the fields the bridge reads.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    pub pane_id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub agent_status: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
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
