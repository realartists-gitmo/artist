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

/// One key's value, the revision it was written at, and who wrote it.
///
/// The writer is what makes two machines' revisions comparable. A revision on
/// its own is a count of writes *on one machine*, so comparing yours to a
/// peer's is comparing two unrelated clocks — which silently drops whichever
/// side happens to have written less. With a writer to break ties, `(rev,
/// writer)` is a total order every participant computes the same way, and
/// merging becomes order-independent: the same set of writes converges to the
/// same state no matter what sequence they arrive in.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub value: serde_json::Value,
    pub rev: u64,
    /// Absent in state written before canvases could be shared, which is the
    /// common case on disk today. Empty sorts below any real writer, so an old
    /// entry loses a tie to a new one rather than winning it arbitrarily.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub writer: String,
}

impl Entry {
    /// How two writes to the same key are ordered, by everyone, identically.
    fn wins_over(&self, other: &Entry) -> bool {
        (self.rev, self.writer.as_str()) > (other.rev, other.writer.as_str())
    }
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

/// This machine's short name for tie-breaking.
///
/// Derived from the sharing identity so it is stable across restarts — a
/// writer id that changed per process would make a machine lose ties to its
/// own earlier writes. Falls back to a per-process value when there is no
/// identity yet, which is correct for the case that fallback covers: a machine
/// that has never shared anything has nobody to tie with.
fn writer_id() -> String {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(|| match crate::peer::identity() {
        Ok(key) => key.public().to_string().chars().take(16).collect(),
        Err(_) => format!("local-{}", std::process::id()),
    })
    .clone()
}

/// What the mutex guards: the state, and how fresh we believe it to be.
#[derive(Debug, Default)]
struct Held {
    snapshot: Snapshot,
    /// The file's mtime as of our last read or write. A different one on disk
    /// means somebody else got there since.
    seen: Option<std::time::SystemTime>,
}

/// The default ceiling on one canvas's durable state.
///
/// `state.json` lives in the user's repo and nothing about a write is
/// rate-limited: a canvas that writes state on every render grows it until the
/// disk is gone, and a canvas is model-written. Four mebibytes is far past what
/// this is for — thousands of table rows — so meeting it means a bug rather
/// than an ambitious canvas, which is why the message says so.
pub const DEFAULT_MAX_BYTES: usize = 4 * 1024 * 1024;

/// A write refused for being too large.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error(
    "this canvas's state would reach {attempted} bytes, over its {limit}-byte limit, so nothing \
     was written. That usually means state is being written in a render or a loop rather than \
     in response to something. If the canvas genuinely needs to hold more, raise it in \
     canvas.toml:\n\n  [limits]\n  state_bytes = {suggestion}"
)]
pub struct OverLimit {
    /// The ceiling in force, whether the default or the manifest's.
    pub limit: usize,
    /// What the state would have reached had the write been applied.
    pub attempted: usize,
    /// A limit that would admit this write, so the fix can be pasted.
    pub suggestion: usize,
}

/// One canvas's state, guarded and written through to disk.
#[derive(Debug)]
pub struct StateStore {
    path: PathBuf,
    /// Where the ceiling is declared. Re-read per write rather than cached at
    /// open: the model's move on hitting the limit is to raise it in
    /// `canvas.toml` and try again, and a cached limit would make that retry
    /// fail for no visible reason until the session restarted.
    manifest: PathBuf,
    /// Who this machine is, stamped on every local write so a peer can order
    /// it against their own. Short and stable rather than the full key: it is
    /// only ever compared for equality and to break ties.
    writer: String,
    held: Mutex<Held>,
}

impl StateStore {
    /// Load from disk, or start empty. A corrupt file is discarded rather than
    /// fatal: state is a convenience, and refusing to open a canvas because its
    /// last session wrote half a JSON object would be the wrong trade.
    pub fn open(canvas_root: &Path) -> Self {
        let path = canvas_root.join(STATE_FILE);
        let store = StateStore {
            path,
            manifest: canvas_root.join(crate::registry::MANIFEST_FILE),
            writer: writer_id(),
            held: Mutex::new(Held::default()),
        };
        let mut held = store.held.lock().expect("state lock poisoned");
        store.refresh(&mut held);
        drop(held);
        store
    }

