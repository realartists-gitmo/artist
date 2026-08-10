use artist_empirical::{InferenceArtifact, Observation, PosteriorClaim};
use artist_formal::InterpretedGraph;
use artist_kernel::{
    Certificate, CheckSession, CompiledOntologySubmission, DependencyReport, ElaborationRecord,
};
use artist_theories::{TheoryPackage, UniversalRun};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use thiserror::Error;

/// Stable artifact contract for durable typed values.
pub trait ArtifactContract {
    /// Globally stable artifact kind.
    const KIND: &'static str;
    /// Kind-specific schema version.
    const FORMAT_VERSION: u32 = 1;
}

/// Durable canonical JSON artifact envelope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactEnvelope {
    /// Stable artifact kind, such as `artist.formal.interpreted-graph`.
    pub kind: String,
    /// Kind-specific schema version.
    pub format_version: u32,
    /// Canonicalizable payload.
    pub payload: Value,
    /// BLAKE3 digest of kind, version, and canonical payload bytes.
    pub digest: String,
}

/// Artifact encoding, corruption, or type failure.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ArtifactError {
    /// Serialization or deserialization failed.
    #[error("artifact JSON failed: {0}")]
    Json(String),
    /// Digest does not match the canonical bytes.
    #[error("artifact digest mismatch")]
    DigestMismatch,
    /// Artifact kind or format differs from the expected contract.
    #[error(
        "expected artifact {expected_kind} format {expected_version}, found {actual_kind} format {actual_version}"
    )]
    WrongContract {
        expected_kind: String,
        expected_version: u32,
        actual_kind: String,
        actual_version: u32,
    },
}

impl ArtifactEnvelope {
    /// Encodes a value into a canonical, content-addressed envelope.
    pub fn encode<T: Serialize>(
        kind: impl Into<String>,
        format_version: u32,
        value: &T,
    ) -> Result<Self, ArtifactError> {
        let kind = kind.into();
        let payload =
            serde_json::to_value(value).map_err(|error| ArtifactError::Json(error.to_string()))?;
        let digest = compute_digest(&kind, format_version, &payload);
        Ok(Self {
            kind,
            format_version,
            payload,
            digest,
        })
    }

    /// Encodes a value using its stable declared artifact contract.
    pub fn encode_typed<T: Serialize + ArtifactContract>(value: &T) -> Result<Self, ArtifactError> {
        Self::encode(T::KIND, T::FORMAT_VERSION, value)
    }

    /// Returns the canonical artifact digest without retaining the envelope.
    pub fn digest_typed<T: Serialize + ArtifactContract>(
        value: &T,
    ) -> Result<String, ArtifactError> {
        Ok(Self::encode_typed(value)?.digest)
    }

    /// Verifies and decodes a value using its stable declared artifact contract.
    pub fn decode_typed<T: DeserializeOwned + ArtifactContract>(&self) -> Result<T, ArtifactError> {
        self.decode(T::KIND, T::FORMAT_VERSION)
    }

    /// Verifies digest integrity.
    pub fn verify(&self) -> Result<(), ArtifactError> {
        if self.digest == compute_digest(&self.kind, self.format_version, &self.payload) {
            Ok(())
        } else {
            Err(ArtifactError::DigestMismatch)
        }
    }

    /// Verifies and decodes an exact artifact contract.
    pub fn decode<T: DeserializeOwned>(
        &self,
        expected_kind: &str,
        expected_version: u32,
    ) -> Result<T, ArtifactError> {
        self.verify()?;
        if self.kind != expected_kind || self.format_version != expected_version {
            return Err(ArtifactError::WrongContract {
                expected_kind: expected_kind.to_owned(),
                expected_version,
                actual_kind: self.kind.clone(),
                actual_version: self.format_version,
            });
        }
        serde_json::from_value(self.payload.clone())
            .map_err(|error| ArtifactError::Json(error.to_string()))
    }

