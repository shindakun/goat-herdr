//! Desktop notification on the machine running Herdr: `osascript` on
//! macOS, `notify-send` on Linux. No network.

use std::process::Command;

use crate::alert::{Alert, Status};
use crate::config::DesktopConfig;
use crate::sink::{Delivery, Sink};

/// Two tail lines fit a banner.
const TAIL_LINES: usize = 2;

pub struct Desktop {
    sound: Option<String>,
}

impl Desktop {
    pub fn new(cfg: &DesktopConfig) -> Self {
        Self {
            sound: cfg.sound.clone(),
        }
    }
}

impl Sink for Desktop {
    fn name(&self) -> &str {
        "desktop"
    }

    fn send(&self, alert: &Alert) -> Result<Delivery, String> {
        if alert.status == Status::Closed {
            return Ok(Delivery::Skipped);
        }
        let (program, args) = command(alert, self.sound.as_deref());
        let output = Command::new(program)
            .args(&args)
            .output()
            .map_err(|err| format!("desktop: run {program}: {err}"))?;
        if output.status.success() {
            Ok(Delivery::Sent)
        } else {
            Err(format!(
                "desktop: {program} exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

fn body(alert: &Alert) -> String {
    let mut lines = vec![format!("pane {}", alert.pane_id)];
    if let Some(tail) = &alert.tail {
        lines.extend(
            tail.lines()
                .rev()
                .filter(|l| !l.trim().is_empty())
                .take(TAIL_LINES)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .map(str::to_string),
        );
    }
    lines.join("\n")
}

/// The program and argv for this platform.
#[cfg(target_os = "macos")]
fn command(alert: &Alert, sound: Option<&str>) -> (&'static str, Vec<String>) {
    let mut script = format!(
        "display notification \"{}\" with title \"{}\"",
        applescript(&body(alert)),
        applescript(&alert.headline())
    );
    if let Some(sound) = sound {
        script.push_str(&format!(" sound name \"{}\"", applescript(sound)));
    }
    ("osascript", vec!["-e".to_string(), script])
}

#[cfg(not(target_os = "macos"))]
fn command(alert: &Alert, _sound: Option<&str>) -> (&'static str, Vec<String>) {
    let urgency = match alert.status {
        Status::Blocked => "critical",
        Status::Done => "normal",
        _ => "low",
    };
    (
        "notify-send",
        vec![
            "-a".to_string(),
            "goat-herdr".to_string(),
            "-u".to_string(),
            urgency.to_string(),
            alert.headline(),
            body(alert),
        ],
    )
}

/// AppleScript string literal contents: backslash and double quote escaped.
#[cfg(target_os = "macos")]
fn applescript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert() -> Alert {
        Alert {
            host: "box".into(),
            workspace: "ws".into(),
            agent: "claude".into(),
            pane_id: "w1:p1".into(),
            status: Status::Blocked,
            tail: Some("one\ntwo\n\nsay \"hi\"\n".into()),
        }
    }

    #[test]
    fn body_is_pane_plus_last_two_tail_lines() {
        assert_eq!(body(&alert()), "pane w1:p1\ntwo\nsay \"hi\"");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_osascript_with_escaped_strings() {
        let (program, args) = command(&alert(), Some("Ping"));
        assert_eq!(program, "osascript");
        assert_eq!(args[0], "-e");
        assert_eq!(
            args[1],
            "display notification \"pane w1:p1\\ntwo\\nsay \\\"hi\\\"\" with title \"BLOCKED claude · ws · box\" sound name \"Ping\""
                .replace("\\n", "\n")
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_uses_notify_send_with_urgency() {
        let (program, args) = command(&alert(), None);
        assert_eq!(program, "notify-send");
        assert_eq!(&args[..4], ["-a", "goat-herdr", "-u", "critical"]);
        assert_eq!(args[4], "BLOCKED claude · ws · box");
    }
}
