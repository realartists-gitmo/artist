use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const MNEMONIC_WORDS: &str = include_str!("mnemonic_words.txt");
const BINDING_SEPARATOR: char = '\u{1f}';

pub(crate) fn pack_binding(full_hash: &str) -> String {
    full_hash.to_string()
}

pub(crate) fn binding_full(value: &str) -> &str {
    // The guard-prefix half of the binding was removed; `split_once` keeps
    // tolerating any legacy value that still carries a separator.
    value
        .split_once(BINDING_SEPARATOR)
        .map_or(value, |(full_hash, _)| full_hash)
}

fn words() -> &'static [&'static str] {
    static WORDS: OnceLock<Vec<&'static str>> = OnceLock::new();
    WORDS
        .get_or_init(|| {
            MNEMONIC_WORDS
                .lines()
                .filter(|word| !word.is_empty())
                .collect()
        })
        .as_slice()
}

fn word_set() -> &'static HashSet<&'static str> {
    static WORD_SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORD_SET.get_or_init(|| words().iter().copied().collect())
}

fn is_mnemonic_handle_in_set(handle: &str, word_set: &HashSet<&str>) -> bool {
    let mut parts = handle.split(' ');
    let Some(first) = parts.next() else {
        return false;
    };
    if !word_set.contains(first) {
        return false;
    }
    match (parts.next(), parts.next()) {
        (None, None) => true,
        (Some(second), None) => word_set.contains(second),
        _ => false,
    }
}

fn prefer_handle(candidate: &str, current: &str) -> bool {
    candidate < current
}

const PRIMARY_CURSOR_KEY: &str = "__hashline_internal_primary_cursor__";
const SECONDARY_CURSOR_KEY: &str = "__hashline_internal_secondary_cursor__";

/// One path's anchor state: the handles issued for it, plus where each
/// namespace's allocator last stopped.
///
/// The persisted form is a flat `handle -> value` map with the two cursors
/// smuggled in under reserved keys. That shape is fixed — it is what is already
/// in every `anchor_states.prefixes_json` column, and the store has no schema
/// migration — so it survives, but only at the serialization boundary in
/// [`to_flat`](Self::to_flat) / [`from_flat`](Self::from_flat).
///
/// In memory the cursors are fields. Previously every consumer had to remember
/// that two entries in a map of anchors were not anchors, and guard with
/// `is_mnemonic_handle_in_set` before trusting a key. Anything that forgot got
/// a cursor parsed as a mnemonic; the cross-file allocator work in this module
/// nearly did exactly that.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PathAnchors {
    /// Model-facing handle -> packed hidden hash binding.
    pub bindings: HashMap<String, String>,
    pub primary_cursor: Option<usize>,
    pub secondary_cursor: Option<usize>,
}

impl PathAnchors {
    /// Read the persisted flat map, separating the reserved cursor keys from
    /// real bindings.
    pub fn from_flat(flat: HashMap<String, String>) -> Self {
        let mut bindings = flat;
        let primary = bindings
            .remove(PRIMARY_CURSOR_KEY)
            .and_then(|v| v.parse::<usize>().ok());
        let secondary = bindings
            .remove(SECONDARY_CURSOR_KEY)
            .and_then(|v| v.parse::<usize>().ok());
        Self {
            bindings,
            primary_cursor: primary,
            secondary_cursor: secondary,
        }
    }

    /// Rebuild the persisted flat map, re-inserting the reserved keys.
    pub fn to_flat(&self) -> HashMap<String, String> {
        let mut flat = self.bindings.clone();
        if let Some(cursor) = self.primary_cursor {
            flat.insert(PRIMARY_CURSOR_KEY.to_owned(), cursor.to_string());
        }
        if let Some(cursor) = self.secondary_cursor {
            flat.insert(SECONDARY_CURSOR_KEY.to_owned(), cursor.to_string());
        }
        flat
    }

    /// Highest slot already in use in a namespace, used to recover a cursor
    /// that was never persisted.
    fn max_slot(&self, primary: bool) -> Option<usize> {
        self.bindings
            .keys()
            .filter_map(|handle| handle_slot(handle))
            .filter_map(|(is_primary, index)| (is_primary == primary).then_some(index))
            .max()
    }

