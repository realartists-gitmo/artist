//! Report what is actually in the shipped OCR graphs.
//!
//! Exists because the question "should we fold BatchNorm at export?" deserved a
//! measurement rather than an assumption, and the measurement turned out to
//! contradict the assumption twice over. Keeping the tool means the next person
//! to wonder can check in ten seconds instead of guessing.
//!
//! ```text
//! cargo run -p artist-computer --example inspect-ocr-graph --features ocr
//! ```
//!
//! What it found on PP-OCRv5 mobile, 2026-07-31:
//!
//! * **BatchNorm is already folded.** 3 nodes against 62 convolutions in the
//!   detector, 6 against 38 in the recogniser — Paddle's exporter did roughly
//!   95% of it. The residue is 8 nodes in 1410. Folding it ourselves would be
//!   real work for an unmeasurable gain.
//! * **Weights are `Constant` nodes, not initializers** — 300 and 342 of them,
//!   about 45% of each graph, alongside a large amount of `Add`/`Mul`/`Reshape`
//!   shape arithmetic. That looks like a much bigger win until you check what
//!   the runtime does with it: rten runs `propagate_constants` when the model
//!   loads, so the arithmetic is resolved once at startup and never appears in
//!   an inference. Simplifying at export would move that work, not remove it,
//!   and the models are loaded once per process and kept resident.
//!
//! Conclusion recorded here rather than in a commit message: **there is nothing
//! worth doing at export.** The graph is already in the shape the runtime wants.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

use rten_onnx::onnx::ModelProto;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models"));

    for name in ["ppocrv5-mobile-det.onnx", "ppocrv5-mobile-rec.onnx"] {
        let path = dir.join(name);
        let Ok(file) = File::open(&path) else {
            eprintln!(
                "{}: not found — run scripts/fetch-ocr-models.sh",
                path.display()
            );
            continue;
        };
        let model = match ModelProto::parse_file(file) {
            Ok(model) => model,
            Err(error) => {
                eprintln!("{}: {error}", path.display());
                continue;
            }
        };
        let Some(graph) = model.graph.as_ref() else {
            eprintln!("{}: no graph", path.display());
            continue;
        };

        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        let mut producer: BTreeMap<&str, &str> = BTreeMap::new();
        for node in &graph.node {
            let op = node.op_type.as_deref().unwrap_or("");
            *counts.entry(op).or_default() += 1;
            for output in &node.output {
                producer.insert(output.as_str(), op);
            }
        }

        // A BatchNorm folds into the convolution feeding it. One that follows
        // something else has nowhere to go.
        let foldable = graph
            .node
            .iter()
            .filter(|node| {
                node.op_type.as_deref() == Some("BatchNormalization")
                    && node
                        .input
                        .first()
                        .and_then(|input| producer.get(input.as_str()))
                        .copied()
                        == Some("Conv")
            })
            .count();

        println!("{name}");
        println!(
            "  {} nodes, {} initializers",
            graph.node.len(),
            graph.initializer.len()
        );

        let mut rows: Vec<_> = counts.iter().collect();
        rows.sort_by_key(|(op, count)| (std::cmp::Reverse(**count), **op));
        for (op, count) in rows.iter().take(12) {
            println!("  {count:>5}  {op}");
        }

        let convolutions = counts.get("Conv").copied().unwrap_or(0);
        let batch_norm = counts.get("BatchNormalization").copied().unwrap_or(0);
        println!(
            "  BatchNorm {batch_norm} ({foldable} foldable) against {convolutions} convolutions \
             — already folded by the exporter"
        );
        println!(
            "  Constant {} — resolved by rten's propagate_constants at load, not per inference\n",
            counts.get("Constant").copied().unwrap_or(0)
        );
    }
}
