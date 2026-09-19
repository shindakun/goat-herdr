//! The two-way bridge: a detached daemon that polls chat services for input
//! and turns it into Herdr agent commands. One thread per bridge-capable
//! sink. Service specifics live behind the `Bridge` trait.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::config::Config;
use crate::herdr::{AgentInfo, PluginEnv};
use crate::sink;
use crate::state::State;

/// One message or button press from a user, service-agnostic.
#[derive(Debug, Clone)]
pub struct Inbound {
    /// Routing thread the input arrived in, when the service has threads.
    pub thread_id: Option<i64>,
    pub from_user: i64,
    pub text: String,
    /// Set for button presses; the bridge acks these instead of replying.
    pub callback_id: Option<String>,
}

pub trait Bridge: Send {
    fn name(&self) -> &str;
    /// Blocks up to ~30s. Returns the inputs that arrived.
    fn poll(&self) -> Result<Vec<Inbound>, String>;
    /// Posts `text` where `inbound` came from. `pre` asks for a monospace
    /// block.
    fn reply(&self, inbound: &Inbound, text: &str, pre: bool) -> Result<(), String>;
    /// Acknowledges a button press with a short toast.
    fn ack(&self, inbound: &Inbound, text: &str) -> Result<(), String>;
    fn allowed_user(&self, user: i64) -> bool;
}

const HELP: &str = "In an agent's topic:\n\
  plain text      prompt the agent\n\
  /tail [n]       last n lines of the pane (default 40)\n\
  /keys k...      send keys, e.g. /keys y Enter or /keys esc\n\
  /status         this agent's state\n\
Anywhere:\n\
  /agents         list live agents\n\
  /help           this text";

/// Entry point for `goat-herdr bridge`. `detach` forks the daemon and returns
/// once it is running, which is what the startup hook needs.
pub fn run(env: &PluginEnv, config: &Config, detach: bool) -> Result<(), String> {
    let bridges = sink::build_bridges(config, &env.state_dir)?;
    if bridges.is_empty() {
        println!("bridge: no sink has bridge = true; nothing to run");
        return Ok(());
    }
    let state = State::new(&env.state_dir);
    if detach {
        return spawn_detached(&state);
    }
    let _guard = single_instance(&state)?;
    let mut threads = Vec::new();
    for bridge in bridges {
        let env = env.clone();
        let state = State::new(&env.state_dir);
        threads.push(std::thread::spawn(move || serve(bridge, &env, &state)));
    }
    for thread in threads {
        let _ = thread.join();
    }
    Ok(())
}

/// Starts `goat-herdr bridge` as its own process group with output in
/// `bridge.log`, after stopping any daemon from an earlier server.
fn spawn_detached(state: &State) -> Result<(), String> {
    stop_previous(state);
    let exe = std::env::current_exe().map_err(|err| format!("current_exe: {err}"))?;
    let log_path = state.dir().join("bridge.log");
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|err| format!("open {}: {err}", log_path.display()))?;
    let stderr = log
        .try_clone()
        .map_err(|err| format!("clone log handle: {err}"))?;
    let mut command = Command::new(exe);
    command
        .arg("bridge")
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group so the hook's exit does not take the daemon with it.
        command.process_group(0);
    }
    let child = command
        .spawn()
        .map_err(|err| format!("spawn bridge: {err}"))?;
    println!(
        "bridge: started pid {} (log: {})",
        child.id(),
        log_path.display()
    );
    Ok(())
}