    /// Take the file's word for it if the file has moved on.
    ///
    /// The in-memory copy was authoritative for the process lifetime, which is
    /// wrong in the case this whole subsystem is built around: the user editing
    /// their own project. `state.json` is a file in their repo — they may edit
    /// it, revert it, or check out a branch where it differs — and the next
    /// write from the model would silently put it all back.
    fn refresh(&self, held: &mut Held) {
        let mtime = std::fs::metadata(&self.path)
            .and_then(|meta| meta.modified())
            .ok();
        if mtime == held.seen && held.seen.is_some() {
            return;
        }
        match std::fs::read_to_string(&self.path)
            .ok()
            .and_then(|source| serde_json::from_str::<Snapshot>(&source).ok())
        {
            Some(disk) => {
                // The revision counter only ever moves forward. A page holding
                // rev 9 must not be handed a "new" rev 4 from a file written by
                // an older session, or it would ignore the update as stale.
                let rev = disk.rev.max(held.snapshot.rev);
                held.snapshot = Snapshot { rev, ..disk };
                held.seen = mtime;
            }
            // Unreadable or unparseable: keep what we have. Losing the user's
            // state because their editor saved a half-written file would be a
            // far worse outcome than briefly ignoring the file.
            None if mtime.is_none() => held.seen = None,
            None => {}
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        held.snapshot.clone()
    }

    pub fn get(&self, key: &str) -> Option<serde_json::Value> {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        held.snapshot
            .entries
            .get(key)
            .map(|entry| entry.value.clone())
    }

    /// The ceiling in force: the manifest's if it raised one, else the default.
    ///
    /// Read from disk on every write. A canvas.toml the model just edited has
    /// to count immediately, because raising the limit is the documented fix
    /// for hitting it.
    fn limit(&self) -> usize {
        std::fs::read_to_string(&self.manifest)
            .ok()
            .and_then(|source| crate::manifest::Manifest::parse(&source).ok())
            .and_then(|manifest| manifest.limits.state_bytes)
            .unwrap_or(DEFAULT_MAX_BYTES)
    }

    /// Write one key. Returns the entry as stored, carrying the new revision.
    pub fn set(&self, key: &str, value: serde_json::Value) -> Result<Entry, OverLimit> {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        let restore = held.snapshot.clone();
        held.snapshot.rev += 1;
        let entry = Entry {
            value,
            rev: held.snapshot.rev,
            writer: self.writer.clone(),
        };
        held.snapshot.entries.insert(key.to_owned(), entry.clone());
        self.commit(&mut held, restore)?;
        Ok(entry)
    }

    /// Merge several keys under a single revision bump, so a page applying the
    /// change sees one consistent step rather than a partially-updated view.
    pub fn merge(
        &self,
        values: BTreeMap<String, serde_json::Value>,
    ) -> Result<Snapshot, OverLimit> {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        let restore = held.snapshot.clone();
        held.snapshot.rev += 1;
        let rev = held.snapshot.rev;
        for (key, value) in values {
            held.snapshot.entries.insert(
                key,
                Entry {
                    value,
                    rev,
                    writer: self.writer.clone(),
                },
            );
        }
        self.commit(&mut held, restore)?;
        Ok(held.snapshot.clone())
    }

    /// Take in state written somewhere else.
    ///
    /// This is the one that has to be right, because it is the only place two
    /// independent histories meet. It compares per key rather than per
    /// document: a document revision counts writes on one machine, so gating a
    /// peer's whole update on "is your counter ahead of mine" silently drops
    /// every edit from whichever side has written less — which is not a merge,
    /// it is a coin toss weighted by activity.
    ///
    /// Per key with `(rev, writer)` as the order, the result does not depend on
    /// arrival sequence, on which side initiated, or on how many times either
    /// has written. Two peers who exchange the same edits in any order end up
    /// holding the same state.
    ///
    /// A losing write is dropped rather than merged, so this is last-writer-
    /// wins per key and not a merge of concurrent edits *to the same key*.
    /// Different keys never conflict, which is what makes it enough here: a
    /// canvas's keys are owned by the parts of the UI that write them.
    pub fn absorb(&self, incoming: BTreeMap<String, Entry>) -> Result<Snapshot, OverLimit> {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        let restore = held.snapshot.clone();

        let mut changed = false;
        for (key, entry) in incoming {
            match held.snapshot.entries.get(&key) {
                Some(mine) if !entry.wins_over(mine) => {}
                _ => {
                    // The document revision tracks the highest write this
                    // canvas has seen from anywhere, so a later local write
                    // cannot be handed a revision a peer has already used.
                    held.snapshot.rev = held.snapshot.rev.max(entry.rev);
                    held.snapshot.entries.insert(key, entry);
                    changed = true;
                }
            }
        }
        if !changed {
            return Ok(held.snapshot.clone());
        }
        self.commit(&mut held, restore)?;
        Ok(held.snapshot.clone())
    }

    /// Persist a mutation, or undo it if it would breach the ceiling.
    ///
    /// The check is on the encoded whole rather than on the incoming values,
    /// because what matters is the size of the file this produces — a canvas
    /// creeping over by one key at a time would pass every per-write check.
    /// `restore` is the snapshot from before the mutation: a refused write
    /// leaves neither disk nor memory touched, so the caller's next read sees
    /// exactly what it saw before.
    fn commit(&self, held: &mut Held, restore: Snapshot) -> Result<(), OverLimit> {
        let limit = self.limit();
        let attempted = serde_json::to_vec_pretty(&held.snapshot)
            .map(|encoded| encoded.len())
            .unwrap_or(0);
        if attempted > limit {
            *held = Held {
                snapshot: restore,
                seen: held.seen,
            };
            return Err(OverLimit {
                limit,
                attempted,
                // Rounded up to the next mebibyte, so the suggestion leaves
                // room to work in rather than admitting exactly this one write.
                suggestion: attempted.div_ceil(1024 * 1024) * 1024 * 1024,
            });
        }
        self.persist(held);
        Ok(())
    }

    pub fn remove(&self, key: &str) -> bool {
        let mut held = self.held.lock().expect("state lock poisoned");
        self.refresh(&mut held);
        let removed = held.snapshot.entries.remove(key).is_some();
        if removed {
            held.snapshot.rev += 1;
            self.persist(&mut held);
        }
        removed
    }

    pub fn clear(&self) {
        let mut held = self.held.lock().expect("state lock poisoned");
        held.snapshot.entries.clear();
        held.snapshot.rev += 1;
        self.persist(&mut held);
    }

    /// Write through to disk.
    ///
    /// Temp file plus rename, so a crash mid-write leaves the previous state
    /// intact instead of a truncated file. A failed write is dropped rather
    /// than propagated: losing durability must not fail the user's click.
    fn persist(&self, held: &mut Held) {
        let Ok(encoded) = serde_json::to_vec_pretty(&held.snapshot) else {
            return;
        };
        let temporary = self.path.with_extension("json.tmp");
        if std::fs::write(&temporary, &encoded).is_ok()
            && std::fs::rename(&temporary, &self.path).is_ok()
        {
            // Record what we just wrote, so the next call does not mistake our
            // own write for somebody else's and re-read it needlessly.
            held.seen = std::fs::metadata(&self.path)
                .and_then(|meta| meta.modified())
                .ok();
        }
    }
}

#[cfg(test)]
mod merging {
    use super::*;

