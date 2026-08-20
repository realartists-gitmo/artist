//! Session identity selection for the default composition.
//!
//! The roster is data, not prompt policy. A durable registry may later claim
//! names across processes; this small catalog supplies deterministic selection
//! for hosts that have not installed one yet.

use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityCatalog {
    names: Vec<String>,
}

impl IdentityCatalog {
    pub fn from_text(text: &str) -> Self {
        let names = text
            .lines()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.starts_with('#'))
            .map(str::to_owned)
            .collect();
        Self { names }
    }

    pub fn default_roster() -> Self {
        Self::from_text(include_str!("../../../docs/artists.md"))
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn select(&self, session_id: &str) -> Option<&str> {
        if self.names.is_empty() {
            return None;
        }
        let digest = Sha256::digest(session_id.as_bytes());
        let mut index = [0u8; 8];
        index.copy_from_slice(&digest[..8]);
        Some(&self.names[u64::from_le_bytes(index) as usize % self.names.len()])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_is_loaded_and_selection_is_stable() {
        let catalog = IdentityCatalog::default_roster();
        assert!(catalog.names().len() > 700);
        assert_eq!(catalog.select("session-1"), catalog.select("session-1"));
    }
}
