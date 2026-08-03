//! What to do with a write that resembles something already stored.
//!
//! The store's LSH probe answers "does this look like a fact we hold?", which
//! is not the question a write policy has to answer. A negation shares nearly
//! every 5-gram with what it negates, so the probe fires *hardest* on the
//! writes that matter most:
//!
//! ```text
//! stored:     "always use tabs for indentation in this repo"
//! correction: "never  use tabs for indentation in this repo"   -> near-duplicate
//! ```
//!
//! Treating that as a duplicate drops the correction and leaves the stale
//! belief live — the exact inversion of what a correction trigger is for. The
//! distinction that matters is not *how similar*, it is **restatement versus
//! revision**:
//!
//! * a restatement says the same thing again, and is discarded (no new id, no
//!   event, no churn);
//! * anything else says something *different* about the same subject, and
//!   supersedes what it revises — so the newer belief is what recall returns
//!   and the older one stays reachable through `superseded_by`.
//!
//! Deliberately not a negation-word list. "always/never" is the obvious case,
//! but "use rten" → "use candle" is the same kind of write and carries no
//! negation at all; asking only whether the wording *differs* covers both, and
//! has no vocabulary to fall out of date.
//!
//! **The decision is made here rather than by the LSH index, because the index
//! is not deterministic.** MinHash permutations are seeded when the index is
//! built, so the same pair of texts was flagged in 2 of 5 runs and missed in
//! the other 3 — an admission policy that depends on it decides the same write
//! differently on different machines. Similarity is instead computed exactly
//! over the handful of candidates retrieval returns, which is both cheap and
//! reproducible.

use artist_logic::object::ObjectId;

/// Above this Jaccard similarity, two facts are about the same thing.
///
/// Measured rather than guessed, over a corpus of revision pairs and unrelated
/// pairs drawn from this project's own memories (the test below is that
/// corpus): revisions scored 0.47–0.77 and unrelated pairs 0.00–0.29. This sits
/// in the gap, with margin on both sides.
///
/// Note how far it is from the 0.8 the LSH index targets — that threshold
/// would have missed six of the seven revisions, which is the other half of
/// why admission could not be built on it.
pub const REVISION_SIMILARITY: f64 = 0.38;

/// What a candidate write should do about the facts it resembles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Admission {
    /// Nothing resembles it. Insert.
    Insert,
    /// It repeats a stored fact verbatim, modulo case, spacing and trailing
    /// punctuation. Discard it.
    Restates(ObjectId),
    /// It says something different about the same subject. Insert, and retire
    /// the fact it revises.
    Revises(ObjectId),
}

/// Decide, given whatever retrieval turned up for this text.
///
/// Candidates are the top of an ordinary hybrid search, so most of them are
/// merely *related* and must not be retired — hence the threshold. When more
/// than one clears it the most similar is revised: the store self-prunes,
/// since every revision retires what it revised, so a cluster is rare and
/// picking the closest is the least surprising resolution.
pub fn admit(text: &str, candidates: &[(ObjectId, String)]) -> Admission {
    let normalized = normalize(text);
    if normalized.is_empty() {
        return Admission::Insert;
    }
    let mut best: Option<(ObjectId, f64)> = None;
    for (id, candidate) in candidates {
        let other = normalize(candidate);
        if other == normalized {
            return Admission::Restates(*id);
        }
        let score = similarity(&normalized, &other);
        if score >= REVISION_SIMILARITY && best.is_none_or(|(_, b)| score > b) {
            best = Some((*id, score));
        }
    }
    match best {
        Some((id, _)) => Admission::Revises(id),
        None => Admission::Insert,
    }
}

/// Lowercase, collapse whitespace, drop trailing punctuation.
///
/// The point is that "Adam prefers tabs." and "adam prefers  tabs" are the
/// same belief written twice, and should not produce two ids.
pub fn normalize(text: &str) -> String {
    let lowered = text.to_lowercase();
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed
        .trim_end_matches(['.', '!', ';', ',', ' '])
        .to_owned()
}

/// Jaccard similarity over character 5-grams — the same notion the LSH index
/// uses, computed exactly here because the candidate set is tiny.
fn similarity(a: &str, b: &str) -> f64 {
    let (left, right) = (shingles(a), shingles(b));
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let intersection = left.intersection(&right).count() as f64;
    let union = left.union(&right).count() as f64;
    intersection / union
}