    fn entry(value: u64, rev: u64, writer: &str) -> Entry {
        Entry {
            value: serde_json::json!(value),
            rev,
            writer: writer.to_owned(),
        }
    }

    fn store(at: &Path) -> StateStore {
        StateStore::open(at)
    }

    /// The bug this exists for: the old code compared the *document* revision
    /// of one machine against another's and dropped the whole update if it was
    /// behind. Two counters over two histories, so the side that had written
    /// less silently lost everything it sent.
    #[test]
    fn a_peer_that_has_written_less_does_not_lose_its_writes() {
        let canvas = tempfile::tempdir().expect("tempdir");
        let mine = store(canvas.path());

        // This machine is busy: ten writes, so its document revision is ten.
        for index in 0..10 {
            mine.set("mine", serde_json::json!(index)).expect("write");
        }
        assert_eq!(mine.snapshot().rev, 10);

        // A peer sends a key it wrote on its second-ever write.
        mine.absorb([("theirs".to_owned(), entry(7, 2, "peer"))].into())
            .expect("absorb");

        assert_eq!(
            mine.snapshot().plain().get("theirs"),
            Some(&serde_json::json!(7)),
            "a quieter peer's write was dropped"
        );
    }

    /// Order-independence is the property that makes this a merge rather than
    /// a race: the same writes, applied in any sequence, converge.
    #[test]
    fn the_same_writes_converge_whatever_order_they_arrive_in() {
        let one = tempfile::tempdir().expect("tempdir");
        let two = tempfile::tempdir().expect("tempdir");

        let older = entry(1, 4, "alice");
        let newer = entry(2, 9, "bob");

        let forwards = store(one.path());
        forwards
            .absorb([("k".to_owned(), older.clone())].into())
            .expect("absorb");
        forwards
            .absorb([("k".to_owned(), newer.clone())].into())
            .expect("absorb");

        let backwards = store(two.path());
        backwards
            .absorb([("k".to_owned(), newer)].into())
            .expect("absorb");
        backwards
            .absorb([("k".to_owned(), older)].into())
            .expect("absorb");

        assert_eq!(forwards.snapshot().entries, backwards.snapshot().entries);
        assert_eq!(
            forwards.snapshot().plain().get("k"),
            Some(&serde_json::json!(2)),
            "the later write should win regardless of arrival order"
        );
    }

