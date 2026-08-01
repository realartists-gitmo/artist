//! Flatten a canvas to a file.
//!
//! `cargo run -p artist-canvas --example export -- <project-dir> <slug> <out.html>`
//!
//! The manual counterpart to the `exporting` tests: those assert on the text of
//! the document, and this produces one to actually open.

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args.next().map(std::path::PathBuf::from).expect("project");
    let slug = args.next().unwrap_or_else(|| "demo".to_owned());
    let out = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("canvas.html"));

    let flattened = artist_canvas::export::export(&project, &slug)?;
    std::fs::write(&out, &flattened.html)?;

    println!("{}", out.display());
    println!("modules: {}", flattened.modules.join(", "));
    println!("bytes: {}", flattened.html.len());
    if !flattened.still_online.is_empty() {
        println!("still online: {}", flattened.still_online.join(", "));
    }
    Ok(())
}
