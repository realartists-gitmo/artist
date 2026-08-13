//! Harness integration around Teca's content-address generation.
//!
//! Teca generates token streams. This module supplies the file-aware layer:
//! structural input encoding, duplicate ordinals, shortest unique prefixes,
//! and resolution against the current set of addressable records.

use serde::{Deserialize, Serialize};
use std::{collections::HashMap, fmt, sync::OnceLock};

/// One addressable source item before Teca encoding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorInput {
    pub line_text: Vec<u8>,
    pub type_kind_chain: Vec<String>,
}

impl AnchorInput {
    pub fn new(line_text: impl Into<Vec<u8>>, type_kind_chain: Vec<String>) -> Self {
        Self {
            line_text: line_text.into(),
            type_kind_chain,
        }
    }
}

/// A Teca-generated address prefix.
#[derive(Clone, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct Anchor {
    tokens: Vec<String>,
}

impl Anchor {
    pub fn from_tokens(tokens: Vec<String>) -> Self {
        Self { tokens }
    }

    pub fn tokens(&self) -> &[String] {
        &self.tokens
    }
}

impl fmt::Display for Anchor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "#{}", self.tokens.join("."))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AddressedItem {
    pub index: usize,
    pub anchor: Anchor,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnchorError {
    Missing { anchor: Anchor },
    Collision { anchor: Anchor, indices: Vec<usize> },
    EmptyInput,
    InvalidAddress { anchor: Anchor, message: String },
    Neighborhood { message: String },
}

impl fmt::Display for AnchorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { anchor } => write!(formatter, "anchor does not resolve: {anchor}"),
            Self::Collision { anchor, indices } => {
                write!(formatter, "anchor {anchor} collides at items {indices:?}")
            }
            Self::EmptyInput => formatter.write_str("anchor input set is empty"),
            Self::InvalidAddress { anchor, message } => {
                write!(formatter, "anchor {anchor} is invalid: {message}")
            }
            Self::Neighborhood { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for AnchorError {}

/// A current file/resource's addressable items and their shortest unique Teca
/// prefixes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnchorSet {
    neighborhood: teca::Neighborhood,
    items: Vec<AddressedItem>,
}

impl AnchorSet {
    pub fn from_inputs(inputs: &[AnchorInput]) -> Result<Self, AnchorError> {
        if inputs.is_empty() {
            return Err(AnchorError::EmptyInput);
        }

        let encoded = encode_inputs(inputs);
        let mut neighborhood = teca::Neighborhood::canonical();
        for (index, bytes) in encoded.iter().enumerate() {
            neighborhood
                .insert_with_identifier(bytes.clone(), index.to_be_bytes().to_vec())
                .map_err(|error| AnchorError::Neighborhood {
                    message: error.to_string(),
                })?;
        }
        let mut items = neighborhood
            .entries()
            .map(|entry| {
                let index = entry
                    .identifier()
                    .and_then(|identifier| identifier.try_into().ok())
                    .map(usize::from_be_bytes)
                    .ok_or_else(|| AnchorError::Neighborhood {
                        message: "TECA neighborhood entry lost its source index".to_owned(),
                    })?;
                Ok(AddressedItem {
                    index,
                    anchor: anchor_from_address(entry.address()),
                })
            })
            .collect::<Result<Vec<_>, AnchorError>>()?;
        items.sort_by_key(|item| item.index);
        Ok(Self {
            neighborhood,
            items,
        })
    }

    pub fn items(&self) -> &[AddressedItem] {
        &self.items
    }

    pub fn resolve(&self, anchor: &Anchor) -> Result<usize, AnchorError> {
        let address = address_from_anchor(anchor)?;
        match self
            .neighborhood
            .resolve(&address)
            .map_err(|error| AnchorError::InvalidAddress {
                anchor: anchor.clone(),
                message: error.to_string(),
            })? {
            teca::Resolution::NotFound => Err(AnchorError::Missing {
                anchor: anchor.clone(),
            }),
            teca::Resolution::Unique(entry) => entry
                .identifier()
                .and_then(|identifier| identifier.try_into().ok())
                .map(usize::from_be_bytes)
                .ok_or_else(|| AnchorError::Neighborhood {
                    message: "TECA neighborhood entry lost its source index".to_owned(),
                }),
            teca::Resolution::Ambiguous { .. } => {
                let indices = self
                    .neighborhood
                    .entries()
                    .filter(|entry| entry.address().atoms().starts_with(address.atoms()))
                    .filter_map(|entry| {
                        entry
                            .identifier()
                            .and_then(|identifier| identifier.try_into().ok())
                            .map(usize::from_be_bytes)
                    })
                    .collect();
                Err(AnchorError::Collision {
                    anchor: anchor.clone(),
                    indices,
                })
            }
        }
    }
}

fn anchor_from_address(address: &teca::NeighborhoodAddress) -> Anchor {
    Anchor {
        tokens: address
            .atoms()
            .iter()
            .map(|atom| {
                String::from_utf8_lossy(teca::default_lexicon().atom(*atom).unwrap()).into_owned()
            })
            .collect(),
    }
}

fn lexicon_atoms() -> &'static HashMap<Vec<u8>, teca::AtomId> {
    static ATOMS: OnceLock<HashMap<Vec<u8>, teca::AtomId>> = OnceLock::new();
    ATOMS.get_or_init(|| {
        teca::default_lexicon()
            .atoms()
            .enumerate()
            .map(|(index, atom)| (atom.to_vec(), teca::AtomId(index as u32)))
            .collect()
    })
}

