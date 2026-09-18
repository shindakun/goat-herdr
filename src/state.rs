//! Files in `HERDR_PLUGIN_STATE_DIR`. Every hook is its own process and two
//! can run at once, so every read-modify-write holds the lock file.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub struct State {
    dir: PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Debounce {
    /// pane id -> (status, unix seconds of the last alert sent)
    panes: HashMap<String, (String, u64)>,
}

impl State {
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// True when the toggle action has paused alerts.
    pub fn paused(&self) -> bool {
        self.dir.join("paused").exists()
    }

    /// Flips the pause flag and returns the new value.
    pub fn toggle_paused(&self) -> Result<bool, String> {
        let flag = self.dir.join("paused");
        if flag.exists() {
            std::fs::remove_file(&flag)
                .map_err(|err| format!("remove {}: {err}", flag.display()))?;
            Ok(false)
        } else {
            std::fs::write(&flag, b"").map_err(|err| format!("write {}: {err}", flag.display()))?;
            Ok(true)
        }
    }

    /// Records this pane and status. Returns false when the same pair was
    /// recorded inside `window_secs`, which means the caller should drop the
    /// alert.
    pub fn debounce(&self, pane_id: &str, status: &str, window_secs: u64) -> Result<bool, String> {
        let _guard = self.lock()?;
        let path = self.dir.join("debounce.json");
        let mut table: Debounce = match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Debounce::default(),
        };
        let now = unix_now();
        if let Some((last_status, at)) = table.panes.get(pane_id) {
            if last_status == status && now.saturating_sub(*at) < window_secs {
                return Ok(false);
            }
        }
        table
            .panes
            .insert(pane_id.to_string(), (status.to_string(), now));
        let text = serde_json::to_string(&table).map_err(|err| err.to_string())?;
        std::fs::write(&path, text).map_err(|err| format!("write {}: {err}", path.display()))?;
        Ok(true)
    }

    /// Drops the pane from the debounce table when it closes.
    pub fn forget(&self, pane_id: &str) -> Result<(), String> {
        let _guard = self.lock()?;
        let path = self.dir.join("debounce.json");
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Ok(());
        };
        let mut table: Debounce = serde_json::from_str(&text).unwrap_or_default();
        table.panes.remove(pane_id);
        let text = serde_json::to_string(&table).map_err(|err| err.to_string())?;
        std::fs::write(&path, text).map_err(|err| format!("write {}: {err}", path.display()))
    }

    fn lock(&self) -> Result<File, String> {
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| format!("create {}: {err}", self.dir.display()))?;
        let path = self.dir.join("lock");
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|err| format!("open {}: {err}", path.display()))?;
        // The advisory lock releases when `file` drops.
        file.lock()
            .map_err(|err| format!("lock {}: {err}", path.display()))?;
        Ok(file)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_state(name: &str) -> (State, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "goat-herdr-test-{name}-{}-{}",
            std::process::id(),
            unix_now()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        (State::new(&dir), dir)
    }

    #[test]
    fn repeat_inside_window_is_dropped() {
        let (state, dir) = temp_state("debounce");
        assert!(state.debounce("p1", "blocked", 5).unwrap());
        assert!(!state.debounce("p1", "blocked", 5).unwrap());
        assert!(state.debounce("p1", "done", 5).unwrap());
        assert!(state.debounce("p2", "blocked", 5).unwrap());
        state.forget("p1").unwrap();
        assert!(state.debounce("p1", "done", 5).unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn toggle_flips_pause() {
        let (state, dir) = temp_state("toggle");
        assert!(!state.paused());
        assert!(state.toggle_paused().unwrap());
        assert!(state.paused());
        assert!(!state.toggle_paused().unwrap());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
