//! Stateless TECA anchors for logical source-line occurrences.
//!
//! Artist owns occurrence identity construction; TECA owns address generation.
//! This module intentionally has no allocator, persisted cursor, hash, or
//! actor-local state.  Prefixes are selected only among the live identities in
//! the view being rendered.

use std::collections::{HashMap, HashSet, VecDeque};

use teca::{default_address, default_atom_ids};

/// Anchor ABI version.  The value identifies the TECA consumer contract, not
/// an Artist-side addressing algorithm. Bumped when the model-visible rendering
/// contract changes.
pub const ANCHOR_ABI_VERSION: &str = "teca-0.1.1-lexicon-atoms";

/// Produce anchors for logical lines belonging to a non-file text surface.
///
/// The identity intentionally contains only canonical line bytes, the fixed
/// label-free role ancestry supplied by the caller, and the bidirectional
/// rank among equivalent siblings. It contains neither a resource path nor
/// actor-local state.
pub fn virtual_line_anchors(lines: &[&str], role: &str) -> Vec<String> {
    let mut equivalent: HashMap<&str, Vec<usize>> = HashMap::new();
    for (index, line) in lines.iter().enumerate() {
        equivalent.entry(line).or_default().push(index);
    }
    let mut identities = vec![Vec::new(); lines.len()];
    for (line, indices) in equivalent {
        let count = indices.len();
        for (position, index) in indices.into_iter().enumerate() {
            let (side, rank) = if position < count / 2 {
                (b'L', position + 1)
            } else {
                // For an odd count, the center is on the right.
                (b'R', count - position)
            };
            let identity = &mut identities[index];
            identity.extend_from_slice(b"artist.anchor.virtual-line.v1\0");
            identity.extend_from_slice(&(role.len() as u64).to_le_bytes());
            identity.extend_from_slice(role.as_bytes());
            identity.extend_from_slice(&(line.len() as u64).to_le_bytes());
            identity.extend_from_slice(line.as_bytes());
            identity.push(side);
            identity.extend_from_slice(&(rank as u64).to_le_bytes());
        }
    }
    shortest_live_anchors(&identities)
}

pub(crate) fn shortest_live_anchors(identities: &[Vec<u8>]) -> Vec<String> {
    if identities.is_empty() {
        return Vec::new();
    }

    let mut unique = HashSet::with_capacity(identities.len());
    for identity in identities {
        assert!(
            unique.insert(identity.as_slice()),
            "TECA occurrence identities must be unique after equivalent-sibling ranking"
        );
    }

    // Calculate rendered-prefix uniqueness using TECA structural atoms.  This
    // avoids depending on the spelling or boundaries of lexicon tokens.
    let streams: Vec<Vec<u32>> = identities
        .iter()
        .map(|identity| {
            default_atom_ids(identity)
                .map(|atom| atom.get())
                .take(32)
                .collect()
        })
        .collect();
    let mut depths = vec![0usize; identities.len()];
    let mut groups = VecDeque::from([(0usize, (0..identities.len()).collect::<Vec<_>>())]);

    while let Some((depth, indices)) = groups.pop_front() {
        let mut buckets: HashMap<u32, Vec<usize>> = HashMap::new();
        for index in indices {
            buckets
                .entry(streams[index][depth])
                .or_default()
                .push(index);
        }
        for bucket in buckets.into_values() {
            if bucket.len() == 1 {
                depths[bucket[0]] = depth + 1;
            } else {
                // TECA addresses are indefinitely extensible.  Grow streams
                // only in the exceptional case where 32 atoms are insufficient.
                if depth + 1 == streams[bucket[0]].len() {
                    return shortest_live_anchors_with_depth(identities, depth + 2);
                }
                groups.push_back((depth + 1, bucket));
            }
        }
    }

    identities
        .iter()
        .zip(depths)
        .map(|(identity, depth)| render_prefix(identity, depth))
        .collect()
}

