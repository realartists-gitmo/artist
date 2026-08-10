//! Stable identities, package pins, diagnostics, and canonical hashing.

#![forbid(unsafe_code)]

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(
            Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }

            pub fn validate(&self) -> Result<(), IdentityError> {
                validate_identity(stringify!($name), &self.0)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_owned())
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl FromStr for $name {
            type Err = IdentityError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let id = Self::from(value);
                id.validate()?;
                Ok(id)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

id_type!(PackageId);
id_type!(PackageVersion);
id_type!(ConceptId);
id_type!(RelationId);
id_type!(LexicalEntryId);
id_type!(LexicalSenseId);
id_type!(FrameId);
id_type!(AttestationId);
id_type!(EvidenceId);
id_type!(SourceId);
id_type!(AgentId);
id_type!(ActivityId);
id_type!(AssertionId);
id_type!(InterpretationId);
id_type!(SemanticObjectId);
id_type!(EdgeId);
id_type!(CompetencyId);
id_type!(LanguageTag);
id_type!(NamespaceId);
id_type!(OccurrenceDocumentId);
id_type!(ReferentId);
id_type!(OccurrenceId);
id_type!(PropositionId);
id_type!(ContentId);
id_type!(RootId);
id_type!(ValueId);
id_type!(StatementId);
id_type!(SourceSpanId);
id_type!(VariableId);
id_type!(AmbiguityId);

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum IdentityError {
    #[error("{kind} must not be empty")]
    Empty { kind: &'static str },
    #[error("{kind} contains surrounding whitespace: {value:?}")]
    SurroundingWhitespace { kind: &'static str, value: String },
    #[error("{kind} contains a control character: {value:?}")]
    ControlCharacter { kind: &'static str, value: String },
}

fn validate_identity(kind: &'static str, value: &str) -> Result<(), IdentityError> {
    if value.is_empty() {
        return Err(IdentityError::Empty { kind });
    }
    if value.trim() != value {
        return Err(IdentityError::SurroundingWhitespace {
            kind,
            value: value.to_owned(),
        });
    }
    if value.chars().any(char::is_control) {
        return Err(IdentityError::ControlCharacter {
            kind,
            value: value.to_owned(),
        });
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigestAlgorithm {
    Blake3,
    Sha256,
}

impl fmt::Display for DigestAlgorithm {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Blake3 => "blake3",
            Self::Sha256 => "sha256",
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ContentDigest {
    pub algorithm: DigestAlgorithm,
    pub value: String,
}

impl ContentDigest {
    #[must_use]
    pub fn sha256_bytes(bytes: &[u8]) -> Self {
        Self {
            algorithm: DigestAlgorithm::Sha256,
            value: hex_lower(&sha256(bytes)),
        }
    }

    #[must_use]
    pub fn sha256(value: impl Into<String>) -> Self {
        Self {
            algorithm: DigestAlgorithm::Sha256,
            value: value.into(),
        }
    }

    pub fn validate(&self) -> Result<(), DigestError> {
        let expected = 64;
        if self.value.len() != expected || !self.value.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(DigestError::Malformed {
                algorithm: self.algorithm,
                value: self.value.clone(),
            });
        }
        Ok(())
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.algorithm, self.value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum DigestError {
    #[error("malformed {algorithm} digest: {value}")]
    Malformed {
        algorithm: DigestAlgorithm,
        value: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PackageRef {
    pub id: PackageId,
    pub version: PackageVersion,
    pub digest: ContentDigest,
}

impl PackageRef {
    pub fn validate(&self) -> Result<(), PackageRefError> {
        self.id.validate()?;
        self.version.validate()?;
        self.digest.validate()?;
        Ok(())
    }
}

impl fmt::Display for PackageRef {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}@{}#{}", self.id, self.version, self.digest)
    }
}

#[derive(Debug, Error)]
pub enum PackageRefError {
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Digest(#[from] DigestError),
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImportRequirement {
    pub id: PackageId,
    pub version: Option<PackageVersion>,
    pub digest: Option<ContentDigest>,
    pub optional: bool,
}

impl ImportRequirement {
    #[must_use]
    pub fn exact(package: &PackageRef) -> Self {
        Self {
            id: package.id.clone(),
            version: Some(package.version.clone()),
            digest: Some(package.digest.clone()),
            optional: false,
        }
    }

    #[must_use]
    pub fn matches(&self, package: &PackageRef) -> bool {
        self.id == package.id
            && self
                .version
                .as_ref()
                .is_none_or(|version| version == &package.version)
            && self
                .digest
                .as_ref()
                .is_none_or(|digest| digest == &package.digest)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageKind {
    Ontology,
    Lexicon,
    Provenance,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageHeader {
    pub package: PackageRef,
    pub kind: PackageKind,
    pub title: String,
    pub description: String,
    pub license: String,
    pub imports: Vec<ImportRequirement>,
    pub evidence: Vec<EvidenceId>,
}

impl PackageHeader {
    pub fn validate(&self) -> Result<(), HeaderError> {
        self.package.validate()?;
        validate_identity("package title", &self.title)?;
        validate_identity("package description", &self.description)?;
        validate_identity("package license", &self.license)?;
        for evidence in &self.evidence {
            evidence.validate()?;
        }
        for import in &self.imports {
            import.id.validate()?;
            if let Some(version) = &import.version {
                version.validate()?;
            }
            if let Some(digest) = &import.digest {
                digest.validate()?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum HeaderError {
    #[error(transparent)]
    Package(#[from] PackageRefError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    Digest(#[from] DigestError),
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Confidence(u16);

impl Confidence {
    pub const MAX_BASIS_POINTS: u16 = 10_000;

    pub fn from_basis_points(value: u16) -> Result<Self, ConfidenceError> {
        if value <= Self::MAX_BASIS_POINTS {
            Ok(Self(value))
        } else {
            Err(ConfidenceError::OutOfRange(value))
        }
    }

    #[must_use]
    pub const fn basis_points(self) -> u16 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ConfidenceError {
    #[error("confidence basis points exceed 10,000: {0}")]
    OutOfRange(u16),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: String,
    pub severity: Severity,
    pub message: String,
    pub path: Option<String>,
    pub range: Option<TextRange>,
    pub related: Vec<String>,
    pub evidence: Vec<EvidenceId>,
}

impl Diagnostic {
    #[must_use]
    pub fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Error,
            message: message.into(),
            path: None,
            range: None,
            related: Vec::new(),
            evidence: Vec::new(),
        }
    }

    #[must_use]
    pub fn warning(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            severity: Severity::Warning,
            message: message.into(),
            path: None,
            range: None,
            related: Vec::new(),
            evidence: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum CanonicalHashError {
    #[error("canonical serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

pub fn canonical_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, CanonicalHashError> {
    Ok(serde_json::to_vec(value)?)
}

pub fn canonical_digest<T: Serialize>(value: &T) -> Result<ContentDigest, CanonicalHashError> {
    Ok(ContentDigest::sha256_bytes(&canonical_bytes(value)?))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(*byte >> 4)]));
        output.push(char::from(HEX[usize::from(*byte & 0x0f)]));
    }
    output
}

// SHA-256's published round constants are kept in their canonical compact
// hexadecimal spelling, matching FIPS 180-4 and the surrounding algorithm.
#[allow(clippy::many_single_char_names, clippy::unreadable_literal)]
fn sha256(input: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h = [
        0x6a09e667u32,
        0xbb67ae85,
        0x3c6ef372,
        0xa54ff53a,
        0x510e527f,
        0x9b05688c,
        0x1f83d9ab,
        0x5be0cd19,
    ];

    let bit_len = u64::try_from(input.len())
        .unwrap_or(u64::MAX)
        .wrapping_mul(8);
    let mut padded = input.to_vec();
    padded.push(0x80);
    while padded.len() % 64 != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for (index, word) in chunk.chunks_exact(4).enumerate() {
            w[index] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for index in 16..64 {
            let s0 = w[index - 15].rotate_right(7)
                ^ w[index - 15].rotate_right(18)
                ^ (w[index - 15] >> 3);
            let s1 = w[index - 2].rotate_right(17)
                ^ w[index - 2].rotate_right(19)
                ^ (w[index - 2] >> 10);
            w[index] = w[index - 16]
                .wrapping_add(s0)
                .wrapping_add(w[index - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for index in 0..64 {
            let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let choose = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(big_s1)
                .wrapping_add(choose)
                .wrapping_add(K[index])
                .wrapping_add(w[index]);
            let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let majority = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_s0.wrapping_add(majority);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut output = [0u8; 32];
    for (index, word) in h.iter().enumerate() {
        output[index * 4..index * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_rejects_values_above_one_hundred_percent() {
        assert_eq!(
            Confidence::from_basis_points(10_001),
            Err(ConfidenceError::OutOfRange(10_001))
        );
    }

    #[test]
    fn digest_validation_rejects_non_hex() {
        let digest = ContentDigest {
            algorithm: DigestAlgorithm::Sha256,
            value: "z".repeat(64),
        };
        assert!(digest.validate().is_err());
    }

    #[test]
    fn canonical_digest_is_deterministic() {
        let value = vec!["a", "b", "c"];
        assert_eq!(
            canonical_digest(&value).unwrap(),
            canonical_digest(&value).unwrap()
        );
    }

    #[test]
    fn sha256_matches_standard_test_vector() {
        assert_eq!(
            ContentDigest::sha256_bytes(b"abc").value,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
