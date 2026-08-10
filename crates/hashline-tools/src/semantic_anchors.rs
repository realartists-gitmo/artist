//! Stateless TECA anchors for logical source-line occurrences.
//!
//! Artist owns occurrence identity construction; TECA owns address generation.
//! This module intentionally has no allocator, persisted cursor, hash, or
//! actor-local state.  Prefixes are selected only among the live identities in
//! the view being rendered.

use std::collections::{HashMap, HashSet, VecDeque};

use teca::{default_atom_ids, default_lexicon, render::render_text_prefix};

/// Anchor ABI version.  The value identifies the TECA consumer contract, not
/// an Artist-side addressing algorithm.
pub const ANCHOR_ABI_VERSION: &str = "teca-0.1.0";

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

pub(crate) fn render_prefix(identity: &[u8], components: usize) -> String {
    assert!(components > 0);
    let mut address = default_atom_ids(identity);
    format!(
        "#{}",
        render_text_prefix(&mut address, default_lexicon(), components)
            .expect("the embedded TECA scheme and lexicon must agree")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchors_are_deterministic_and_use_teca() {
        let identity = b"canonical occurrence identity";
        assert_eq!(ANCHOR_ABI_VERSION, "teca-0.1.0");
        assert_eq!(render_prefix(identity, 2), render_prefix(identity, 2));
        assert!(render_prefix(identity, 2).starts_with('#'));
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