fn shortest_live_anchors_with_depth(identities: &[Vec<u8>], width: usize) -> Vec<String> {
    let streams: Vec<Vec<u32>> = identities
        .iter()
        .map(|identity| {
            default_atom_ids(identity)
                .map(|atom| atom.get())
                .take(width)
                .collect()
        })
        .collect();
    let mut depths = vec![0usize; identities.len()];
    let mut groups = VecDeque::from([(0usize, (0..identities.len()).collect::<Vec<_>>())]);
    while let Some((depth, indices)) = groups.pop_front() {
        let mut buckets: HashMap<u32, Vec<usize>> = HashMap::new();
        for index in indices {
            buckets
                .entry(streams[index][depth])
                .or_default()
                .push(index);
        }
        for bucket in buckets.into_values() {
            if bucket.len() == 1 {
                depths[bucket[0]] = depth + 1;
            } else if depth + 1 == width {
                return shortest_live_anchors_with_depth(identities, width * 2);
            } else {
                groups.push_back((depth + 1, bucket));
            }
        }
    }
    identities
        .iter()
        .zip(depths)
        .map(|(identity, depth)| render_prefix(identity, depth))
        .collect()
}

/// Render the first `components` lexicon atoms of `identity` as a compact
/// `#...` anchor suffix.
///
/// The canonical embedded lexicon atoms never contain `/`, so joining them with
/// `/` is injective over atom sequences: splitting on `/` recovers the atoms,
/// and no distinct stream can collide with another render.
pub(crate) fn render_prefix(identity: &[u8], components: usize) -> String {
    assert!(components > 0);
    let mut out = String::with_capacity(2 + components * 4);
    out.push('#');
    for (index, atom) in default_address(identity).take(components).enumerate() {
        if index != 0 {
            out.push('/');
        }
        out.push_str(std::str::from_utf8(atom).expect("TECA lexicon atoms are valid UTF-8"));
    }
    out
}

/// Result of resolving an opaque `#...` anchor suffix against candidate TECA streams.
pub(crate) enum AnchorResolution {
    /// Zero candidate streams render to the suffix; the anchor is stale/unknown.
    Unknown,
    /// Exactly one candidate stream renders to the suffix.
    Resolved(usize),
    /// Multiple candidate streams render to the suffix; carries strictly longer
    /// renderings of the matched candidates so the caller can disambiguate.
    Ambiguous(Vec<String>),
}