    /// Equal revisions on two machines are not a paradox, they are the normal
    /// case — both wrote once. The writer breaks it, the same way on both.
    #[test]
    fn an_equal_revision_is_broken_by_the_writer_not_by_luck() {
        let one = tempfile::tempdir().expect("tempdir");
        let two = tempfile::tempdir().expect("tempdir");

        let alice = entry(1, 3, "alice");
        let bob = entry(2, 3, "bob");

        let a = store(one.path());
        a.absorb([("k".to_owned(), alice.clone())].into())
            .expect("absorb");
        a.absorb([("k".to_owned(), bob.clone())].into())
            .expect("absorb");

        let b = store(two.path());
        b.absorb([("k".to_owned(), bob)].into()).expect("absorb");
        b.absorb([("k".to_owned(), alice)].into()).expect("absorb");

        assert_eq!(
            a.snapshot().plain().get("k"),
            b.snapshot().plain().get("k"),
            "a tie resolved differently on two machines is a permanent split"
        );
    }

    /// State written before canvases could be shared has no writer. It must
    /// lose a tie rather than win one arbitrarily, and it must not be dropped.
    #[test]
    fn state_from_before_sharing_still_merges() {
        let canvas = tempfile::tempdir().expect("tempdir");
        let mine = store(canvas.path());

        let legacy = Entry {
            value: serde_json::json!("old"),
            rev: 3,
            writer: String::new(),
        };
        mine.absorb([("k".to_owned(), legacy)].into())
            .expect("absorb");
        assert_eq!(
            mine.snapshot().plain().get("k"),
            Some(&serde_json::json!("old"))
        );

        mine.absorb([("k".to_owned(), entry(5, 3, "peer"))].into())
            .expect("absorb");
        assert_eq!(
            mine.snapshot().plain().get("k"),
            Some(&serde_json::json!(5)),
            "an unwritered entry should lose a tie, not win it"
        );
    }

    /// A local write after absorbing must not be handed a revision a peer has
    /// already used, or it would lose to the write it came after.
    #[test]
    fn a_local_write_after_a_peers_takes_a_higher_revision() {
        let canvas = tempfile::tempdir().expect("tempdir");
        let mine = store(canvas.path());

        mine.absorb([("k".to_owned(), entry(1, 50, "peer"))].into())
            .expect("absorb");
        let written = mine.set("k", serde_json::json!("mine")).expect("write");

        assert!(
            written.rev > 50,
            "a local write took revision {} after a peer's 50",
            written.rev
        );
        assert_eq!(
            mine.snapshot().plain().get("k"),
            Some(&serde_json::json!("mine"))
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A temp directory that removes itself when the test ends.
    ///
    /// Derefs to `Path` so a caller can keep using it as one. The owning bind
    /// is what matters: hold it for as long as the thing under test needs the
    /// directory, because dropping it takes the directory with it.
    struct Temp(tempfile::TempDir);

    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            self.0.path()
        }
    }

    /// Named after the test, so a failure leaves an identifiable directory
    /// behind while it is being debugged — but only until the process ends.
    ///
    /// This used to build a path from the pid and clean it at the *start* of
    /// the next run, which meant every run left its directories behind. That
    /// was invisible while `/tmp` was a tmpfs that emptied on reboot; on
    /// disk-backed storage it accumulates forever.
    fn temp(name: &str) -> Temp {
        Temp(
            tempfile::Builder::new()
                .prefix(&format!("artist-canvas-state-{name}-"))
                .tempdir()
                .expect("temp dir"),
        )
    }

