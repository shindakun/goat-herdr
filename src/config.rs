//! `config.toml` in `HERDR_PLUGIN_CONFIG_DIR`. See docs/PLAN.md for the full
//! shape. A missing file yields the defaults with a stdout sink so a fresh
//! install is observable in `herdr plugin log list`.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::herdr::PluginEnv;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub alerts: Alerts,
    #[serde(default)]
    pub sinks: Vec<SinkConfig>,
    /// `KEY=VALUE` pairs from `.env` beside `config.toml`. Consulted before the
    /// process environment when a sink names a `*_env` variable.
    #[serde(skip)]
    pub dotenv: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alerts {
    /// Agent statuses that fire an alert.
    #[serde(default = "default_statuses")]
    pub statuses: Vec<String>,
    /// Drop a repeat of the same pane and status inside this window.
    #[serde(default = "default_debounce_secs")]
    pub debounce_secs: u64,
    /// Lines of pane output attached to blocked alerts. 0 disables.
    #[serde(default = "default_tail_lines")]
    pub tail_lines: u32,
    /// Name of this machine in every alert. Defaults to the hostname.
    #[serde(default = "default_host_label")]
    pub host_label: String,
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            statuses: default_statuses(),
            debounce_secs: default_debounce_secs(),
            tail_lines: default_tail_lines(),
            host_label: default_host_label(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SinkConfig {
    Stdout,
    Telegram(TelegramConfig),
    Ntfy(NtfyConfig),
    Slack(SlackConfig),
    Webhook(WebhookConfig),
    Discord(DiscordConfig),
}

/// Either a channel webhook, or a bot token plus the channel it posts to.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub webhook_url_env: Option<String>,
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub bot_token_env: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    /// Bot API base; override for tests.
    #[serde(default = "default_discord_api")]
    pub api_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub url_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlackConfig {
    #[serde(default)]
    pub webhook_url: Option<String>,
    #[serde(default)]
    pub webhook_url_env: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TelegramConfig {
    #[serde(default)]
    pub bot_token: Option<String>,
    #[serde(default)]
    pub bot_token_env: Option<String>,
    pub chat_id: i64,
    /// Forum topic routing. Needs a supergroup with Topics on and the bot as
    /// an admin with Manage Topics.
    #[serde(default)]
    pub topics: TopicMode,
    /// Run the two-way bridge: replies in a topic go to that agent, blocked
    /// alerts carry an inline keyboard. Needs `allowed_user_ids`.
    #[serde(default)]
    pub bridge: bool,
    /// Telegram user ids the bridge takes input from. Everyone else is ignored.
    #[serde(default)]
    pub allowed_user_ids: Vec<i64>,
    /// Override for tests and self-hosted Bot API servers.
    #[serde(default = "default_telegram_api")]
    pub api_url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TopicMode {
    /// Everything in the chat root.
    #[default]
    None,
    /// One topic per agent pane: host, workspace, agent, and pane id. A
    /// reply in the topic has exactly one place to go.
    PerAgent,
    /// One topic per host and workspace.
    PerWorkspace,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NtfyConfig {
    pub url: String,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub token_env: Option<String>,
}

impl Config {
    pub fn load(env: &PluginEnv) -> Result<Self, String> {
        let path = env.config_dir.join("config.toml");
        let mut config: Self = match std::fs::read_to_string(&path) {
            Ok(text) => {
                toml::from_str(&text).map_err(|err| format!("parse {}: {err}", path.display()))?
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(err) => return Err(format!("read {}: {err}", path.display())),
        };
        config.dotenv = read_dotenv(&env.config_dir.join(".env"));
        Ok(config)
    }

    /// A secret given inline, or named by an environment variable that is
    /// looked up in `.env` first and the process environment second.
    pub fn secret(
        &self,
        inline: Option<&str>,
        env_name: Option<&str>,
        what: &str,
    ) -> Result<String, String> {
        if let Some(value) = inline.filter(|v| !v.is_empty()) {
            return Ok(value.to_string());
        }
        let Some(name) = env_name else {
            return Err(format!("{what}: set it inline or name an env var"));
        };
        self.dotenv
            .get(name)
            .cloned()
            .or_else(|| std::env::var(name).ok())
            .filter(|v| !v.is_empty())
            .ok_or_else(|| format!("{what}: {name} is not set in .env or the environment"))
    }
}

/// Reads `KEY=VALUE` lines. Blank lines and `#` comments are skipped; a
/// leading `export ` and matching surrounding quotes are stripped.
fn read_dotenv(path: &Path) -> HashMap<String, String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    parse_dotenv(&text)
}

fn parse_dotenv(text: &str) -> HashMap<String, String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| {
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(value);
            Some((key.trim().to_string(), value.to_string()))
        })
        .collect()
}

fn default_statuses() -> Vec<String> {
    vec!["blocked".to_string(), "done".to_string()]
}

fn default_debounce_secs() -> u64 {
    5
}

fn default_tail_lines() -> u32 {
    30
}

fn default_discord_api() -> String {
    "https://discord.com/api/v10".to_string()
}

fn default_telegram_api() -> String {
    "https://api.telegram.org".to_string()
}

fn default_host_label() -> String {
    // `hostname` is on every supported platform and avoids a crate for one call.
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "herdr".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_uses_defaults() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.alerts.statuses, ["blocked", "done"]);
        assert_eq!(config.alerts.debounce_secs, 5);
        assert_eq!(config.alerts.tail_lines, 30);
        assert!(config.sinks.is_empty());
    }

    #[test]
    fn sinks_parse_by_type() {
        let config: Config = toml::from_str(
            r#"
[alerts]
statuses = ["blocked"]
host_label = "box"

[[sinks]]
type = "stdout"

[[sinks]]
type = "telegram"
bot_token_env = "TELEGRAM_BOT_TOKEN"
chat_id = -1001234567890
topics = "per-agent"
bridge = true
allowed_user_ids = [42]

[[sinks]]
type = "ntfy"
url = "https://ntfy.sh/goats"

[[sinks]]
type = "slack"
webhook_url_env = "SLACK_WEBHOOK_URL"

[[sinks]]
type = "webhook"
url = "https://example.test/hook"

[[sinks]]
type = "discord"
bot_token_env = "DISCORD_BOT_TOKEN"
channel_id = "186985279377113088"
"#,
        )
        .unwrap();
        assert_eq!(config.alerts.host_label, "box");
        assert!(matches!(config.sinks[0], SinkConfig::Stdout));
        match &config.sinks[1] {
            SinkConfig::Telegram(t) => {
                assert_eq!(t.chat_id, -1001234567890);
                assert_eq!(t.topics, TopicMode::PerAgent);
                assert!(t.bridge);
                assert_eq!(t.allowed_user_ids, [42]);
                assert_eq!(t.api_url, "https://api.telegram.org");
            }
            other => panic!("expected telegram, got {other:?}"),
        }
        assert!(matches!(config.sinks[2], SinkConfig::Ntfy(_)));
        assert!(matches!(config.sinks[3], SinkConfig::Slack(_)));
        assert!(matches!(config.sinks[4], SinkConfig::Webhook(_)));
        match &config.sinks[5] {
            SinkConfig::Discord(d) => {
                assert_eq!(d.channel_id.as_deref(), Some("186985279377113088"))
            }
            other => panic!("expected discord, got {other:?}"),
        }
    }

    #[test]
    fn unknown_sink_type_is_an_error() {
        let result: Result<Config, _> = toml::from_str("[[sinks]]\ntype = \"pager\"\n");
        assert!(result.is_err());
    }

    #[test]
    fn dotenv_parses_common_forms() {
        let map = parse_dotenv("# comment\nA=1\nexport B=\"two words\"\nC='3'\n\nbad line\n");
        assert_eq!(map["A"], "1");
        assert_eq!(map["B"], "two words");
        assert_eq!(map["C"], "3");
        assert_eq!(map.len(), 3);
    }

    #[test]
    fn secret_prefers_inline_then_dotenv() {
        let mut config = Config::default();
        config
            .dotenv
            .insert("TOK".to_string(), "from-dotenv".to_string());
        assert_eq!(
            config.secret(Some("inline"), Some("TOK"), "t").unwrap(),
            "inline"
        );
        assert_eq!(
            config.secret(None, Some("TOK"), "t").unwrap(),
            "from-dotenv"
        );
        assert!(config
            .secret(None, Some("GOAT_HERDR_MISSING"), "t")
            .is_err());
        assert!(config.secret(None, None, "t").is_err());
    }
}
