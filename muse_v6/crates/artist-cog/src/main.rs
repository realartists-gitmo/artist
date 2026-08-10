use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::Path;

use artist_cognition::empirical::{InferenceArtifact, Observation, PosteriorClaim};
use artist_cognition::{ArtifactEnvelope, CertifiedGraphClaim, CognitiveAnswer, CognitiveQuery};
use artist_formal::{InterpretedGraph, ObjectGraph, ObjectId};
use artist_kernel::{
    CheckSession, CompiledOntologySubmission, ElaborationRole, Kernel, KernelRequest,
    OntologyCompiler, SessionStatus, Theory,
};
use artist_theories::{
    ComputationCertificate, TheoryPackage, UniversalCertificate, UniversalVerifier,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
struct CheckBundle {
    theory: Theory,
    request: KernelRequest,
}

#[derive(Debug, Deserialize)]
struct OntologyCompileBundle {
    base: Theory,
    submission: InterpretedGraph,
}

#[derive(Debug, Deserialize)]
struct OntologyElaborateBundle {
    base: Theory,
    submission: InterpretedGraph,
    object: ObjectId,
    role: ElaborationRole,
}

#[derive(Debug, Deserialize)]
struct UniversalCheckBundle {
    verifier: UniversalVerifier,
    certificate: UniversalCertificate,
}

#[derive(Debug, Deserialize)]
struct CompiledVerifyBundle {
    submission: InterpretedGraph,
    compiled: CompiledOntologySubmission,
}

#[derive(Debug, Deserialize)]
struct ClaimVerifyBundle {
    submission: InterpretedGraph,
    compiled: CompiledOntologySubmission,
    proof_theory: Theory,
    claim: CertifiedGraphClaim,
}

#[derive(Debug, Deserialize)]
struct InferenceVerifyBundle {
    submission: InterpretedGraph,
    compiled: CompiledOntologySubmission,
    proof_theory: Theory,
    artifact: InferenceArtifact,
}

#[derive(Debug, Deserialize)]
struct AnswerBundle {
    query: CognitiveQuery,
    answer: CognitiveAnswer,
}

#[derive(Debug, Deserialize)]
struct AnswerVerifyBundle {
    query: CognitiveQuery,
    answer: CognitiveAnswer,
    compiled: CompiledOntologySubmission,
    proof_theory: Theory,
}

#[derive(Debug, Serialize)]
struct SliceOutput<'a> {
    status: SessionStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    checkpoint: Option<&'a CheckSession>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("artist-cog: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("check") => {
            let path = required_path(args.next(), "check bundle")?;
            let steps = parse_steps(args.next())?;
            let bundle: CheckBundle = read_json(&path)?;
            let mut session = Kernel::start(bundle.theory, bundle.request);
            emit_slice(&mut session, steps)?;
        }
        Some("resume") => {
            let path = required_path(args.next(), "checkpoint")?;
            let steps = parse_steps(args.next())?;
            let mut session: CheckSession = read_json(&path)?;
            emit_slice(&mut session, steps)?;
        }
        Some("graph-validate") => {
            let path = required_path(args.next(), "graph")?;
            let graph: ObjectGraph = read_json(&path)?;
            graph.validate()?;
            print_json(&serde_json::json!({
                "valid": true,
                "hash": graph.canonical_hash(),
                "roots": graph.roots,
            }))?;
        }
        Some("interpreted-validate") => {
            let path = required_path(args.next(), "interpreted graph")?;
            let submission: InterpretedGraph = read_json(&path)?;
            submission.validate()?;
            print_json(&serde_json::json!({
                "valid": true,
                "submission": submission.canonical_hash(),
                "graph": submission.graph.canonical_hash(),
                "ontology": submission.ontology.canonical_hash(),
                "typing_diagnostics": submission.typing_diagnostics(),
            }))?;
        }
        Some("ontology-compile") => {
            let path = required_path(args.next(), "ontology compile bundle")?;
            let bundle: OntologyCompileBundle = read_json(&path)?;
            let compiled = OntologyCompiler::compile(bundle.base, &bundle.submission)?;
            print_json(&compiled)?;
        }
        Some("ontology-elaborate") => {
            let path = required_path(args.next(), "ontology elaboration bundle")?;
            let bundle: OntologyElaborateBundle = read_json(&path)?;
            let compiled = OntologyCompiler::compile(bundle.base, &bundle.submission)?;
            let elaboration = OntologyCompiler::elaborate(
                &bundle.submission,
                &compiled,
                &bundle.object,
                bundle.role,
            )?;
            print_json(&serde_json::json!({
                "compiled": compiled,
                "elaboration": elaboration,
            }))?;
        }
        Some("compiled-verify") => {
            let path = required_path(args.next(), "compiled ontology bundle")?;
            let bundle: CompiledVerifyBundle = read_json(&path)?;
            OntologyCompiler::verify_compiled(&bundle.submission, &bundle.compiled)?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("claim-verify") => {
            let path = required_path(args.next(), "certified claim bundle")?;
            let bundle: ClaimVerifyBundle = read_json(&path)?;
            bundle
                .claim
                .verify(&bundle.submission, &bundle.compiled, &bundle.proof_theory)?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("query-validate") => {
            let path = required_path(args.next(), "cognitive query")?;
            let query: CognitiveQuery = read_json(&path)?;
            query.validate()?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("answer-validate") => {
            let path = required_path(args.next(), "query answer bundle")?;
            let bundle: AnswerBundle = read_json(&path)?;
            bundle.query.validate_answer(&bundle.answer)?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("answer-verify") => {
            let path = required_path(args.next(), "verified query answer bundle")?;
            let bundle: AnswerVerifyBundle = read_json(&path)?;
            bundle
                .query
                .verify_answer(&bundle.answer, &bundle.compiled, &bundle.proof_theory)?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("observation-validate") => {
            let path = required_path(args.next(), "observation")?;
            let observation: Observation = read_json(&path)?;
            observation.validate()?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("posterior-validate") => {
            let path = required_path(args.next(), "posterior claim")?;
            let posterior: PosteriorClaim = read_json(&path)?;
            posterior.validate()?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("inference-verify") => {
            let path = required_path(args.next(), "empirical inference bundle")?;
            let bundle: InferenceVerifyBundle = read_json(&path)?;
            let classification = bundle.artifact.verify(
                &bundle.submission,
                &bundle.compiled,
                &bundle.proof_theory,
            )?;
            print_json(&serde_json::json!({
                "valid": true,
                "classification": classification,
            }))?;
        }
        Some("universal-check") => {
            let path = required_path(args.next(), "universal verifier bundle")?;
            let bundle: UniversalCheckBundle = read_json(&path)?;
            bundle.verifier.check(&bundle.certificate)?;
            print_json(&serde_json::json!({ "accepted": true }))?;
        }
        Some("computation-check") => {
            let path = required_path(args.next(), "computation certificate")?;
            let certificate: ComputationCertificate = read_json(&path)?;
            certificate.check()?;
            print_json(&serde_json::json!({ "valid": true }))?;
        }
        Some("package-validate") => {
            let path = required_path(args.next(), "theory package")?;
            let package: TheoryPackage = read_json(&path)?;
            package.validate()?;
            print_json(&serde_json::json!({
                "valid": true,
                "package": package.id()?,
                "theory": package.kernel.id(),
            }))?;
        }
        Some("artifact-verify") => {
            let path = required_path(args.next(), "artifact envelope")?;
            let artifact: ArtifactEnvelope = read_json(&path)?;
            artifact.verify()?;
            print_json(&serde_json::json!({
                "valid": true,
                "kind": artifact.kind,
                "format_version": artifact.format_version,
                "digest": artifact.digest,
            }))?;
        }
        Some("help") | None => print_help(),
        Some(command) => return Err(format!("unknown command {command:?}").into()),
    }
    Ok(())
}

fn required_path(
    value: Option<String>,
    description: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    value.ok_or_else(|| format!("missing {description} path").into())
}

fn parse_steps(value: Option<String>) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(match value {
        Some(value) => value.parse()?,
        None => 10_000,
    })
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &str) -> Result<T, Box<dyn std::error::Error>> {
    Ok(serde_json::from_slice(&fs::read(Path::new(path))?)?)
}

fn print_json<T: Serialize>(value: &T) -> Result<(), Box<dyn std::error::Error>> {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    serde_json::to_writer_pretty(&mut lock, value)?;
    writeln!(lock)?;
    Ok(())
}

fn emit_slice(session: &mut CheckSession, steps: u64) -> Result<(), Box<dyn std::error::Error>> {
    let status = session.run_slice(steps);
    let checkpoint = matches!(&status, SessionStatus::Running { .. }).then_some(&*session);
    print_json(&SliceOutput { status, checkpoint })
}

fn print_help() {
    println!(
        "artist-cog\n\n\
         check <bundle.json> [steps]             Start a bounded kernel slice\n\
         resume <checkpoint.json> [steps]        Resume an exact checker continuation\n\
         graph-validate <graph.json>             Validate and hash open graph syntax\n\
         interpreted-validate <submission.json>  Validate exact ontology coverage\n\
         ontology-compile <bundle.json>           Compile ontology and quote constants into DTT\n\
         ontology-elaborate <bundle.json>         Compile and elaborate one exact graph object\n\
         compiled-verify <bundle.json>            Recompute and verify a compiled submission\n\
         claim-verify <bundle.json>               Verify a complete graph-to-DTT proof chain\n\
         query-validate <query.json>              Validate a complete cognitive query\n\
         answer-validate <bundle.json>            Validate answer identity and structural contract\n\
         answer-verify <bundle.json>              Verify every certificate in an exact answer\n\
         observation-validate <observation.json> Validate immutable observation structure\n\
         posterior-validate <posterior.json>     Validate posterior data and method provenance\n\
         inference-verify <bundle.json>          Verify exact empirical claim certification\n\
         universal-check <bundle.json>           Check a finite universal verifier trace\n\
         computation-check <certificate.json>    Check a finite computation trace\n\
         package-validate <package.json>         Validate a complete theory package\n\
         artifact-verify <artifact.json>         Verify a canonical artifact envelope\n"
    );
}