fn address_from_anchor(anchor: &Anchor) -> Result<teca::NeighborhoodAddress, AnchorError> {
    let atoms = anchor
        .tokens
        .iter()
        .map(|token| {
            lexicon_atoms()
                .get(token.as_bytes())
                .copied()
                .ok_or_else(|| AnchorError::InvalidAddress {
                    anchor: anchor.clone(),
                    message: format!("unknown TECA token {token:?}"),
                })
        })
        .collect::<Result<Vec<_>, _>>()?;
    teca::NeighborhoodAddress::new(atoms).map_err(|error| AnchorError::InvalidAddress {
        anchor: anchor.clone(),
        message: error.to_string(),
    })
}

fn encode_inputs(inputs: &[AnchorInput]) -> Vec<Vec<u8>> {
    let mut occurrences = std::collections::HashMap::<Vec<u8>, Vec<usize>>::new();
    let bases = inputs.iter().map(encode_base).collect::<Vec<_>>();
    for (index, base) in bases.iter().enumerate() {
        occurrences.entry(base.clone()).or_default().push(index);
    }

    bases
        .into_iter()
        .enumerate()
        .map(|(index, mut base)| {
            let group = occurrences
                .values()
                .find(|indices| indices.contains(&index))
                .expect("every input belongs to an occurrence group");
            let position = group
                .iter()
                .position(|candidate| *candidate == index)
                .expect("group contains its member");
            let count = group.len();
            let ordinal = if position < count / 2 {
                format!("L{}", position + 1)
            } else {
                format!("R{}", count - position)
            };
            append_field(&mut base, ordinal.as_bytes());
            base
        })
        .collect()
}

fn encode_base(input: &AnchorInput) -> Vec<u8> {
    let mut encoded = Vec::new();
    append_field(&mut encoded, b"artist-anchor-v1");
    append_field(&mut encoded, &input.line_text);
    for kind in &input.type_kind_chain {
        append_field(&mut encoded, kind.as_bytes());
    }
    encoded
}

fn append_field(output: &mut Vec<u8>, field: &[u8]) {
    output.extend_from_slice(&(field.len() as u64).to_le_bytes());
    output.extend_from_slice(field);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str) -> AnchorInput {
        AnchorInput::new(text.as_bytes().to_vec(), vec!["function".to_owned()])
    }

    #[test]
    fn generates_resolvable_shortest_prefixes() {
        let set = AnchorSet::from_inputs(&[line("alpha"), line("beta"), line("gamma")]).unwrap();
        assert_eq!(set.items().len(), 3);
        for item in set.items() {
            assert_eq!(set.resolve(&item.anchor), Ok(item.index));
            assert!(!item.anchor.tokens().is_empty());
        }
    }

    #[test]
    fn unrelated_churn_preserves_existing_addresses() {
        let before = AnchorSet::from_inputs(&[line("alpha"), line("beta")]).unwrap();
        let after =
            AnchorSet::from_inputs(&[line("new unrelated line"), line("alpha"), line("beta")])
                .unwrap();
        assert_eq!(before.items()[0].anchor, after.items()[1].anchor);
        assert_eq!(before.items()[1].anchor, after.items()[2].anchor);
    }

    #[test]
    fn duplicate_inputs_receive_distinct_ordinals() {
        let set = AnchorSet::from_inputs(&[line("same"), line("same"), line("same")]).unwrap();
        let anchors = set
            .items()
            .iter()
            .map(|item| item.anchor.clone())
            .collect::<Vec<_>>();
        assert_eq!(anchors.len(), 3);
        assert_ne!(anchors[0], anchors[1]);
        assert_ne!(anchors[1], anchors[2]);
        assert_eq!(set.resolve(&anchors[0]), Ok(0));
        assert_eq!(set.resolve(&anchors[1]), Ok(1));
        assert_eq!(set.resolve(&anchors[2]), Ok(2));
    }

    #[test]
    fn stale_prefixes_fail_loudly() {
        let set = AnchorSet::from_inputs(&[line("alpha"), line("beta")]).unwrap();
        let missing = Anchor {
            tokens: vec!["does-not-exist".to_owned()],
        };
        assert!(matches!(
            set.resolve(&missing),
            Err(AnchorError::Missing { .. } | AnchorError::InvalidAddress { .. })
        ));
    }
}
