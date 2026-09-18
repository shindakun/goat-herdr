//! Files in `HERDR_PLUGIN_STATE_DIR`. Every hook is its own process and two
//! can run at once, so every read-modify-write holds the lock file.

use std::collections::{HashMap, HashSet};
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

/// Telegram forum topics, keyed by the routing key a sink derives from an
/// alert. `panes` remembers which key each live pane last posted under, so a
/// pane close (which carries no workspace label or agent) can find its topic.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Topics {
    threads: HashMap<String, i64>,
    panes: HashMap<String, String>,
    /// Keys whose topic this plugin closed. Telegram lets an admin bot post
    /// into a closed topic without error, so the reopen has to be explicit.
    #[serde(default)]
    closed: HashSet<String>,
}

/// A topic to post into.
pub struct TopicHandle {
    pub thread_id: i64,
    /// This plugin closed the topic; reopen it before posting.
    pub closed: bool,
}

/// What a pane close means for its topic.
pub struct TopicRelease {
    pub key: String,
    pub thread_id: i64,
    /// No other live pane posts to this topic.
    pub last_pane: bool,
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

    /// The thread id for `key`, creating it through `create` under the lock
    /// so two hooks for the same key cannot both create a topic. Records that
    /// `pane_id` posts to this key.
    pub fn topic_thread(
        &self,
        key: &str,
        pane_id: &str,
        create: impl FnOnce() -> Result<i64, String>,
    ) -> Result<TopicHandle, String> {
        let _guard = self.lock()?;
        let path = self.dir.join("topics.json");
        let mut topics: Topics = read_json(&path);
        let thread_id = match topics.threads.get(key) {
            Some(id) => *id,
            None => {
                let id = create()?;
                topics.threads.insert(key.to_string(), id);
                id
            }
        };
        topics.panes.insert(pane_id.to_string(), key.to_string());
        write_json(&path, &topics)?;
        Ok(TopicHandle {
            thread_id,
            closed: topics.closed.contains(key),
        })
    }

    /// Records whether this plugin has the topic closed.
    pub fn topic_set_closed(&self, key: &str, closed: bool) -> Result<(), String> {
        let _guard = self.lock()?;
        let path = self.dir.join("topics.json");
        let mut topics: Topics = read_json(&path);
        if closed {
            topics.closed.insert(key.to_string());
        } else {
            topics.closed.remove(key);
        }
        write_json(&path, &topics)
    }

    /// Drops a topic mapping that Telegram no longer knows about.
    pub fn topic_forget(&self, key: &str) -> Result<(), String> {
        let _guard = self.lock()?;
        let path = self.dir.join("topics.json");
        let mut topics: Topics = read_json(&path);
        topics.threads.remove(key);
        topics.closed.remove(key);
        write_json(&path, &topics)
    }

    /// Unregisters a closed pane from its topic.
    pub fn topic_pane_closed(&self, pane_id: &str) -> Result<Option<TopicRelease>, String> {
        let _guard = self.lock()?;
        let path = self.dir.join("topics.json");
        let mut topics: Topics = read_json(&path);
        let Some(key) = topics.panes.remove(pane_id) else {
            return Ok(None);
        };
        let Some(thread_id) = topics.threads.get(&key).copied() else {
            write_json(&path, &topics)?;
            return Ok(None);
        };
        let last_pane = !topics.panes.values().any(|k| *k == key);
        write_json(&path, &topics)?;
        Ok(Some(TopicRelease {
            key,
            thread_id,
            last_pane,
        }))
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

fn read_json<T: Default + serde::de::DeserializeOwned>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let text = serde_json::to_string(value).map_err(|err| err.to_string())?;
    std::fs::write(path, text).map_err(|err| format!("write {}: {err}", path.display()))
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
    fn topics_create_once_and_release_on_last_pane() {
        let (state, dir) = temp_state("topics");
        let mut created = 0;
        let h = state
            .topic_thread("k", "p1", || {
                created += 1;
                Ok(41)
            })
            .unwrap();
        assert_eq!((h.thread_id, h.closed), (41, false));
        let h = state
            .topic_thread("k", "p2", || {
                created += 1;
                Ok(99)
            })
            .unwrap();
        assert_eq!(h.thread_id, 41);
        assert_eq!(created, 1);
        let r = state.topic_pane_closed("p1").unwrap().unwrap();
        assert_eq!((r.thread_id, r.last_pane), (41, false));
        let r = state.topic_pane_closed("p2").unwrap().unwrap();
        assert_eq!((r.key.as_str(), r.thread_id, r.last_pane), ("k", 41, true));
        assert!(state.topic_pane_closed("p2").unwrap().is_none());
        state.topic_set_closed("k", true).unwrap();
        let h = state.topic_thread("k", "p3", || Ok(7)).unwrap();
        assert_eq!((h.thread_id, h.closed), (41, true));
        state.topic_set_closed("k", false).unwrap();
        assert!(!state.topic_thread("k", "p3", || Ok(7)).unwrap().closed);
        state.topic_forget("k").unwrap();
        let h = state.topic_thread("k", "p3", || Ok(7)).unwrap();
        assert_eq!(h.thread_id, 7);
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
