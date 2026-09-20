//! Delivery targets. One module per service; `build` maps config entries to
//! instances. Adding a service means one file here and one match arm below.

mod desktop;
mod discord;
mod ntfy;
mod pushbullet;
mod pushover;
mod slack;
mod stdout;
mod telegram;
mod webhook;

use std::path::Path;

use crate::alert::Alert;
use crate::bridge::Bridge;
use crate::config::{Config, SinkConfig};
use crate::state::State;

pub trait Sink {
    fn name(&self) -> &str;
    fn send(&self, alert: &Alert) -> Result<Delivery, String>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Nothing to do for this alert, such as a pane close on a sink with no
    /// per-agent state.
    Skipped,
}

/// Instantiates every configured sink. With no sinks configured, alerts go to
/// stdout so the plugin log shows what would have been sent.
pub fn build(config: &Config, state_dir: &Path) -> Result<Vec<Box<dyn Sink>>, String> {
    if config.sinks.is_empty() {
        return Ok(vec![Box::new(stdout::Stdout)]);
    }
    config
        .sinks
        .iter()
        .map(|entry| -> Result<Box<dyn Sink>, String> {
            match entry {
                SinkConfig::Stdout => Ok(Box::new(stdout::Stdout)),
                SinkConfig::Telegram(cfg) => Ok(Box::new(telegram::Telegram::new(
                    cfg,
                    config,
                    State::new(state_dir),
                )?)),
                SinkConfig::Ntfy(cfg) => Ok(Box::new(ntfy::Ntfy::new(cfg, config)?)),
                SinkConfig::Slack(cfg) => Ok(Box::new(slack::Slack::new(cfg, config)?)),
                SinkConfig::Webhook(cfg) => Ok(Box::new(webhook::Webhook::new(cfg, config)?)),
                SinkConfig::Discord(cfg) => Ok(Box::new(discord::Discord::new(cfg, config)?)),
                SinkConfig::Pushbullet(cfg) => {
                    Ok(Box::new(pushbullet::Pushbullet::new(cfg, config)?))
                }
                SinkConfig::Pushover(cfg) => Ok(Box::new(pushover::Pushover::new(cfg, config)?)),
                SinkConfig::Desktop(cfg) => Ok(Box::new(desktop::Desktop::new(cfg))),
            }
        })
        .collect()
}

/// Every configured sink that has its two-way bridge turned on.
pub fn build_bridges(config: &Config, state_dir: &Path) -> Result<Vec<Box<dyn Bridge>>, String> {
    let mut bridges: Vec<Box<dyn Bridge>> = Vec::new();
    for entry in &config.sinks {
        if let SinkConfig::Telegram(cfg) = entry {
            if cfg.bridge {
                bridges.push(Box::new(telegram::Telegram::new(
                    cfg,
                    config,
                    State::new(state_dir),
                )?));
            }
        }
    }
    Ok(bridges)
}

/// Plain-text body shared by sinks without their own markup.
pub fn plain_text(alert: &Alert) -> String {
    let mut text = format!(
        "{} {}\npane {}",
        alert.status.emoji(),
        alert.headline(),
        alert.pane_id
    );
    if let Some(tail) = &alert.tail {
        text.push_str("\n---\n");
        text.push_str(tail);
    }
    text
}
