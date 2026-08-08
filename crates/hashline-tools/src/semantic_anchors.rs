use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::OnceLock;

use crate::anchor_address_v1::{
    anchor_address_component, anchor_address_prefix, ADDRESS_ALPHABET_SIZE,
};

/// Separator between address components after the leading `#`.
///
/// U+2016 does not occur in any payload in the frozen v1 token alphabet, so the
/// textual grammar remains unambiguous without altering any token payload.
pub const ANCHOR_ABI_VERSION: &str = "v1";
pub const COMPONENT_SEPARATOR: char = '‖';

const TOKENS_TEXT: &str = include_str!("anchor_tokens_68399.txt");

fn tokens() -> &'static [&'static str] {
    static TOKENS: OnceLock<Vec<&'static str>> = OnceLock::new();
    TOKENS
        .get_or_init(|| {
            let tokens: Vec<&str> = TOKENS_TEXT.lines().collect();
            assert_eq!(tokens.len(), ADDRESS_ALPHABET_SIZE);
            assert!(tokens
                .iter()
                .all(|token| !token.contains(COMPONENT_SEPARATOR)));
            tokens
        })
        .as_slice()
}

pub(crate) fn shortest_live_anchors(identities: &[Vec<u8>]) -> Vec<String> {
    if identities.is_empty() {
        return Vec::new();
    }

    let mut unique = HashSet::with_capacity(identities.len());
    for identity in identities {
        assert!(
            unique.insert(identity.as_slice()),
            "v1 occurrence identities must be unique after equivalent-sibling ranking"
        );
    }

    let vocabulary = tokens();
    let mut depths = vec![0usize; identities.len()];
    let mut groups = VecDeque::from([(0usize, (0..identities.len()).collect::<Vec<_>>())]);

    while let Some((depth, indices)) = groups.pop_front() {
        // The frozen vocabulary contains a small number of duplicate payload rows.
        // Prefix uniqueness is therefore defined over rendered payloads, not merely
        // numeric field values. Different field elements that render identically extend
        // by another component; the v1 numeric address itself is never changed.
        let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
        for index in indices {
            let component = anchor_address_component(&identities[index], depth) as usize;
            buckets
                .entry(vocabulary[component])
                .or_default()
                .push(index);
        }

        for bucket in buckets.into_values() {
            if bucket.len() == 1 {
                depths[bucket[0]] = depth + 1;
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
    let vocabulary = tokens();
    let mut rendered = String::from("#");
    for (depth, value) in anchor_address_prefix(identity, components)
        .into_iter()
        .enumerate()
    {
        if depth != 0 {
            rendered.push(COMPONENT_SEPARATOR);
        }
        rendered.push_str(vocabulary[value as usize]);
    }
    rendered
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor_address_v1::{
        anchor_address_prefix, BYTE_CODEC_MULTIPLIER, BYTE_CODEC_OFFSET, FIELD_PRIME,
        OPTIMIZED_POINTS, TAIL_OFFSET, TAIL_STEP,
    };

    #[test]
    fn frozen_v1_constants_are_exact() {
        assert_eq!(ANCHOR_ABI_VERSION, "v1");
        let scheme: String = include_str!("scheme_v1.json")
            .chars()
            .filter(|ch| !ch.is_whitespace())
            .collect();
        assert!(scheme.contains("\"field_prime\":68399"));
        assert!(scheme.contains("\"optimized_points\":[49667,38410,64413,58963]"));
        assert!(scheme.contains("\"tail_offset\":30920"));
        assert!(scheme.contains("\"tail_step\":8449"));
        assert_eq!(FIELD_PRIME, 68_399);
        assert_eq!(ADDRESS_ALPHABET_SIZE, 68_399);
        assert_eq!(BYTE_CODEC_MULTIPLIER, 17);
        assert_eq!(BYTE_CODEC_OFFSET, 17);
        assert_eq!(OPTIMIZED_POINTS, &[49_667, 38_410, 64_413, 58_963]);
        assert_eq!(TAIL_OFFSET, 30_920);
        assert_eq!(TAIL_STEP, 8_449);
    }

    #[test]
    fn token_rows_are_direct_and_separator_is_reserved() {
        let vocabulary = tokens();
        assert_eq!(vocabulary.len(), 68_399);
        assert_eq!(vocabulary[0], "%");
        assert_eq!(vocabulary[7], "0");
        assert_eq!(vocabulary[68_398], "flashdata");
        assert!(vocabulary
            .iter()
            .all(|token| !token.contains(COMPONENT_SEPARATOR)));
        assert!(vocabulary.iter().all(|token| !token.contains(": ")));
        assert!(vocabulary.iter().all(|token| !token.contains(" ⟶ ")));
        // The source artifact intentionally/actually contains duplicate payload rows;
        // the live-prefix algorithm must handle them textually rather than mutating the
        // frozen row mapping.
        assert_eq!(
            vocabulary.iter().copied().collect::<HashSet<_>>().len(),
            68_384
        );
    }

    #[test]
    fn render_maps_each_field_value_directly_to_its_row() {
        let identity = b"canonical identity bytes";
        let values = anchor_address_prefix(identity, 3);
        let vocabulary = tokens();
        let expected = format!(
            "#{}{}{}{}{}",
            vocabulary[values[0] as usize],
            COMPONENT_SEPARATOR,
            vocabulary[values[1] as usize],
            COMPONENT_SEPARATOR,
            vocabulary[values[2] as usize],
        );
        assert_eq!(render_prefix(identity, 3), expected);
    }

    #[test]
    fn duplicate_token_rows_extend_textual_prefixes() {
        let vocabulary = tokens();
        assert_eq!(vocabulary[1_402], vocabulary[10_841]);

        let mut left = None;
        let mut right = None;
        for n in 0u64..2_000_000 {
            let identity = format!("duplicate-token-row-{n}").into_bytes();
            match anchor_address_component(&identity, 0) {
                1_402 if left.is_none() => left = Some(identity),
                10_841 if right.is_none() => right = Some(identity),
                _ => {}
            }
            if left.is_some() && right.is_some() {
                break;
            }
        }
        let left = left.expect("identity hitting first duplicate token row");
        let right = right.expect("identity hitting second duplicate token row");
        assert_eq!(render_prefix(&left, 1), render_prefix(&right, 1));

        let rendered = shortest_live_anchors(&[left.clone(), right.clone()]);
        assert_ne!(rendered[0], rendered[1]);
        assert_eq!(rendered[0], render_prefix(&left, 2));
        assert_eq!(rendered[1], render_prefix(&right, 2));
    }

    #[test]
    fn live_first_component_collision_extends_without_reassignment() {
        let mut seen: HashMap<u32, Vec<u8>> = HashMap::new();
        let (a, b) = (0u64..200_000)
            .find_map(|n| {
                let candidate = format!("identity-{n}").into_bytes();
                let first = anchor_address_component(&candidate, 0);
                match seen.insert(first, candidate.clone()) {
                    Some(previous)
                        if anchor_address_component(&previous, 1)
                            != anchor_address_component(&candidate, 1) =>
                    {
                        Some((previous, candidate))
                    }
                    _ => None,
                }
            })
            .expect("expected a first-component collision in bounded search");

        let solo = shortest_live_anchors(std::slice::from_ref(&a));
        let together = shortest_live_anchors(&[a.clone(), b.clone()]);
        assert_eq!(solo[0], render_prefix(&a, 1));
        assert_eq!(together[0], render_prefix(&a, 2));
        assert_eq!(together[1], render_prefix(&b, 2));

        let reversed = shortest_live_anchors(&[b.clone(), a.clone()]);
        assert_eq!(together[0], reversed[1]);
        assert_eq!(together[1], reversed[0]);
    }

    #[test]
    fn unrelated_identity_does_not_change_existing_unique_prefixes() {
        let a = b"alpha identity".to_vec();
        let b = b"beta identity".to_vec();
        let before = shortest_live_anchors(&[a.clone(), b.clone()]);

        let mut n = 0u64;
        let c = loop {
            let candidate = format!("other-{n}").into_bytes();
            let first = anchor_address_component(&candidate, 0);
            if first != anchor_address_component(&a, 0) && first != anchor_address_component(&b, 0)
            {
                break candidate;
            }
            n += 1;
        };
        let after = shortest_live_anchors(&[a, b, c]);
        assert_eq!(&before, &after[..2]);
    }
}