    /// A refused write must leave the canvas exactly as it was. Truncating, or
    /// applying half of a merge, would give the model a state it never wrote
    /// and cannot reason about — worse than the write plainly failing.
    #[test]
    fn a_write_over_the_limit_changes_nothing() {
        let root = temp("limit");
        std::fs::write(
            root.join(crate::registry::MANIFEST_FILE),
            "[limits]\nstate_bytes = 2048\n",
        )
        .expect("manifest");
        let store = StateStore::open(&root);
        store.set("kept", json!("small")).expect("within the limit");
        let before = store.snapshot();

        let refused = store
            .set("huge", json!("x".repeat(4096)))
            .expect_err("a 4KiB value must not fit under a 2KiB ceiling");
        assert_eq!(refused.limit, 2048);
        assert!(refused.attempted > 2048, "{refused:?}");

        // Memory, disk and revision all unmoved: a refusal is not a write.
        assert_eq!(store.snapshot(), before);
        assert_eq!(StateStore::open(&root).snapshot(), before);
    }

    /// The override is the whole reason the default is safe to set low: a
    /// canvas that genuinely holds a lot declares it and is believed.
    #[test]
    fn a_manifest_can_raise_the_ceiling() {
        let root = temp("raised");
        let manifest = root.join(crate::registry::MANIFEST_FILE);
        std::fs::write(&manifest, "[limits]\nstate_bytes = 2048\n").expect("manifest");
        let store = StateStore::open(&root);
        let big = json!("x".repeat(4096));

        assert!(store.set("big", big.clone()).is_err(), "2KiB should refuse");

        // Re-read per write, so the canvas the model just edited counts now
        // rather than after a restart.
        std::fs::write(&manifest, "[limits]\nstate_bytes = 65536\n").expect("raise");
        assert!(
            store.set("big", big).is_ok(),
            "the raised ceiling should admit it"
        );
        assert!(store.snapshot().entries.contains_key("big"));
    }

    /// Nothing declared means the default, not "no limit".
    #[test]
    fn a_canvas_without_a_manifest_still_has_a_ceiling() {
        let root = temp("default-limit");
        let store = StateStore::open(&root);
        assert_eq!(store.limit(), DEFAULT_MAX_BYTES);
    }

    #[test]
    fn writes_are_revision_stamped_so_a_client_can_order_them() {
        let root = temp("rev");
        let store = StateStore::open(&root);

        let first = store.set("rows", json!([1, 2])).expect("within the limit");
        let second = store.set("selected", json!(1)).expect("within the limit");

        assert_eq!(first.rev, 1);
        assert_eq!(second.rev, 2);
        assert_eq!(store.snapshot().rev, 2);
    }

    /// `state.json` is a file in the user's own repo. They may edit it, revert
    /// it, or check out a branch where it differs — and the next write from the
    /// model was silently putting all of that back.
    #[test]
    fn an_edit_made_outside_artist_is_not_clobbered() {
        let root = temp("external");
        let store = StateStore::open(&root);
        store
            .set("mine", json!("original"))
            .expect("within the limit");

        // Somebody else writes the file: the user, their editor, another
        // process. The mtime has to differ for the store to notice, and a
        // filesystem with second granularity would otherwise make this flaky.
        let path = root.join(STATE_FILE);
        std::fs::write(
            &path,
            serde_json::to_string(&Snapshot {
                rev: 1,
                entries: BTreeMap::from([
                    (
                        "mine".to_owned(),
                        Entry {
                            value: json!("theirs"),
                            rev: 1,
                            writer: String::new(),
                        },
                    ),
                    (
                        "added".to_owned(),
                        Entry {
                            value: json!(7),
                            rev: 1,
                            writer: String::new(),
                        },
                    ),
                ]),
            })
            .expect("encode"),
        )
        .expect("write");
        filetime_bump(&path);

        assert_eq!(
            store.get("added"),
            Some(json!(7)),
            "the outside write was not seen"
        );
        assert_eq!(store.get("mine"), Some(json!("theirs")));

        // And a write from this side merges into it rather than over it.
        store
            .set("mine", json!("updated"))
            .expect("within the limit");
        let snapshot = store.snapshot();
        assert_eq!(
            snapshot.entries["added"].value,
            json!(7),
            "the outside key was lost"
        );
        assert_eq!(snapshot.entries["mine"].value, json!("updated"));
    }