    fn cursor(&self, primary: bool, capacity: usize) -> Option<usize> {
        let stored = if primary {
            self.primary_cursor
        } else {
            self.secondary_cursor
        };
        stored
            .filter(|index| *index < capacity)
            .or_else(|| self.max_slot(primary))
    }
}

fn word_indices() -> &'static HashMap<&'static str, usize> {
    static WORD_INDICES: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    WORD_INDICES.get_or_init(|| {
        words()
            .iter()
            .copied()
            .enumerate()
            .map(|(index, word)| (word, index))
            .collect()
    })
}

/// What the allocator knows about slot usage *outside* the table being
/// reconciled.
///
/// Without this, every file starts its deck at slot 0, so any file shorter than
/// the word list draws the same prefix: three fresh files all get
/// `like, time, people, good, know, think`. That is not merely untidy. An
/// anchor the model saw in one file resolves *successfully* against another,
/// because it is legitimately live in both — a wrong edit with no staleness
/// error to catch it.
///
/// The fields are scoped to match the preference order in [`rank_slots`], and
/// all clocks are from one monotonic counter: a larger value means more
/// recently freed, therefore worse.
#[derive(Debug, Default, Clone)]
pub struct SlotPreference {
    /// Slot -> when it was last freed *in this table*.
    pub freed_here: HashMap<usize, u64>,
    /// Slots currently assigned in some other table. Sorted, for binary search.
    pub live_elsewhere: Vec<usize>,
    /// Slot -> when it was last freed in some other table.
    pub freed_elsewhere: HashMap<usize, u64>,
}

impl SlotPreference {
    /// No cross-table knowledge: reproduces the plain circular scan.
    pub fn none() -> Self {
        Self::default()
    }

    /// Whether there is anything here worth ranking on. An empty preference set
    /// must fall through to the original cursor scan rather than re-ordering by
    /// slot index, which would hand every caller slot 0 first.
    fn is_informative(&self) -> bool {
        !self.freed_here.is_empty()
            || !self.live_elsewhere.is_empty()
            || !self.freed_elsewhere.is_empty()
    }

    fn distance_to_live_elsewhere(&self, slot: usize) -> usize {
        if self.live_elsewhere.is_empty() {
            return usize::MAX;
        }
        match self.live_elsewhere.binary_search(&slot) {
            Ok(_) => 0,
            Err(position) => {
                let after = self
                    .live_elsewhere
                    .get(position)
                    .map(|s| s - slot)
                    .unwrap_or(usize::MAX);
                let before = position
                    .checked_sub(1)
                    .and_then(|i| self.live_elsewhere.get(i))
                    .map(|s| slot - s)
                    .unwrap_or(usize::MAX);
                after.min(before)
            }
        }
    }
}

/// Order free slots best-first under the preference hierarchy.
///
/// Strictly lexicographic: each key is only consulted to break ties in the one
/// before it. Smaller is better throughout, so the whole thing is one ascending
/// sort rather than a chain of comparisons.
///
/// 1. is enforced by the caller, which only offers slots free in this table.
/// 2. is enforced by namespace: one-word handles are exhausted before two-word
///    ones are minted, and within the one-word namespace every candidate costs
///    the same, so it does not discriminate here.
/// 3. never used in this table beats used-then-freed, oldest freed first.
/// 4. not live in another table beats live there, then furthest from wherever
///    other tables are currently allocating.
/// 5. never freed elsewhere beats freed elsewhere, oldest first.
///
/// Cost is one sort per reconcile, not per assignment: the inputs are fixed for
/// the duration of a call, so ranking once and drawing from the front gives the
/// same answer as re-ranking after every pick.
fn rank_slots(candidates: Vec<usize>, prefs: &SlotPreference) -> Vec<usize> {
    let mut keyed: Vec<(u64, u64, u64, u64, usize)> = candidates
        .into_iter()
        .map(|slot| {
            // 3: 0 sorts first and means "never used here".
            let here = prefs
                .freed_here
                .get(&slot)
                .map(|clock| clock + 1)
                .unwrap_or(0);
            // 4a: live elsewhere is strictly worse than not.
            let live = u64::from(prefs.live_elsewhere.binary_search(&slot).is_ok());
            // 4b: further from other tables' allocations is better, so invert.
            let distance = u64::MAX - prefs.distance_to_live_elsewhere(slot) as u64;
            // 5: 0 means never freed elsewhere.
            let elsewhere = prefs.freed_elsewhere.get(&slot).copied().unwrap_or(0);
            (here, live, distance, elsewhere, slot)
        })
        .collect();
    keyed.sort_unstable();
    keyed.into_iter().map(|(_, _, _, _, slot)| slot).collect()
}

