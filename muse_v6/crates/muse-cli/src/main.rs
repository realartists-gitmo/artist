//! Standalone command-line interface for Muse semantic packages.

#![forbid(unsafe_code)]

use std::{collections::BTreeSet, env, error::Error, fs, io, path::Path, process::ExitCode};

use muse::{
    classification::{ClassificationRequest, OntologyClassifier, RegistryClassifier},
    core::{ContentDigest, LanguageTag},
    interpretation::{DeterministicInterpreter, InterpretationRequest, SemanticInterpreter},
    io::{SemanticDocument, load_registry_directory, read_envelope, write_document},
    reasoning::{ForwardReasoner, KnowledgeBase, ReasoningOptions, WorldAssumption},
    registry::SemanticPackage,
    resolution::{
        LexicalResolver, ReferenceMode, RegistryResolver, ResolutionPolicy, ResolutionRequest,
    },
    validation::{ValidationOptions, validate_registry},
};

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<ExitCode, Box<dyn Error>> {
    let mut arguments = env::args().skip(1);
    let Some(command) = arguments.next() else {
        print_help();
        return Ok(ExitCode::SUCCESS);
    };

    match command.as_str() {
        "validate" => {
            let directory = required(&mut arguments, "package directory")?;
            let registry = load_registry_directory(&directory)?;
            let snapshot = registry.snapshot_all()?;
            let options = ValidationOptions::default();
            let report = validate_registry(&registry, &snapshot, &options)?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(if report.passed(&options) {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            })
        }
        "snapshot" => {
            let directory = required(&mut arguments, "package directory")?;
            let registry = load_registry_directory(&directory)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&registry.snapshot_all()?)?
            );
            Ok(ExitCode::SUCCESS)
        }
        "classify" => {
            let directory = required(&mut arguments, "package directory")?;
            let request_path = required(&mut arguments, "classification request path")?;
            let registry = load_registry_directory(&directory)?;
            let request: ClassificationRequest = serde_json::from_slice(&fs::read(request_path)?)?;
            let result = RegistryClassifier::new(&registry).classify(&request)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(ExitCode::SUCCESS)
        }
        "reason" => {
            let directory = required(&mut arguments, "package directory")?;
            let knowledge_base_path = required(&mut arguments, "knowledge-base path")?;
            let closed_world = arguments.any(|argument| argument == "--closed-world");
            let registry = load_registry_directory(&directory)?;
            let snapshot = registry.snapshot_all()?;
            let index = registry.ontology_index(&snapshot)?;
            let knowledge_base: KnowledgeBase =
                serde_json::from_slice(&fs::read(knowledge_base_path)?)?;
            let report = ForwardReasoner::new(&index).reason(
                &knowledge_base,
                &ReasoningOptions {
                    world_assumption: if closed_world {
                        WorldAssumption::Closed
                    } else {
                        WorldAssumption::Open
                    },
                    ..ReasoningOptions::default()
                },
            )?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            Ok(if report.conforms() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(3)
            })
        }
        "interpret" => {
            let directory = required(&mut arguments, "package directory")?;
            let request_path = required(&mut arguments, "interpretation request path")?;
            let output_path = required(&mut arguments, "output document path")?;
            let registry = load_registry_directory(&directory)?;
            let request: InterpretationRequest = serde_json::from_slice(&fs::read(request_path)?)?;
            let bundle = DeterministicInterpreter::new(&registry).interpret(&request)?;
            let envelope = write_document(output_path, SemanticDocument::Interpretation(bundle))?;
            println!("{}", envelope.document_digest);
            Ok(ExitCode::SUCCESS)
        }
        "seal-package" => {
            let input_path = required(&mut arguments, "raw package path")?;
            let output_path = required(&mut arguments, "output document path")?;
            let package: SemanticPackage = serde_json::from_slice(&fs::read(input_path)?)?;
            let envelope = write_document(output_path, SemanticDocument::Package(package))?;
            println!("{}", envelope.document_digest);
            Ok(ExitCode::SUCCESS)
        }
        "resolve" => {
            let directory = required(&mut arguments, "package directory")?;
            let language = required(&mut arguments, "language tag")?;
            let surface = arguments.collect::<Vec<_>>().join(" ");
            if surface.is_empty() {
                return Err(invalid_input("missing surface text"));
            }
            let registry = load_registry_directory(&directory)?;
            let snapshot = registry.snapshot_all()?;
            let request = ResolutionRequest {
                surface,
                language: LanguageTag::from(language),
                snapshot,
                domain_hints: BTreeSet::new(),
                context_concepts: BTreeSet::new(),
                reference_mode: ReferenceMode::Use,
                policy: ResolutionPolicy::default(),
            };
            let result = RegistryResolver::new(&registry).resolve(&request)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            Ok(ExitCode::SUCCESS)
        }
        "inspect" => {
            let path = required(&mut arguments, "document path")?;
            let envelope = read_envelope(path)?;
            match &envelope.payload {
                SemanticDocument::Package(package) => {
                    println!("kind: package");
                    println!("package: {}", package.package_ref());
                }
                SemanticDocument::Interpretation(bundle) => {
                    println!("kind: interpretation");
                    println!("id: {}", bundle.id);
                    println!("nodes: {}", bundle.graph.nodes.len());
                    println!("edges: {}", bundle.graph.edges.len());
                    println!("issues: {}", bundle.issues.len());
                }
                SemanticDocument::ValidationReport(report) => {
                    println!("kind: validation_report");
                    println!("diagnostics: {}", report.diagnostics.len());
                }
            }
            println!("document_digest: {}", envelope.document_digest);
            Ok(ExitCode::SUCCESS)
        }
        "digest" => {
            let path = required(&mut arguments, "file path")?;
            let bytes = fs::read(Path::new(&path))?;
            println!("{}", ContentDigest::sha256_bytes(&bytes));
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            print_help();
            Ok(ExitCode::SUCCESS)
        }
        other => Err(invalid_input(format!("unknown command {other:?}"))),
    }
}

fn required(
    arguments: &mut impl Iterator<Item = String>,
    name: &str,
) -> Result<String, Box<dyn Error>> {
    arguments
        .next()
        .ok_or_else(|| invalid_input(format!("missing {name}")))
}

fn invalid_input(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::new(io::ErrorKind::InvalidInput, message.into()))
}

fn print_help() {
    println!(
        "Muse semantic foundation\n\n\
         Usage:\n\
           muse validate <package-directory>\n\
           muse snapshot <package-directory>\n\
           muse classify <package-directory> <request.json>\n\
           muse reason <package-directory> <knowledge-base.json> [--closed-world]\n\
           muse resolve <package-directory> <language-tag> <surface...>\n\
           muse interpret <package-directory> <request.json> <output.muse.json>\n\
           muse seal-package <raw-package.json> <output.muse.json>\n\
           muse inspect <document.muse.json>\n\
           muse digest <file>\n"
    );
}
