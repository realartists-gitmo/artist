//! What a fact *is*, as distinct from what was observed.
//!
//! A stored fact used to conflate two things behind one counter-derived id, and
//! under replication they want opposite behaviour:
//!
//! * The **proposition** — the belief itself — should *converge*. Two machines
//!   that independently learn the same thing must end up with one row, or
//!   cross-peer dedup is impossible and the store grows a copy per peer forever.
//! * The **observation** — that *this* session recorded it at *this* point —
//!   should *not* converge. "My desktop learned this at T1" and "my laptop
//!   learned it at T2" are two facts about the world, and collapsing them
//!   destroys the signal that two independent sources agree, which is evidence.
//!
//! So the proposition is keyed by content and the observation by
//! `(source_session, source_seq)`. Those columns already existed and were
//! already populated on every row; what was missing was treating them as an
//! identity rather than as metadata.
//!
//! The proposition id is the **object graph's** content id for the text interned
//! as an atom — not a private hash. Facts are becoming object-graph expressions
//! with the sentence derived by printing them, so minting a second identity
//! space for them now would only have to be reconciled later.

use artist_logic::object::{CoreNode, ObjectId, content_id};

use crate::admission::normalize;

/// The converging identity of a belief.
///
/// Derived from the *normalized* text, so "Adam prefers tabs." and
/// "adam prefers  tabs" are one proposition rather than two — the same
/// normalization admission already uses to decide a restatement, which is what
/// makes exact dedup a keyed lookup instead of a search.
pub fn proposition_id(text: &str) -> ObjectId {
    content_id(&CoreNode::Atom {
        name: Some(normalize(text)),
    })
}

/// Split a 128-bit id into the two `i64`s Cozo can store. Mirrors
/// `assertion_store`, which faced the same limit.
pub fn split(id: ObjectId) -> (i64, i64) {
    ((id.0 >> 64) as i64, (id.0 & u64::MAX as u128) as i64)
}

pub fn join(hi: i64, lo: i64) -> ObjectId {
    ObjectId(((hi as u64 as u128) << 64) | (lo as u64 as u128))
}

/// How many hex digits of a proposition id to show the model.
///
/// The full id is 32 hex digits. Rendering that costs roughly 32 tokens against
/// 2 for the old counter, on a surface that injects 8 facts every turn and
/// expects the model to echo one back in `replaces:` — so the full form is paid
/// twice. A prefix is the same trade git makes, and for the same reason.
pub const HANDLE_LEN: usize = 12;

/// The short form shown to the model.
pub fn handle(id: ObjectId) -> String {
    format!("{:032x}", id.0)[..HANDLE_LEN].to_string()
}

/// Errors resolving a handle the model quoted back.
#[derive(Debug, PartialEq)]
pub enum HandleError {
    /// No stored fact starts with it.
    Unknown,
    /// More than one does. Never silently pick — superseding the wrong belief
    /// is worse than refusing.
    Ambiguous(usize),
}

impl std::fmt::Display for HandleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandleError::Unknown => write!(f, "no fact matches that id"),
            HandleError::Ambiguous(n) => {
                write!(
                    f,
                    "that id is ambiguous — {n} facts match; use more characters"
                )
            }
        }
    }
}

impl std::error::Error for HandleError {}

/// Resolve a prefix against known ids.
///
/// Case-insensitive, and tolerant of the model echoing the full id back.
pub fn resolve(prefix: &str, known: &[ObjectId]) -> Result<ObjectId, HandleError> {
    let needle = prefix.trim().to_ascii_lowercase();
    let matches: Vec<ObjectId> = known
        .iter()
        .copied()
        .filter(|id| format!("{:032x}", id.0).starts_with(&needle))
        .collect();
    match matches.len() {
        0 => Err(HandleError::Unknown),
        1 => Ok(matches[0]),
        n => Err(HandleError::Ambiguous(n)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point: the same belief written twice, on two machines, is one
    /// proposition. Without this cross-peer dedup cannot exist.
    #[test]
    fn normalization_makes_restatements_one_proposition() {
        assert_eq!(
            proposition_id("Adam prefers tabs."),
            proposition_id("adam prefers  tabs")
        );
        assert_ne!(
            proposition_id("Adam prefers tabs"),
            proposition_id("Adam prefers spaces")
        );
    }

    /// Ids must land in the content space, not the nominal one — a nominal id
    /// is random per graph and would never converge across machines.
    #[test]
    fn proposition_ids_are_content_addressed() {
        use artist_logic::object::IdSpace;
        assert_eq!(proposition_id("anything at all").space(), IdSpace::Content);
    }

    #[test]
    fn split_and_join_round_trip() {
        let id = proposition_id("a fact worth keeping");
        let (hi, lo) = split(id);
        assert_eq!(join(hi, lo), id);
    }

    #[test]
    fn a_handle_prefixes_its_own_id() {
        let id = proposition_id("some belief");
        assert_eq!(handle(id).len(), HANDLE_LEN);
        assert!(format!("{:032x}", id.0).starts_with(&handle(id)));
    }

    #[test]
    fn resolve_finds_by_prefix_and_refuses_ambiguity() {
        let a = proposition_id("first");
        let b = proposition_id("second");
        assert_eq!(resolve(&handle(a), &[a, b]), Ok(a));
        // The full id resolves too — the model may echo either form.
        assert_eq!(resolve(&format!("{:032x}", a.0), &[a, b]), Ok(a));
        assert_eq!(resolve("zzzz", &[a, b]), Err(HandleError::Unknown));
        // An empty prefix matches everything, and must not pick one.
        assert_eq!(resolve("", &[a, b]), Err(HandleError::Ambiguous(2)));
    }
}