/// Sends TERM to the pid in `bridge.pid` and waits briefly for its lock.
fn stop_previous(state: &State) {
    let Ok(pid) = std::fs::read_to_string(state.dir().join("bridge.pid")) else {
        return;
    };
    let pid = pid.trim();
    if pid.is_empty() {
        return;
    }
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", pid]).status();
    }
    for _ in 0..20 {
        if let Ok(file) = File::create(state.dir().join("bridge.lock")) {
            if file.try_lock().is_ok() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Holds `bridge.lock` for the life of the daemon and records the pid.
fn single_instance(state: &State) -> Result<File, String> {
    let path = state.dir().join("bridge.lock");
    let file = File::create(&path).map_err(|err| format!("create {}: {err}", path.display()))?;
    file.try_lock()
        .map_err(|_| "bridge: another instance holds bridge.lock".to_string())?;
    let mut pid = File::create(state.dir().join("bridge.pid"))
        .map_err(|err| format!("write bridge.pid: {err}"))?;
    let _ = writeln!(pid, "{}", std::process::id());
    Ok(file)
}

fn serve(bridge: Box<dyn Bridge>, env: &PluginEnv, state: &State) {
    eprintln!("bridge {}: polling", bridge.name());
    let mut failures = 0u32;
    loop {
        match bridge.poll() {
            Ok(inbounds) => {
                failures = 0;
                for inbound in inbounds {
                    if !bridge.allowed_user(inbound.from_user) {
                        eprintln!(
                            "bridge {}: ignored user {}",
                            bridge.name(),
                            inbound.from_user
                        );
                        continue;
                    }
                    match handle(bridge.as_ref(), env, state, &inbound) {
                        Ok(done) => eprintln!("bridge {}: {done}", bridge.name()),
                        Err(err) => {
                            eprintln!("bridge {}: error: {err}", bridge.name());
                            let _ = bridge.reply(&inbound, &format!("error: {err}"), false);
                        }
                    }
                }
            }
            Err(err) => {
                failures += 1;
                eprintln!("bridge {}: poll failed ({failures}): {err}", bridge.name());
                // Short, capped back-off: a DNS blip should not cost half a minute.
                std::thread::sleep(Duration::from_secs((failures as u64 * 2).min(10)));
            }
        }
    }
}

/// Runs one input against Herdr. Returns a one-line record of what it did.
fn handle(
    bridge: &dyn Bridge,
    env: &PluginEnv,
    state: &State,
    inbound: &Inbound,
) -> Result<String, String> {
    let text = inbound.text.trim();
    if inbound.callback_id.is_some() {
        return handle_button(bridge, env, inbound, text);
    }
    let (command, rest) = split_command(text);
    match command.as_deref() {
        Some("help") | Some("start") => {
            bridge.reply(inbound, HELP, true)?;
            Ok("help".to_string())
        }
        Some("agents") => {
            let agents = env.agents()?;
            bridge.reply(inbound, &agents_table(&agents), true)?;
            Ok("agents".to_string())
        }
        Some("tail") => {
            let pane = target_pane(env, state, inbound)?;
            let lines: u32 = rest.trim().parse().unwrap_or(40);
            let tail = env.read_tail(&pane, lines)?;
            bridge.reply(
                inbound,
                if tail.is_empty() { "(empty)" } else { &tail },
                true,
            )?;
            Ok(format!("tail {lines} of {pane}"))
        }
        Some("keys") => {
            let keys: Vec<&str> = rest.split_whitespace().collect();
            if keys.is_empty() {
                bridge.reply(inbound, "usage: /keys k... (e.g. /keys y Enter)", false)?;
                return Ok("keys usage".to_string());
            }
            let pane = target_pane(env, state, inbound)?;
            env.send_keys(&pane, &keys)?;
            bridge.reply(inbound, &format!("{pane} ← {}", keys.join(" ")), false)?;
            Ok(format!("keys {} -> {pane}", keys.join(" ")))
        }
        Some("status") => {
            let pane = target_pane(env, state, inbound)?;
            let agents = env.agents()?;
            let row = agents
                .iter()
                .filter(|a| a.pane_id == pane)
                .cloned()
                .collect::<Vec<_>>();
            bridge.reply(inbound, &agents_table(&row), true)?;
            Ok(format!("status of {pane}"))
        }
        Some(other) => {
            bridge.reply(
                inbound,
                &format!("unknown command /{other}\n\n{HELP}"),
                true,
            )?;
            Ok(format!("unknown command /{other}"))
        }
        None => {
            if text.is_empty() {
                return Ok("empty".to_string());
            }
            let pane = target_pane(env, state, inbound)?;
            let how = env.agent_prompt(&pane, text)?;
            bridge.reply(inbound, &format!("{how} {pane} ← {text}"), false)?;
            Ok(format!("{how} {pane}, {} chars", text.chars().count()))
        }
    }
}

/// Inline keyboard presses. Data is `k|<pane>|<key>...` or `t|<pane>`.
fn handle_button(
    bridge: &dyn Bridge,
    env: &PluginEnv,
    inbound: &Inbound,
    data: &str,
) -> Result<String, String> {
    let mut parts = data.split('|');
    let (Some(kind), Some(pane)) = (parts.next(), parts.next()) else {
        bridge.ack(inbound, "bad button")?;
        return Ok(format!("bad button {data}"));
    };
    let alive = env.agents()?.iter().any(|a| a.pane_id == pane);
    if !alive {
        bridge.ack(inbound, &format!("{pane} is gone"))?;
        return Ok(format!("button for gone pane {pane}"));
    }
    match kind {
        "k" => {
            let keys: Vec<&str> = parts.collect();
            env.send_keys(pane, &keys)?;
            // The toast is easy to miss; leave a line in the topic too.
            bridge.ack(inbound, &format!("sent {}", keys.join(" ")))?;
            let line = if keys.len() == 1 && keys[0].chars().all(|c| c.is_ascii_digit()) {
                format!("{pane} ← {}. Now send your text as a reply.", keys[0])
            } else {
                format!("{pane} ← {}", keys.join(" "))
            };
            bridge.reply(inbound, &line, false)?;
            Ok(format!("button keys {} -> {pane}", keys.join(" ")))
        }
        "t" => {
            let tail = env.read_tail(pane, 40)?;
            bridge.ack(inbound, "tail")?;
            bridge.reply(
                inbound,
                if tail.is_empty() { "(empty)" } else { &tail },
                true,
            )?;
            Ok(format!("button tail of {pane}"))
        }
        _ => {
            bridge.ack(inbound, "bad button")?;
            Ok(format!("bad button {data}"))
        }
    }
}

/// `/tail 20` -> (Some("tail"), "20"); `/tail@bot` strips the mention.
fn split_command(text: &str) -> (Option<String>, &str) {
    let Some(rest) = text.strip_prefix('/') else {
        return (None, text);
    };
    let (word, args) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    let word = word.split('@').next().unwrap_or(word);
    (Some(word.to_ascii_lowercase()), args)
}

/// The pane a topic's input goes to: a live pane registered under the
/// topic's key, preferring one that is blocked.
fn target_pane(env: &PluginEnv, state: &State, inbound: &Inbound) -> Result<String, String> {
    let Some(thread_id) = inbound.thread_id else {
        return Err("send this inside an agent's topic".to_string());
    };
    let Some((key, panes)) = state.topic_panes_for_thread(thread_id)? else {
        return Err("this topic is not tied to an agent".to_string());
    };
    let agents = env.agents()?;
    let mut live: Vec<&AgentInfo> = agents
        .iter()
        .filter(|a| panes.contains(&a.pane_id))
        .collect();
    if live.is_empty() {
        return Err(format!("no live agent for {}", key.replace('|', " · ")));
    }
    live.sort_by_key(|a| a.agent_status.as_deref() != Some("blocked"));
    Ok(live[0].pane_id.clone())
}

fn agents_table(agents: &[AgentInfo]) -> String {
    if agents.is_empty() {
        return "no agents".to_string();
    }
    agents
        .iter()
        .map(|a| {
            format!(
                "{:<8} {:<8} {:<8} {}",
                a.pane_id,
                a.agent.as_deref().unwrap_or("?"),
                a.agent_status.as_deref().unwrap_or("?"),
                a.cwd.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_split_and_strip_mentions() {
        assert_eq!(split_command("/tail 20"), (Some("tail".into()), "20"));
        assert_eq!(split_command("/tail@goat_bot"), (Some("tail".into()), ""));
        assert_eq!(
            split_command("/KEYS y Enter"),
            (Some("keys".into()), "y Enter")
        );
        assert_eq!(split_command("just text"), (None, "just text"));
    }

    #[test]
    fn agents_table_lists_rows() {
        let rows = vec![AgentInfo {
            pane_id: "w1:p1".into(),
            agent: Some("claude".into()),
            agent_status: Some("blocked".into()),
            cwd: Some("/x".into()),
        }];
        assert_eq!(agents_table(&rows), "w1:p1    claude   blocked  /x");
        assert_eq!(agents_table(&[]), "no agents");
    }
}
