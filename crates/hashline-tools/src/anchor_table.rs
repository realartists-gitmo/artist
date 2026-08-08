use std::collections::HashMap;

use crate::semantic_anchors::shortest_live_anchors;

/// Deterministic live-set address table for non-file surfaces such as `computer`.
///
/// Allocation is stateless: every reconciliation recomputes v1 addresses directly
/// from complete binding identities. The map retained here exists only so callers can
/// resolve the currently live opaque address back to its binding.
#[derive(Clone, Debug, Default)]
pub struct AnchorTable {
    live: HashMap<String, String>,
}

impl AnchorTable {
    pub fn reconcile(&mut self, bindings: &[String]) -> Vec<String> {
        let mut ranks: HashMap<&str, u64> = HashMap::new();
        let identities: Vec<Vec<u8>> = bindings
            .iter()
            .map(|binding| {
                let rank = ranks.entry(binding.as_str()).or_insert(0);
                let mut identity = Vec::with_capacity(binding.len() + 40);
                identity.extend_from_slice(b"artist.anchor.binding.v1\0");
                identity.extend_from_slice(&(binding.len() as u64).to_le_bytes());
                identity.extend_from_slice(binding.as_bytes());
                identity.extend_from_slice(&rank.to_le_bytes());
                *rank += 1;
                identity
            })
            .collect();
        let anchors = shortest_live_anchors(&identities);
        self.live = anchors
            .iter()
            .cloned()
            .zip(bindings.iter().cloned())
            .collect();
        anchors
    }

    pub fn binding(&self, anchor: &str) -> Option<&str> {
        self.live.get(anchor).map(String::as_str)
    }

    pub fn is_issued(&self, anchor: &str) -> bool {
        self.live.contains_key(anchor)
    }
}

pub fn not_issued_message(anchor: &str, what: &str) -> String {
    format!(
        "anchor '{anchor}' was not issued for this {what}; use an exact anchor from the current observation"
    )
}

pub fn stale_anchor_message(anchor: &str, what: &str) -> String {
    format!(
        "anchor '{anchor}' is stale for this {what}; observe again and use an exact current anchor"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_deterministic_and_order_independent_for_distinct_bindings() {
        let mut table = AnchorTable::default();
        let a = table.reconcile(&["alpha".into(), "beta".into()]);
        let mut reversed = AnchorTable::default();
        let b = reversed.reconcile(&["beta".into(), "alpha".into()]);
        assert_eq!(a[0], b[1]);
        assert_eq!(a[1], b[0]);
    }

    #[test]
    fn duplicate_bindings_get_complete_ranked_identities() {
        let mut table = AnchorTable::default();
        let anchors = table.reconcile(&["same".into(), "same".into()]);
        assert_ne!(anchors[0], anchors[1]);
    }

    #[test]
    fn resolution_is_exact_and_opaque() {
        let mut table = AnchorTable::default();
        let anchors = table.reconcile(&["binding".into()]);
        let anchor = &anchors[0];
        assert_eq!(table.binding(anchor), Some("binding"));
        assert_eq!(table.binding(&format!(" {anchor}")), None);
        assert_eq!(table.binding(&anchor.to_uppercase()), None);
    }
}