pub(crate) fn handle_slot(handle: &str) -> Option<(bool, usize)> {
    let indices = word_indices();
    let word_count = words().len();
    let mut parts = handle.split(' ');
    let first = *indices.get(parts.next()?)?;
    let second = parts.next();
    if parts.next().is_some() {
        return None;
    }
    Some(match second {
        None => (true, first),
        Some(second) => (false, first * word_count + *indices.get(second)?),
    })
}


/// A circular mnemonic-handle allocator over opaque binding strings.
///
/// The file tools bind handles to line-content hashes. Nothing about the
/// allocator is specific to files, though — any caller that can produce a stable
/// identity per item gets the same guarantees: a surviving binding keeps its
/// handle across reconciles, a freed one-word handle is not reissued until the
/// cursor wraps, and a two-word handle is never silently shortened. That is
/// exactly the contract an on-screen element needs, so screen anchors and file
/// anchors mint from one implementation rather than two.
///
/// Bindings must be unique within a single [`reconcile`](Self::reconcile) call;
/// a repeated binding collapses to one handle.
#[derive(Clone, Debug, Default)]
pub struct AnchorTable {
    state: HashMap<String, String>,
}

impl AnchorTable {
    /// Restore an allocator from previously persisted state.
    pub fn from_state(state: HashMap<String, String>) -> Self {
        Self { state }
    }

    pub fn state(&self) -> &HashMap<String, String> {
        &self.state
    }

    pub fn into_state(self) -> HashMap<String, String> {
        self.state
    }

    /// Issue or retain one handle per binding, in order.
    ///
    /// `reclaim_dead` frees handles whose bindings are absent from `bindings`.
    /// Callers that recompute the full item set every time — which is the usual
    /// case for a screen — should pass `true`, or a handle leaks per destroyed
    /// item until the word space is exhausted.
    pub fn reconcile(&mut self, bindings: &[String], reclaim_dead: bool) -> Vec<String> {
        self.reconcile_with(bindings, reclaim_dead, &SlotPreference::none())
    }

    /// Reconcile while preferring slots that are not in use, and were not
    /// recently in use, elsewhere. See [`SlotPreference`].
    pub fn reconcile_with(
        &mut self,
        bindings: &[String],
        reclaim_dead: bool,
        prefs: &SlotPreference,
    ) -> Vec<String> {
        // `AnchorTable`'s published state is the flat map, so convert either
        // side rather than forcing the shape on screen anchors too.
        let existing = PathAnchors::from_flat(self.state.clone());
        let (next, visible) = reconcile_handles(&existing, bindings, reclaim_dead, prefs);
        self.state = next.to_flat();
        visible
    }

    /// The binding an issued handle names, or `None` if it was never issued.
    ///
    /// Rejects the allocator's internal cursor keys and anything that is not a
    /// one- or two-word handle from the word list, so a model that invents a
    /// plausible-looking token cannot address an item by accident.
    pub fn binding(&self, handle: &str) -> Option<&str> {
        if !is_mnemonic_handle_in_set(handle, word_set()) {
            return None;
        }
        self.state.get(handle).map(|packed| binding_full(packed))
    }

    pub fn is_issued(&self, handle: &str) -> bool {
        self.binding(handle).is_some()
    }
}

/// The two halves of the staleness contract.
///
/// Shared so the model sees one wording for one concept whether it mis-anchored
/// a file edit or a click.
pub fn not_issued_message(handle: &str, what: &str) -> String {
    format!(
        "'{handle}' is not an issued anchor for this {what}. Use only the bare mnemonic token, \
         exactly as it appeared in the most recent output."
    )
}

