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

fn handle_slot(handle: &str) -> Option<(bool, usize)> {
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

fn recover_cursor(
    existing: &HashMap<String, String>,
    metadata_key: &str,
    primary: bool,
    capacity: usize,
) -> Option<usize> {
    existing
        .get(metadata_key)
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|index| *index < capacity)
        .or_else(|| {
            existing
                .keys()
                .filter_map(|handle| handle_slot(handle))
                .filter_map(|(is_primary, index)| (is_primary == primary).then_some(index))
                .max()
        })
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
    existing: &HashMap<String, String>,
    full_hashes: &[String],
    reclaim_dead: bool,
) -> (HashMap<String, String>, Vec<String>) {
    let words = words();
    let word_set = word_set();
    let primary_capacity = words.len();
    let secondary_capacity = primary_capacity * primary_capacity;
    let mut primary_cursor = recover_cursor(existing, PRIMARY_CURSOR_KEY, true, primary_capacity);
    let secondary_cursor =
        recover_cursor(existing, SECONDARY_CURSOR_KEY, false, secondary_capacity);
    let current_hashes: HashSet<&str> = full_hashes.iter().map(String::as_str).collect();

    // Select at most one existing mnemonic per current hidden hash. This also
    // migrates legacy visible hash prefixes to mnemonics on the next render.
    // Normal allocator state has exactly one mnemonic per live hash; the
    // deterministic comparison is only a recovery rule for malformed state.
    let mut preferred_by_hash: HashMap<&str, (&str, &str)> = HashMap::new();
    for (handle, packed) in existing {
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
        existing.clone()
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

    let mut used: HashSet<String> = state
        .keys()
        .filter(|handle| is_mnemonic_handle_in_set(handle, word_set))
        .cloned()
        .collect();
    let primary_start = primary_cursor.map_or(0, |index| (index + 1) % primary_capacity);
    let free_words: Vec<(usize, &str)> = (0..primary_capacity)
        .map(|offset| (primary_start + offset) % primary_capacity)
        .filter(|slot| !used.contains(words[*slot]))
        .map(|slot| (slot, words[slot]))
        .collect();
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
        state.insert(
            handle.clone(),
            pack_binding(&full_hashes[index]),
        );
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
        state.insert(
            handle.clone(),
            pack_binding(&full_hashes[index]),
        );
        used.insert(handle.clone());
        visible[index] = Some(handle);
    }

    let primary_cursor = primary_cursor.unwrap_or(primary_capacity - 1);
    state.insert(PRIMARY_CURSOR_KEY.to_owned(), primary_cursor.to_string());
    let secondary_cursor = (next_pair + secondary_capacity - 1) % secondary_capacity;
    state.insert(
        SECONDARY_CURSOR_KEY.to_owned(),
        secondary_cursor.to_string(),
    );
    (
        state,
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
        let (state, visible) =
            reconcile_handles(&HashMap::new(), &full_hashes, true);
        assert_eq!(state.len(), capacity + 4);
        assert_eq!(
            visible.iter().filter(|value| value.contains(' ')).count(),
            2
        );
    }

    #[test]
    fn freed_one_word_goes_to_new_line_without_changing_live_pair() {
        let capacity = words().len();
        let original_hashes = hashes(capacity + 1);
        let (state, original_visible) =
            reconcile_handles(&HashMap::new(), &original_hashes, true);
        let deleted_handle = original_visible[0].clone();
        let surviving_pair = original_visible.last().unwrap().clone();
        assert!(!deleted_handle.contains(' '));
        assert!(surviving_pair.contains(' '));

        let mut next_hashes = original_hashes[1..].to_vec();
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) =
            reconcile_handles(&state, &next_hashes, true);

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
        let (state, original_visible) =
            reconcile_handles(&HashMap::new(), &original_hashes, true);
        let surviving_pair = original_visible.last().unwrap().clone();
        assert!(surviving_pair.contains(' '));

        let next_hashes = original_hashes[1..].to_vec();
        let (_, next_visible) =
            reconcile_handles(&state, &next_hashes, true);

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
        let (state, original_visible) =
            reconcile_handles(&HashMap::new(), &original_hashes, true);
        let freed_handle = original_visible[0].clone();

        let mut next_hashes = original_hashes[1..].to_vec();
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) =
            reconcile_handles(&state, &next_hashes, true);

        assert_eq!(next_visible[0], original_visible[1]);
        assert_eq!(next_visible[1], original_visible[2]);
        assert_eq!(next_visible[2], words()[3]);
        assert_ne!(next_visible[2], freed_handle);
    }

    #[test]
    fn freed_secondary_handle_waits_for_cursor_wrap() {
        let capacity = words().len();
        let original_hashes = hashes(capacity + 3);
        let (state, original_visible) =
            reconcile_handles(&HashMap::new(), &original_hashes, true);
        let freed_handle = original_visible[capacity].clone();

        let mut next_hashes = original_hashes[..capacity].to_vec();
        next_hashes.extend_from_slice(&original_hashes[capacity + 1..]);
        next_hashes.push("new0000000000".to_owned());
        let (_, next_visible) =
            reconcile_handles(&state, &next_hashes, true);

        assert_eq!(next_visible[capacity], original_visible[capacity + 1]);
        assert_eq!(next_visible[capacity + 1], original_visible[capacity + 2]);
        assert_eq!(
            next_visible.last().unwrap(),
            &format!("{} {}", words()[0], words()[3])
        );
        assert_ne!(next_visible.last().unwrap(), &freed_handle);
    }
}
