//! Delivery targets. One module per service; `build` maps config entries to
//! instances. Adding a service means one file here and one match arm below.

mod stdout;

use crate::alert::Alert;
use crate::config::{Config, SinkConfig};

pub trait Sink {
    fn name(&self) -> &str;
    fn send(&self, alert: &Alert) -> Result<(), String>;
}

/// Instantiates every configured sink. With no sinks configured, alerts go to
/// stdout so the plugin log shows what would have been sent.
pub fn build(config: &Config) -> Result<Vec<Box<dyn Sink>>, String> {
    if config.sinks.is_empty() {
        return Ok(vec![Box::new(stdout::Stdout)]);
    }
    config
        .sinks
        .iter()
        .map(|entry| -> Result<Box<dyn Sink>, String> {
            match entry {
                SinkConfig::Stdout => Ok(Box::new(stdout::Stdout)),
            }
        })
        .collect()
}
