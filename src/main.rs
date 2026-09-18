//! goat-herdr: a Herdr plugin that alerts you when an agent needs you.
//!
//! Herdr runs this binary as an event hook, a startup hook, or an action.
//! Each run is a short-lived process that reads its input from the
//! environment Herdr injects, so all I/O here is blocking.

mod alert;
mod config;
mod herdr;
mod sink;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("notify") => notify(),
        Some("test") => test(),
        Some("toggle") => Err("toggle: not implemented".into()),
        Some("bridge") => Err("bridge: not implemented".into()),
        Some(other) => Err(format!("unknown subcommand: {other}")),
        None => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("goat-herdr: {err}");
            ExitCode::FAILURE
        }
    }
}

const USAGE: &str = "usage: goat-herdr <notify|bridge|test|toggle>";

/// Event hook entry point. Builds one alert from the Herdr event and hands it
/// to every configured sink.
fn notify() -> Result<(), String> {
    let env = herdr::PluginEnv::from_env()?;
    let config = config::Config::load(&env)?;
    let Some(alert) = alert::Alert::from_event(&env, &config)? else {
        return Ok(());
    };
    deliver(&config, &alert)
}

/// Action entry point. Sends a fixed alert so the user can confirm the wiring.
fn test() -> Result<(), String> {
    let env = herdr::PluginEnv::from_env()?;
    let config = config::Config::load(&env)?;
    let alert = alert::Alert::test(&config);
    deliver(&config, &alert)
}

fn deliver(config: &config::Config, alert: &alert::Alert) -> Result<(), String> {
    let sinks = sink::build(config)?;
    let mut failures = Vec::new();
    for sink in &sinks {
        if let Err(err) = sink.send(alert) {
            failures.push(format!("{}: {err}", sink.name()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