    /// A page holding rev 9 must not be handed a "new" rev 4 from an older
    /// file: it would read the update as stale and ignore it.
    #[test]
    fn the_revision_counter_only_moves_forward() {
        let root = temp("rewind");
        let store = StateStore::open(&root);
        for index in 0..5 {
            store.set("k", json!(index)).expect("within the limit");
        }
        assert_eq!(store.snapshot().rev, 5);

        let path = root.join(STATE_FILE);
        std::fs::write(
            &path,
            serde_json::to_string(&Snapshot {
                rev: 1,
                entries: BTreeMap::from([(
                    "k".to_owned(),
                    Entry {
                        value: json!("old"),
                        rev: 1,
                        writer: String::new(),
                    },
                )]),
            })
            .expect("encode"),
        )
        .expect("write");
        filetime_bump(&path);

        assert_eq!(
            store.get("k"),
            Some(json!("old")),
            "the file should win on content"
        );
        assert!(store.snapshot().rev >= 5, "the revision went backwards");
    }

    /// A half-written file — an editor saving, a crash mid-write — must not
    /// wipe state that is currently in memory and correct.
    #[test]
    fn an_unparseable_file_is_ignored_rather_than_believed() {
        let root = temp("corrupt");
        let store = StateStore::open(&root);
        store.set("keep", json!("me")).expect("within the limit");

        let path = root.join(STATE_FILE);
        std::fs::write(&path, "{\"rev\": 3, \"entr").expect("write");
        filetime_bump(&path);

        assert_eq!(store.get("keep"), Some(json!("me")));
    }

    /// Make the file look newer than whatever the store last recorded, on
    /// filesystems whose mtime granularity is coarser than this test is fast.
    fn filetime_bump(path: &Path) {
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
        let _ = std::fs::File::open(path).map(|file| file.set_modified(later));
    }

    /// A page applying a merge must never see half of it.
    #[test]
    fn a_merge_lands_under_one_revision() {
        let root = temp("merge");
        let store = StateStore::open(&root);
        store
            .set("untouched", json!(true))
            .expect("within the limit");

        let snapshot = store
            .merge(BTreeMap::from([
                ("a".to_owned(), json!(1)),
                ("b".to_owned(), json!(2)),
            ]))
            .expect("within the limit");

        assert_eq!(snapshot.entries["a"].rev, snapshot.entries["b"].rev);
        assert_eq!(snapshot.rev, 2);
        assert_eq!(snapshot.entries["untouched"].rev, 1);
    }

    #[test]
    fn state_survives_reopening_the_canvas() {
        let root = temp("durable");
        {
            let store = StateStore::open(&root);
            store.set("count", json!(41)).expect("within the limit");
            store.set("count", json!(42)).expect("within the limit");
        }

        let reopened = StateStore::open(&root);
        assert_eq!(reopened.get("count"), Some(json!(42)));
        // The revision continues rather than restarting, so a still-open page
        // cannot mistake reloaded state for something it already applied.
        assert_eq!(reopened.snapshot().rev, 2);
        assert_eq!(
            reopened
                .set("count", json!(43))
                .expect("within the limit")
                .rev,
            3
        );
    }

    /// Half a JSON object from an interrupted write must not make the canvas
    /// unopenable — state is a convenience, not the canvas itself.
    #[test]
    fn a_corrupt_state_file_is_discarded_rather_than_fatal() {
        let root = temp("corrupt");
        std::fs::write(root.join(STATE_FILE), "{\"entries\": {\"a\":").expect("write");

        let store = StateStore::open(&root);
        assert_eq!(store.snapshot(), Snapshot::default());
        assert_eq!(store.set("a", json!(1)).expect("within the limit").rev, 1);
    }

    #[test]
    fn removing_a_missing_key_does_not_bump_the_revision() {
        let root = temp("remove");
        let store = StateStore::open(&root);
        store.set("a", json!(1)).expect("within the limit");

        assert!(!store.remove("nope"));
        assert_eq!(store.snapshot().rev, 1);
        assert!(store.remove("a"));
        assert_eq!(store.snapshot().rev, 2);
    }
}