pub fn stale_anchor_message(handle: &str, what: &str) -> String {
    format!(
        "anchor '{handle}' is stale: it no longer resolves to anything current. \
         Re-read the {what} to get fresh anchors before acting on it."
    )
}

/// Reconcile model-facing mnemonic handles with a current file view.
///
/// `existing` maps visible handles to packed hidden bindings. When
/// `reclaim_dead` is false, dead bindings remain reserved as tombstones. When
/// true (after an acknowledged successful write/edit), dead bindings and
/// legacy hash-prefix aliases are discarded. Every surviving line retains its
/// existing mnemonic unchanged. Newly created lines use a free one-word handle
/// whenever available, otherwise a two-word handle. Each namespace scans
/// circularly from the slot after its persisted last-assigned cursor. A live
/// two-word handle is never shortened or reassigned merely because one-word
/// capacity later becomes available.
pub(crate) fn reconcile_handles(
    existing: &PathAnchors,
    full_hashes: &[String],
    reclaim_dead: bool,
    prefs: &SlotPreference,
) -> (PathAnchors, Vec<String>) {
    let words = words();
    let word_set = word_set();
    let primary_capacity = words.len();
    let secondary_capacity = primary_capacity * primary_capacity;
    let mut primary_cursor = existing.cursor(true, primary_capacity);
    let secondary_cursor = existing.cursor(false, secondary_capacity);
    let current_hashes: HashSet<&str> = full_hashes.iter().map(String::as_str).collect();

    // Select at most one existing mnemonic per current hidden hash. This also
    // migrates legacy visible hash prefixes to mnemonics on the next render.
    // Normal allocator state has exactly one mnemonic per live hash; the
    // deterministic comparison is only a recovery rule for malformed state.
    let mut preferred_by_hash: HashMap<&str, (&str, &str)> = HashMap::new();
    for (handle, packed) in &existing.bindings {
        let full_hash = binding_full(packed);
        if !current_hashes.contains(full_hash) || !is_mnemonic_handle_in_set(handle, word_set) {
            continue;
        }
        match preferred_by_hash.get(full_hash) {
            Some((current, _)) if !prefer_handle(handle, current) => {}
            _ => {
                preferred_by_hash.insert(full_hash, (handle, packed));
            }
        }
    }

    let mut state = if reclaim_dead {
        HashMap::new()
    } else {
        existing.bindings.clone()
    };
    let mut visible: Vec<Option<String>> = vec![None; full_hashes.len()];

    for (index, full_hash) in full_hashes.iter().enumerate() {
        if let Some((handle, packed)) = preferred_by_hash.get(full_hash.as_str()) {
            visible[index] = Some((*handle).to_owned());
            if reclaim_dead {
                state.insert((*handle).to_owned(), (*packed).to_owned());
            }
        }
    }

    // No filtering needed: `state` holds bindings only, the cursors are fields.
    let mut used: HashSet<String> = state.keys().cloned().collect();
    // Criterion 1: only slots free in *this* table are candidates at all.
    let free: Vec<usize> = (0..primary_capacity)
        .filter(|slot| !used.contains(words[*slot]))
        .collect();
    let free_words: Vec<(usize, &str)> = if prefs.is_informative() {
        rank_slots(free, prefs)
            .into_iter()
            .map(|slot| (slot, words[slot]))
            .collect()
    } else {
        // No cross-table knowledge: keep the original circular scan, so a
        // caller that does not track usage (screen anchors) is unaffected.
        let start = primary_cursor.map_or(0, |index| (index + 1) % primary_capacity);
        let mut ordered: Vec<usize> = free;
        ordered.sort_by_key(|slot| (slot + primary_capacity - start) % primary_capacity);
        ordered
            .into_iter()
            .map(|slot| (slot, words[slot]))
            .collect()
    };
    let mut free_words = free_words.into_iter();

    // Only newly created hashes may consume newly free one-word handles.
    // Existing live handles, including two-word handles, remain immutable.
    let new_indices: Vec<usize> = visible
        .iter()
        .enumerate()
        .filter_map(|(index, handle)| handle.is_none().then_some(index))
        .collect();
    for index in new_indices {
        let Some((slot, word)) = free_words.next() else {
            break;
        };
        let handle = word.to_owned();
        state.insert(handle.clone(), pack_binding(&full_hashes[index]));
        used.insert(handle.clone());
        primary_cursor = Some(slot);
        visible[index] = Some(handle);
    }

    let mut next_pair = secondary_cursor.map_or(0, |index| (index + 1) % secondary_capacity);
    for index in 0..visible.len() {
        if visible[index].is_some() {
            continue;
        }
        let mut searched = 0usize;
        let handle = loop {
            assert!(
                searched < secondary_capacity,
                "mnemonic handle space exhausted"
            );
            searched += 1;
            let slot = next_pair;
            let candidate = format!(
                "{} {}",
                words[slot / primary_capacity],
                words[slot % primary_capacity]
            );
            next_pair = (next_pair + 1) % secondary_capacity;
            if !used.contains(&candidate) {
                break candidate;
            }
        };
        state.insert(handle.clone(), pack_binding(&full_hashes[index]));
        used.insert(handle.clone());
        visible[index] = Some(handle);
    }

    let primary_cursor = primary_cursor.unwrap_or(primary_capacity - 1);
    let secondary_cursor = (next_pair + secondary_capacity - 1) % secondary_capacity;
    (
        PathAnchors {
            bindings: state,
            primary_cursor: Some(primary_cursor),
            secondary_cursor: Some(secondary_cursor),
        },
        visible
            .into_iter()
            .map(|handle| handle.expect("every current line receives a handle"))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hashes(count: usize) -> Vec<String> {
        (0..count).map(|index| format!("{index:013x}")).collect()
    }

    #[test]
    fn one_word_capacity_is_large_enough_for_normal_files() {
        assert!(words().len() >= 2_900);
    }

    #[test]
    fn only_initial_overflow_lines_receive_pairs() {
        let capacity = words().len();
        let full_hashes = hashes(capacity + 2);
        let (state, visible) = reconcile_handles(&PathAnchors::default(), &full_hashes, true, &SlotPreference::none());
        // `capacity + 2` handles and nothing else. This asserted `+ 4` while the
        // two allocator cursors lived in the same map as the bindings.
        assert_eq!(state.bindings.len(), capacity + 2);
        assert_eq!(
            visible.iter().filter(|value| value.contains(' ')).count(),
            2
        );
    }

    #[test]
    fn freed_one_word_goes_to_new_line_without_changing_live_pair() {
        let capacity = words().len();
        let original_hashes = hashes(capacity + 1);
        let (state, original_visible) = reconcile_handles(&PathAnchors::default(), &original_hashes, true, &SlotPreference::none());
        let deleted_handle = original_visible[0].clone();
        let surviving_pair = original_visible.last().unwrap().clone();
        assert!(!deleted_handle.contains(' '));
        assert!(surviving_pair.contains(' '));

        let mut next_hashes = original_hashes[1..].to_vec();
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) = reconcile_handles(&state, &next_hashes, true, &SlotPreference::none());

        assert_eq!(next_visible[capacity - 1], surviving_pair);
        assert_eq!(next_visible.last().unwrap(), &deleted_handle);
        assert_eq!(
            next_visible
                .iter()
                .filter(|value| value.contains(' '))
                .count(),
            1
        );
    }

    #[test]
    fn live_pair_remains_stable_when_file_returns_below_capacity() {
        let capacity = words().len();
        let original_hashes = hashes(capacity + 1);
        let (state, original_visible) = reconcile_handles(&PathAnchors::default(), &original_hashes, true, &SlotPreference::none());
        let surviving_pair = original_visible.last().unwrap().clone();
        assert!(surviving_pair.contains(' '));

        let next_hashes = original_hashes[1..].to_vec();
        let (_, next_visible) = reconcile_handles(&state, &next_hashes, true, &SlotPreference::none());

        assert_eq!(next_visible.last().unwrap(), &surviving_pair);
        assert_eq!(
            next_visible
                .iter()
                .filter(|value| value.contains(' '))
                .count(),
            1
        );
    }

    #[test]
    fn freed_primary_handle_waits_for_cursor_wrap() {
        let original_hashes = hashes(3);
        let (state, original_visible) = reconcile_handles(&PathAnchors::default(), &original_hashes, true, &SlotPreference::none());
        let freed_handle = original_visible[0].clone();

        let mut next_hashes = original_hashes[1..].to_vec();
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) = reconcile_handles(&state, &next_hashes, true, &SlotPreference::none());

        assert_eq!(next_visible[0], original_visible[1]);
        assert_eq!(next_visible[1], original_visible[2]);
        assert_eq!(next_visible[2], words()[3]);
        assert_ne!(next_visible[2], freed_handle);
    }

    #[test]
    fn freed_secondary_handle_waits_for_cursor_wrap() {
        let capacity = words().len();
        let original_hashes = hashes(capacity + 3);
        let (state, original_visible) = reconcile_handles(&PathAnchors::default(), &original_hashes, true, &SlotPreference::none());
        let freed_handle = original_visible[capacity].clone();

        let mut next_hashes = original_hashes[..capacity].to_vec();
        next_hashes.extend_from_slice(&original_hashes[capacity + 1..]);
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) = reconcile_handles(&state, &next_hashes, true, &SlotPreference::none());

        assert_eq!(next_visible[capacity], original_visible[capacity + 1]);
        assert_eq!(next_visible[capacity + 1], original_visible[capacity + 2]);
        assert_eq!(
            next_visible.last().unwrap(),
            &format!("{} {}", words()[0], words()[3])
        );
        assert_ne!(next_visible.last().unwrap(), &freed_handle);
    }

    // --- persisted shape --------------------------------------------------

    /// The flat map with reserved cursor keys is what is already sitting in
    /// every `anchor_states.prefixes_json` column, and the store creates tables
    /// with `CREATE TABLE IF NOT EXISTS` and no migration step. Changing the
    /// shape would fail to deserialize existing rows, so it is pinned here.
    #[test]
    fn the_persisted_shape_survives_a_round_trip() {
        let anchors = PathAnchors {
            bindings: HashMap::from([
                ("time".to_string(), "aaa".to_string()),
                ("beta gamma".to_string(), "bbb".to_string()),
            ]),
            primary_cursor: Some(41),
            secondary_cursor: Some(9001),
        };
        let flat = anchors.to_flat();
        assert_eq!(flat.get(PRIMARY_CURSOR_KEY).map(String::as_str), Some("41"));
        assert_eq!(
            flat.get(SECONDARY_CURSOR_KEY).map(String::as_str),
            Some("9001")
        );
        assert_eq!(flat.get("time").map(String::as_str), Some("aaa"));
        assert_eq!(PathAnchors::from_flat(flat), anchors);
    }

    /// Reading state written before the split must not treat a cursor key as a
    /// handle — the failure mode the typed representation exists to prevent.
    #[test]
    fn cursor_keys_are_never_mistaken_for_bindings() {
        let flat = HashMap::from([
            ("time".to_string(), "aaa".to_string()),
            (PRIMARY_CURSOR_KEY.to_string(), "7".to_string()),
            (SECONDARY_CURSOR_KEY.to_string(), "13".to_string()),
        ]);
        let anchors = PathAnchors::from_flat(flat);
        assert_eq!(anchors.bindings.len(), 1, "a cursor leaked into bindings");
        assert!(anchors.bindings.contains_key("time"));
        assert_eq!(anchors.primary_cursor, Some(7));
        assert_eq!(anchors.secondary_cursor, Some(13));
    }

    /// A cursor that was never written falls back to the highest slot in use,
    /// which is what `recover_cursor` did before the fields existed.
    #[test]
    fn a_missing_cursor_recovers_from_the_handles_in_use() {
        let anchors = PathAnchors::from_flat(HashMap::from([(
            words()[5].to_string(),
            "aaa".to_string(),
        )]));
        assert_eq!(anchors.primary_cursor, None);
        assert_eq!(anchors.cursor(true, words().len()), Some(5));
    }

}