    /// Canonical UTF-8 JSON bytes suitable for durable storage.
    #[must_use]
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut output = Vec::new();
        let envelope = Value::Object(serde_json::Map::from_iter([
            ("digest".to_owned(), Value::String(self.digest.clone())),
            (
                "format_version".to_owned(),
                Value::Number(serde_json::Number::from(self.format_version)),
            ),
            ("kind".to_owned(), Value::String(self.kind.clone())),
            ("payload".to_owned(), self.payload.clone()),
        ]));
        write_json(&envelope, &mut output);
        output
    }
}

macro_rules! artifact_contract {
    ($ty:ty, $kind:literal) => {
        impl ArtifactContract for $ty {
            const KIND: &'static str = $kind;
        }
    };
    ($ty:ty, $kind:literal, $version:literal) => {
        impl ArtifactContract for $ty {
            const KIND: &'static str = $kind;
            const FORMAT_VERSION: u32 = $version;
        }
    };
}

artifact_contract!(InterpretedGraph, "artist.formal.interpreted-graph", 3);
artifact_contract!(
    CompiledOntologySubmission,
    "artist.kernel.compiled-ontology",
    3
);
artifact_contract!(ElaborationRecord, "artist.kernel.elaboration", 3);
artifact_contract!(Certificate, "artist.kernel.certificate");
artifact_contract!(DependencyReport, "artist.kernel.dependency-report");
artifact_contract!(CheckSession, "artist.kernel.check-session");
artifact_contract!(UniversalRun, "artist.theories.universal-run");
artifact_contract!(TheoryPackage, "artist.theories.package");
artifact_contract!(Observation, "artist.empirical.observation", 2);
artifact_contract!(InferenceArtifact, "artist.empirical.inference-artifact", 2);
artifact_contract!(PosteriorClaim, "artist.empirical.posterior", 2);
artifact_contract!(
    crate::certification::CertifiedGraphClaim,
    "artist.cognition.certified-claim",
    3
);
artifact_contract!(crate::query::CognitiveQuery, "artist.cognition.query", 4);
artifact_contract!(crate::query::CognitiveAnswer, "artist.cognition.answer", 4);
artifact_contract!(
    crate::query::DiscoveryContinuation,
    "artist.cognition.discovery-continuation",
    3
);
artifact_contract!(
    crate::query::KernelContinuation,
    "artist.cognition.kernel-continuation",
    2
);

fn compute_digest(kind: &str, version: u32, payload: &Value) -> String {
    let mut bytes = Vec::new();
    write_string("artist.artifact/1", &mut bytes);
    write_string(kind, &mut bytes);
    bytes.extend_from_slice(&version.to_be_bytes());
    write_json(payload, &mut bytes);
    blake3::hash(&bytes).to_hex().to_string()
}

fn write_json(value: &Value, output: &mut Vec<u8>) {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
        Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        Value::String(value) => write_json_string(value, output),
        Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_json(value, output);
            }
            output.push(b']');
        }
        Value::Object(values) => {
            output.push(b'{');
            let mut entries: Vec<_> = values.iter().collect();
            entries.sort_by(|left, right| left.0.cmp(right.0));
            for (index, (key, value)) in entries.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_json_string(key, output);
                output.push(b':');
                write_json(value, output);
            }
            output.push(b'}');
        }
    }
}

fn write_json_string(value: &str, output: &mut Vec<u8>) {
    output.push(b'"');
    for character in value.chars() {
        match character {
            '"' => output.extend_from_slice(b"\\\""),
            '\\' => output.extend_from_slice(b"\\\\"),
            '\u{08}' => output.extend_from_slice(b"\\b"),
            '\u{0c}' => output.extend_from_slice(b"\\f"),
            '\n' => output.extend_from_slice(b"\\n"),
            '\r' => output.extend_from_slice(b"\\r"),
            '\t' => output.extend_from_slice(b"\\t"),
            value if value <= '\u{1f}' => {
                let escaped = format!("\\u{:04x}", value as u32);
                output.extend_from_slice(escaped.as_bytes());
            }
            value => {
                let mut buffer = [0; 4];
                output.extend_from_slice(value.encode_utf8(&mut buffer).as_bytes());
            }
        }
    }
    output.push(b'"');
}

fn write_string(value: &str, output: &mut Vec<u8>) {
    output.extend_from_slice(&(value.len() as u64).to_be_bytes());
    output.extend_from_slice(value.as_bytes());
}
