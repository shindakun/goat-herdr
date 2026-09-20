//! goat-herdr: a Herdr plugin that alerts you when an agent needs you.
//!
//! Herdr runs this binary as an event hook, a startup hook, or an action.
//! Each run is a short-lived process that reads its input from the
//! environment Herdr injects, so all I/O here is blocking.

mod alert;
mod bridge;
mod config;
mod herdr;
mod http;
mod sink;
mod state;

use std::process::ExitCode;

use alert::{Alert, Status};
use config::Config;
use herdr::PluginEnv;
use state::State;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("notify") => notify(),
        Some("test") => test(),
        Some("toggle") => toggle(),
        Some("bridge") => bridge_cmd(args.iter().any(|a| a == "--detach")),
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

const USAGE: &str = "usage: goat-herdr <notify|bridge [--detach]|test|toggle>";

/// Startup hook (`--detach`) or foreground daemon.
fn bridge_cmd(detach: bool) -> Result<(), String> {
    let env = PluginEnv::from_env()?;
    let config = Config::load(&env)?;
    bridge::run(&env, &config, detach)
}

/// Event hook entry point. Builds one alert from the Herdr event and hands it
/// to every configured sink.
fn notify() -> Result<(), String> {
    let env = PluginEnv::from_env()?;
    let config = Config::load(&env)?;
    let state = State::new(&env.state_dir);
    let event_json = env
        .event_json
        .as_deref()
        .ok_or("HERDR_PLUGIN_EVENT_JSON is not set; this is an event hook")?;
    let event = herdr::parse_event(event_json)?;
    let context = match env.context_json.as_deref() {
        Some(json) => herdr::parse_context(json)?,
        None => herdr::Context::default(),
    };
    let Some(mut alert) = Alert::from_event(&event, &context, &config) else {
        return Ok(());
    };
    if alert.status == Status::Closed {
        state.forget(&alert.pane_id)?;
    } else {
        if state.paused() {
            return Ok(());
        }
        if !state.debounce(
            &alert.pane_id,
            alert.status.as_str(),
            config.alerts.debounce_secs,
        )? {
            return Ok(());
        }
        if alert.status == Status::Blocked && config.alerts.tail_lines > 0 {
            // The alert goes out without a tail.
            let started = std::time::Instant::now();
            match env.read_tail(&alert.pane_id, config.alerts.tail_lines) {
                Ok(tail) => alert.tail = Some(tail),
                Err(err) => eprintln!("goat-herdr: {err}"),
            }
            println!("tail: {}ms", started.elapsed().as_millis());
        }
    }
    deliver(&config, &env, &alert)
}

/// Action entry point. Sends a fixed alert so the user can confirm the wiring.
fn test() -> Result<(), String> {
    let env = PluginEnv::from_env()?;
    let config = Config::load(&env)?;
    deliver(&config, &env, &Alert::test(&config))
}

/// Action entry point. Pauses or resumes alerts.
fn toggle() -> Result<(), String> {
    let env = PluginEnv::from_env()?;
    let paused = State::new(&env.state_dir).toggle_paused()?;
    println!(
        "goat-herdr alerts {}",
        if paused { "paused" } else { "resumed" }
    );
    Ok(())
}

fn deliver(config: &Config, env: &PluginEnv, alert: &Alert) -> Result<(), String> {
    let sinks = sink::build(config, &env.state_dir)?;
    let mut failures = Vec::new();
    for sink in &sinks {
        let started = std::time::Instant::now();
        let outcome = sink.send(alert);
        let ms = started.elapsed().as_millis();
        match outcome {
            Ok(sink::Delivery::Sent) => {
                println!("{}: sent {} ({ms}ms)", sink.name(), alert.headline())
            }
            Ok(sink::Delivery::Skipped) => {}
            Err(err) => failures.push(format!("{}: {err} ({ms}ms)", sink.name())),
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}
