//! Shared state between the user and the agent.
//!
//! A canvas is not a render target — both sides write to it. The user clicks
//! and the model reads what they chose; the model pushes rows and the page
//! re-renders. This module is that shared surface.
//!
//! State is revision-stamped so a client can tell whether an update it receives
//! is newer than what it already applied, and durable so reopening a canvas
//! tomorrow finds what was there yesterday.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};

pub const STATE_FILE: &str = "state.json";

/// One key's value plus the revision it was written at.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub value: serde_json::Value,
    pub rev: u64,
}

/// The durable form. A map rather than a bare value so two parts of a canvas
/// can own separate keys without coordinating writes.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub rev: u64,
    #[serde(default)]
    pub entries: BTreeMap<String, Entry>,
}

impl Snapshot {
    pub fn plain(&self) -> BTreeMap<String, serde_json::Value> {
        self.entries
            .iter()
            .map(|(key, entry)| (key.clone(), entry.value.clone()))
            .collect()
    }
}

/// One canvas's state, guarded and written through to disk.
#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
    snapshot: Mutex<Snapshot>,
}

impl StateStore {
    /// Load from disk, or start empty. A corrupt file is discarded rather than
    /// fatal: state is a convenience, and refusing to open a canvas because its
    /// last session wrote half a JSON object would be the wrong trade.
    pub fn open(canvas_root: &Path) -> Self {
        let path = canvas_root.join(STATE_FILE);
        let snapshot = std::fs::read_to_string(&path)
            .ok()
            .and_then(|source| serde_json::from_str::<Snapshot>(&source).ok())
            .unwrap_or_default();
        StateStore {
            path,
            snapshot: Mutex::new(snapshot),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.snapshot.lock().expect("state lock poisoned").clone()
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        self.snapshot
            .lock()
            .expect("state lock poisoned")
            .entries
            .get(key)
            .map(|entry| entry.value.clone())
    }

    /// Write one key. Returns the entry as stored, carrying the new revision.
    pub fn set(&self, key: &str, value: serde_json::Value) -> Entry {
        let mut snapshot = self.snapshot.lock().expect("state lock poisoned");
        snapshot.rev += 1;
        let entry = Entry {
            value,
            rev: snapshot.rev,
        };
        snapshot.entries.insert(key.to_owned(), entry.clone());
        self.persist(&snapshot);
        entry
    }

    /// Merge several keys under a single revision bump, so a page applying the
    /// change sees one consistent step rather than a partially-updated view.
    pub fn merge(&self, values: BTreeMap<String, serde_json::Value>) -> Snapshot {
        let mut snapshot = self.snapshot.lock().expect("state lock poisoned");
        snapshot.rev += 1;
        let rev = snapshot.rev;
        for (key, value) in values {
            snapshot.entries.insert(key, Entry { value, rev });
        }
        self.persist(&snapshot);
        snapshot.clone()
    }

    pub fn remove(&self, key: &str) -> bool {
        let mut snapshot = self.snapshot.lock().expect("state lock poisoned");
        let removed = snapshot.entries.remove(key).is_some();
        if removed {
            snapshot.rev += 1;
            self.persist(&snapshot);
        }
        removed
    }

    pub fn clear(&self) {
        let mut snapshot = self.snapshot.lock().expect("state lock poisoned");
        snapshot.entries.clear();
        snapshot.rev += 1;
        self.persist(&snapshot);
    }

    /// Write through to disk.
    ///
    /// Temp file plus rename, so a crash mid-write leaves the previous state
    /// intact instead of a truncated file. A failed write is dropped rather
    /// than propagated: losing durability must not fail the user's click.
    fn persist(&self, snapshot: &Snapshot) {
        let Ok(encoded) = serde_json::to_vec_pretty(snapshot) else {
            return;
        };
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, &encoded).is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "artist-canvas-state-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("temp dir");
        base
    }

    #[test]
    fn writes_are_revision_stamped_so_a_client_can_order_them() {
        let store = StateStore::open(&temp("rev"));

        let first = store.set("rows", json!([1, 2]));
        let second = store.set("selected", json!(1));

        assert_eq!(first.rev, 1);
        assert_eq!(second.rev, 2);
        assert_eq!(store.snapshot().rev, 2);
    }

    /// A page applying a merge must never see half of it.
    #[test]
    fn a_merge_lands_under_one_revision() {
        let store = StateStore::open(&temp("merge"));
        store.set("untouched", json!(true));

        let snapshot = store.merge(BTreeMap::from([
            ("a".to_owned(), json!(1)),
            ("b".to_owned(), json!(2)),
        ]));

        assert_eq!(snapshot.entries["a"].rev, snapshot.entries["b"].rev);
        assert_eq!(snapshot.rev, 2);
        assert_eq!(snapshot.entries["untouched"].rev, 1);
    }

    #[test]
    fn state_survives_reopening_the_canvas() {
        let root = temp("durable");
        {
            let store = StateStore::open(&root);
            store.set("count", json!(41));
            store.set("count", json!(42));
        }

        let reopened = StateStore::open(&root);
        assert_eq!(reopened.get("count"), Some(json!(42)));
        // The revision continues rather than restarting, so a still-open page
        // cannot mistake reloaded state for something it already applied.
        assert_eq!(reopened.snapshot().rev, 2);
        assert_eq!(reopened.set("count", json!(43)).rev, 3);
    }

    /// Half a JSON object from an interrupted write must not make the canvas
    /// unopenable — state is a convenience, not the canvas itself.
    #[test]
    fn a_corrupt_state_file_is_discarded_rather_than_fatal() {
        let root = temp("corrupt");
        std::fs::write(root.join(STATE_FILE), "{\"entries\": {\"a\":").expect("write");

        let store = StateStore::open(&root);
        assert_eq!(store.snapshot(), Snapshot::default());
        assert_eq!(store.set("a", json!(1)).rev, 1);
    }

    #[test]
    fn removing_a_missing_key_does_not_bump_the_revision() {
        let store = StateStore::open(&temp("remove"));
        store.set("a", json!(1));

        assert!(!store.remove("nope"));
        assert_eq!(store.snapshot().rev, 1);
        assert!(store.remove("a"));
        assert_eq!(store.snapshot().rev, 2);
    }
}
