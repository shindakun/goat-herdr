//! `config.toml` in `HERDR_PLUGIN_CONFIG_DIR`. See docs/PLAN.md for the full
//! shape. A missing file yields the defaults with a stdout sink so a fresh
//! install is observable in `herdr plugin log list`.

use serde::Deserialize;

use crate::herdr::PluginEnv;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub alerts: Alerts,
    #[serde(default)]
    pub sinks: Vec<SinkConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alerts {
    /// Agent statuses that fire an alert.
    #[serde(default = "default_statuses")]
    pub statuses: Vec<String>,
    /// Name of this machine in every alert. Defaults to the hostname.
    #[serde(default = "default_host_label")]
    pub host_label: String,
}

impl Default for Alerts {
    fn default() -> Self {
        Self {
            statuses: default_statuses(),
            host_label: default_host_label(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum SinkConfig {
    Stdout,
}

impl Config {
    pub fn load(env: &PluginEnv) -> Result<Self, String> {
        let path = env.config_dir.join("config.toml");
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(err) => return Err(format!("read {}: {err}", path.display())),
        };
        toml::from_str(&text).map_err(|err| format!("parse {}: {err}", path.display()))
    }
}

fn default_statuses() -> Vec<String> {
    vec!["blocked".to_string(), "done".to_string()]
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
"#,
        )
        .unwrap();
        assert_eq!(config.alerts.host_label, "box");
        assert!(matches!(config.sinks[0], SinkConfig::Stdout));
    }

    #[test]
    fn unknown_sink_type_is_an_error() {
        let result: Result<Config, _> = toml::from_str("[[sinks]]\ntype = \"pager\"\n");
        assert!(result.is_err());
    }
}
