use artist_formal::{GraphBuilder, GraphError, Literal, ObjectGraph, ObjectId, ObjectNode, Symbol};
use thiserror::Error;

use crate::{Certificate, CheckSession};

const ROOT: &str = "main";
const SYMBOL: &str = "artist.core/symbol";
const PAYLOAD: &str = "artist.core/payload";
const CERTIFICATE: &str = "artist.kernel/certificate";
const CONTINUATION: &str = "artist.kernel/continuation";

/// Errors while storing or restoring certificates and continuations in the open graph.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum KernelArtifactCodecError {
    /// Malformed graph references.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// The standard artifact root is absent.
    #[error("kernel artifact graph has no main root")]
    MissingRoot,
    /// The root has no operator.
    #[error("kernel artifact root {0} has no operator")]
    MissingOperator(ObjectId),
    /// The operator does not carry the standard symbol property.
    #[error("kernel artifact operator {0} is malformed")]
    MalformedOperator(ObjectId),
    /// The graph contains another artifact kind.
    #[error("expected {expected}, found {actual}")]
    WrongKind {
        expected: &'static str,
        actual: String,
    },
    /// The serialized byte payload is absent or malformed.
    #[error("kernel artifact root {0} has no byte payload")]
    MissingPayload(ObjectId),
    /// JSON serialization or deserialization failed.
    #[error("kernel artifact JSON failed: {0}")]
    Json(String),
}

/// Standard language-level storage envelope for kernel artifacts.
#[derive(Clone, Copy, Debug, Default)]
pub struct KernelArtifactCodec;

impl KernelArtifactCodec {
    /// Encodes an exact theory-relative certificate as an open-graph object.
    pub fn encode_certificate(
        certificate: &Certificate,
    ) -> Result<ObjectGraph, KernelArtifactCodecError> {
        let payload = serde_json::to_vec(certificate)
            .map_err(|error| KernelArtifactCodecError::Json(error.to_string()))?;
        encode_payload(CERTIFICATE, payload)
    }

    /// Restores a certificate from the standard open-graph object.
    pub fn decode_certificate(
        graph: &ObjectGraph,
    ) -> Result<Certificate, KernelArtifactCodecError> {
        let payload = decode_payload(graph, CERTIFICATE)?;
        serde_json::from_slice(payload)
            .map_err(|error| KernelArtifactCodecError::Json(error.to_string()))
    }

    /// Encodes an exact resumable checker continuation as an open-graph object.
    pub fn encode_continuation(
        continuation: &CheckSession,
    ) -> Result<ObjectGraph, KernelArtifactCodecError> {
        let payload = serde_json::to_vec(continuation)
            .map_err(|error| KernelArtifactCodecError::Json(error.to_string()))?;
        encode_payload(CONTINUATION, payload)
    }

    /// Restores a continuation. Its embedded theory is deliberately revalidated
    /// before the next transition by [`CheckSession::run_slice`].
    pub fn decode_continuation(
        graph: &ObjectGraph,
    ) -> Result<CheckSession, KernelArtifactCodecError> {
        let payload = decode_payload(graph, CONTINUATION)?;
        serde_json::from_slice(payload)
            .map_err(|error| KernelArtifactCodecError::Json(error.to_string()))
    }
}

fn encode_payload(
    kind: &'static str,
    payload: Vec<u8>,
) -> Result<ObjectGraph, KernelArtifactCodecError> {
    let mut builder = GraphBuilder::new();
    let operator = builder.insert(
        format!("op:{kind}"),
        ObjectNode::new().with_property(SYMBOL, Literal::Text(kind.to_owned())),
    );
    let artifact = builder.alloc(
        ObjectNode::new()
            .with_operator(operator)
            .with_property(PAYLOAD, Literal::Bytes(payload)),
    );
    builder.root(ROOT, artifact);
    Ok(builder.finish()?)
}

fn decode_payload<'a>(
    graph: &'a ObjectGraph,
    expected: &'static str,
) -> Result<&'a [u8], KernelArtifactCodecError> {
    graph.validate()?;
    let root_id = graph
        .roots
        .get(&Symbol::from(ROOT))
        .ok_or(KernelArtifactCodecError::MissingRoot)?;
    let root = &graph.nodes[root_id];
    let operator_id = root
        .operator
        .as_ref()
        .ok_or_else(|| KernelArtifactCodecError::MissingOperator(root_id.clone()))?;
    let operator = &graph.nodes[operator_id];
    let Some(Literal::Text(actual)) = operator.properties.get(&Symbol::from(SYMBOL)) else {
        return Err(KernelArtifactCodecError::MalformedOperator(
            operator_id.clone(),
        ));
    };
    if actual != expected {
        return Err(KernelArtifactCodecError::WrongKind {
            expected,
            actual: actual.clone(),
        });
    }
    match root.properties.get(&Symbol::from(PAYLOAD)) {
        Some(Literal::Bytes(payload)) => Ok(payload),
        _ => Err(KernelArtifactCodecError::MissingPayload(root_id.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Context, Kernel, KernelRequest, Provenance, Term, TheoryBuilder};

    fn theory() -> crate::Theory {
        let mut builder = TheoryBuilder::new("artifact", "1");
        builder
            .axiom("A", Term::universe(0), Provenance::new("fixture"))
            .unwrap();
        builder.finish()
    }

    #[test]
    fn certificate_roundtrips_as_an_open_graph_object() {
        let theory = theory();
        let certificate = Certificate::new(
            &theory,
            Context::new(),
            Term::universe(0),
            Term::constant("A"),
        );
        let graph = KernelArtifactCodec::encode_certificate(&certificate).unwrap();
        assert_eq!(
            KernelArtifactCodec::decode_certificate(&graph).unwrap(),
            certificate
        );
    }

    #[test]
    fn continuation_roundtrips_as_an_open_graph_object() {
        let session = Kernel::start(
            theory(),
            KernelRequest::Infer {
                context: Context::new(),
                term: Term::constant("A"),
            },
        );
        let before = serde_json::to_value(&session).unwrap();
        let graph = KernelArtifactCodec::encode_continuation(&session).unwrap();
        let restored = KernelArtifactCodec::decode_continuation(&graph).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap(), before);
    }

    #[test]
    fn artifact_kinds_cannot_be_confused() {
        let session = Kernel::start(
            theory(),
            KernelRequest::Infer {
                context: Context::new(),
                term: Term::constant("A"),
            },
        );
        let graph = KernelArtifactCodec::encode_continuation(&session).unwrap();
        assert!(matches!(
            KernelArtifactCodec::decode_certificate(&graph),
            Err(KernelArtifactCodecError::WrongKind { .. })
        ));
    }
}