/// Resolve an opaque `#...` anchor suffix as a prefix over the actual candidate
/// TECA streams.
///
/// The suffix is never parsed. Each candidate identity is rendered atom by atom
/// and the accumulated string is compared at atom boundaries, so a suffix is a
/// match only when it equals the render of the candidate's first N atoms.
/// Zero matches are stale/unknown, one match is the target, and multiple
/// matches are ambiguous — never guessed.
pub(crate) fn resolve_anchor(anchor: &str, identities: &[Vec<u8>]) -> AnchorResolution {
    let suffix = anchor.strip_prefix('#').unwrap_or(anchor);
    let mut matched: Vec<(usize, usize)> = Vec::new();
    for (index, identity) in identities.iter().enumerate() {
        let mut rendered = String::new();
        let mut depth = 0usize;
        for atom in default_address(identity) {
            depth += 1;
            if depth != 1 {
                rendered.push('/');
            }
            rendered
                .push_str(std::str::from_utf8(atom).expect("TECA lexicon atoms are valid UTF-8"));
            if rendered == suffix {
                matched.push((index, depth));
                break;
            }
            if rendered.len() > suffix.len() {
                break;
            }
        }
    }
    match matched.len() {
        0 => AnchorResolution::Unknown,
        1 => AnchorResolution::Resolved(matched[0].0),
        _ => AnchorResolution::Ambiguous(
            matched
                .iter()
                .map(|&(index, depth)| render_prefix(&identities[index], depth + 2))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_deterministic_and_use_teca() {
        let identity = b"canonical occurrence identity";
        assert_eq!(ANCHOR_ABI_VERSION, "teca-0.1.1-lexicon-atoms");
        assert_eq!(render_prefix(identity, 2), render_prefix(identity, 2));
        assert!(render_prefix(identity, 2).starts_with('#'));
    }

    #[test]
    fn rendering_is_compact_lexicon_atoms_joined_by_slash() {
        let identity = b"compact rendering probe";
        let anchor = render_prefix(identity, 3);
        assert!(anchor.starts_with('#'));
        let suffix = &anchor[1..];
        assert!(suffix.contains('/'));
        assert!(!suffix.contains(':'));
        for part in suffix.split('/') {
            assert!(!part.is_empty());
        }
        assert!(render_prefix(identity, 3).len() < b"compact rendering probe".len() * 3);
    }

    #[test]
    fn render_is_injective_for_distinct_identities() {
        let left = render_prefix(b"alpha identity", 4);
        let right = render_prefix(b"beta identity", 4);
        assert_ne!(left, right);
        assert_ne!(
            render_prefix(b"same bytes", 2),
            render_prefix(b"same bytes", 3)
        );
    }

    #[test]
    fn opaque_resolution_reports_unknown_resolved_and_ambiguous() {
        // Two distinct identities whose TECA streams share their first atom
        // (both render atom 0 as "irt").
        let identities: Vec<Vec<u8>> = vec![
            vec![116, 128, 99, 100, 101, 102, 103, 104],
            vec![128, 114, 99, 100, 101, 102, 103, 104],
        ];
        assert_ne!(identities[0], identities[1]);
        let shared = render_prefix(&identities[0], 1);
        match resolve_anchor(&shared, &identities) {
            AnchorResolution::Ambiguous(candidates) => {
                assert!(candidates.len() >= 2);
                assert!(candidates.iter().all(|c| c.starts_with(&shared)));
            }
            _ => panic!("expected ambiguity when a shallow prefix is shared"),
        }

        let deep = render_prefix(&identities[0], 3);
        match resolve_anchor(&deep, &identities) {
            AnchorResolution::Resolved(index) => assert_eq!(index, 0),
            _ => panic!("expected an unambiguous deep prefix to resolve to its identity"),
        }
        match resolve_anchor("#no/such/anchor", &identities) {
            AnchorResolution::Unknown => {}
            _ => panic!("expected unknown for an anchor outside the live set"),
        }
    }

    #[test]
    fn resolution_never_guesses_between_streams() {
        let identities: Vec<Vec<u8>> = vec![b"same prefix a".to_vec(), b"same prefix b".to_vec()];
        let shallow = render_prefix(&identities[0], 2);
        let shallow_for_second = render_prefix(&identities[1], 2);
        assert_ne!(shallow, shallow_for_second);
        match resolve_anchor(&shallow, &identities) {
            AnchorResolution::Resolved(index) => assert_eq!(index, 0),
            AnchorResolution::Ambiguous(_) => {
                // A 2-atom prefix is already unique here; ambiguity must not guess.
            }
            AnchorResolution::Unknown => panic!("expected a match within the live set"),
        }
    }

    #[test]
    fn shortest_prefixes_are_unique_and_order_independent() {
        let left = b"left occurrence".to_vec();
        let right = b"right occurrence".to_vec();
        let forward = shortest_live_anchors(&[left.clone(), right.clone()]);
        let reverse = shortest_live_anchors(&[right, left]);
        assert_ne!(forward[0], forward[1]);
        assert_eq!(forward[0], reverse[1]);
        assert_eq!(forward[1], reverse[0]);
    }

    #[test]
    fn virtual_line_duplicates_are_distinct_and_deterministic() {
        let lines = ["same", "same", "same", "same", "same"];
        let anchors = virtual_line_anchors(&lines, "virtual/terminal/logical-line");
        assert_eq!(anchors.len(), lines.len());
        assert_eq!(anchors.iter().collect::<HashSet<_>>().len(), lines.len());
        assert_eq!(
            anchors,
            virtual_line_anchors(&lines, "virtual/terminal/logical-line")
        );
    }
}