fn shingles(text: &str) -> std::collections::BTreeSet<String> {
    const N: usize = 5;
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= N {
        return std::iter::once(text.to_owned()).collect();
    }
    chars.windows(N).map(|w| w.iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in proposition id; these tests are about the decision, not
    /// about identity derivation.
    fn id(n: u128) -> ObjectId {
        ObjectId(n)
    }

    #[test]
    fn nothing_stored_means_insert() {
        assert_eq!(admit("Adam prefers tabs", &[]), Admission::Insert);
    }

    #[test]
    fn a_restatement_is_discarded() {
        let stored = [(id(7), "Adam prefers tabs over spaces.".to_owned())];
        assert_eq!(
            admit("adam prefers  tabs over spaces", &stored),
            Admission::Restates(id(7))
        );
    }

    /// The case that motivated this module: the write most likely to be
    /// suppressed is the one that reverses a stored belief.
    #[test]
    fn a_negation_revises_rather_than_duplicating() {
        let stored = [(
            id(7),
            "always use tabs for indentation in this repo".to_owned(),
        )];
        assert_eq!(
            admit("never use tabs for indentation in this repo", &stored),
            Admission::Revises(id(7))
        );
    }

    /// A changed value is the same kind of write as a negation, and carries no
    /// negation word — which is why this is not a vocabulary test.
    #[test]
    fn a_changed_value_also_revises() {
        let stored = [(
            id(3),
            "embeddings are produced by rten running CodeRankEmbed".to_owned(),
        )];
        assert_eq!(
            admit("embeddings are produced by rten running bge-small", &stored),
            Admission::Revises(id(3))
        );
    }

    #[test]
    fn the_closest_candidate_is_the_one_revised() {
        let stored = [
            (id(1), "the canvas server hot-reloads React apps".to_owned()),
            (
                id(2),
                "always use tabs for indentation in this repo".to_owned(),
            ),
        ];
        assert_eq!(
            admit("never use tabs for indentation in this repo", &stored),
            Admission::Revises(id(2))
        );
    }

    /// An exact restatement wins over a merely-similar candidate no matter
    /// which order the store returned them in.
    #[test]
    fn an_exact_match_beats_a_similar_one() {
        let stored = [
            (id(1), "always use tabs for indentation".to_owned()),
            (
                id(2),
                "Always use tabs for indentation in this repo.".to_owned(),
            ),
        ];
        assert_eq!(
            admit("always use tabs for indentation in this repo", &stored),
            Admission::Restates(id(2))
        );
    }

    /// Candidates come from an ordinary hybrid search, so most of what arrives
    /// is merely related and must survive untouched.
    #[test]
    fn a_related_but_different_fact_is_not_retired() {
        let stored = [(id(1), "Adam prefers dark mode in the terminal".to_owned())];
        assert_eq!(
            admit("Adam prefers tabs over spaces", &stored),
            Admission::Insert
        );
    }

    /// The corpus `REVISION_SIMILARITY` was calibrated against, kept as a test
    /// so the threshold cannot be nudged without seeing what it breaks. Every
    /// pair is drawn from the kind of memory this project actually stores.
    #[test]
    fn the_threshold_separates_revisions_from_unrelated_facts() {
        const REVISIONS: &[(&str, &str)] = &[
            (
                "always use tabs for indentation in this repo",
                "never use tabs for indentation in this repo",
            ),
            (
                "embeddings are produced by rten running CodeRankEmbed",
                "embeddings are produced by rten running bge-small",
            ),
            (
                "Adam prefers tabs over spaces",
                "Adam prefers spaces over tabs",
            ),
            (
                "stage all work on the Gortnite branch",
                "stage all work on the main branch",
            ),
            (
                "the compaction planner retains 20000 tokens",
                "the compaction planner retains 40000 tokens",
            ),
            (
                "use Edit and Write for source changes",
                "use Edit and Write and NotebookEdit for source changes",
            ),
            (
                "never use python heredocs for source edits",
                "always use python heredocs for source edits",
            ),
        ];
        const UNRELATED: &[(&str, &str)] = &[
            (
                "Adam prefers tabs over spaces",
                "Adam prefers dark mode in the terminal",
            ),
            (
                "the canvas server hot-reloads React apps",
                "the memory store uses RocksDB via mnestic",
            ),
            (
                "stage all work on the Gortnite branch",
                "never create side branches without asking",
            ),
            (
                "the compaction planner retains 20000 tokens",
                "the compaction planner never cuts at a tool result",
            ),
            (
                "use Edit and Write for source changes",
                "use the fff-search file watcher for reindexing",
            ),
            (
                "embeddings are produced by rten",
                "chunking uses text-splitter over tree-sitter",
            ),
        ];

        for (old, new) in REVISIONS {
            let score = similarity(&normalize(old), &normalize(new));
            assert!(
                score >= REVISION_SIMILARITY,
                "{new:?} should revise {old:?}, scored {score:.3}"
            );
            assert_eq!(
                admit(new, &[(id(1), (*old).to_owned())]),
                Admission::Revises(id(1)),
                "{new:?} vs {old:?}"
            );
        }
        for (a, b) in UNRELATED {
            let score = similarity(&normalize(a), &normalize(b));
            assert!(
                score < REVISION_SIMILARITY,
                "{a:?} and {b:?} are unrelated but scored {score:.3}"
            );
            assert_eq!(admit(b, &[(id(1), (*a).to_owned())]), Admission::Insert);
        }
    }
}
