//! Flatten a canvas to a file.
//!
//! `cargo run -p artist-canvas --example export -- <project-dir> <slug> <out.html>`
//!
//! The manual counterpart to the `exporting` tests: those assert on the text of
//! the document, and this produces one to actually open.

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args.next().map(std::path::PathBuf::from).expect("project");
    let slug = args.next().unwrap_or_else(|| "demo".to_owned());
    let out = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("canvas.html"));

    // `*` for the whole project, which is a different document: one realm with
    // every canvas in it rather than one canvas alone.
    if slug == "*" {
        let set = artist_canvas::export::export_project(&project).await?;
        std::fs::write(&out, &set.html)?;
        println!("{}", out.display());
        println!("canvases: {}", set.canvases.join(", "));
        println!("bytes: {}", set.html.len());
        if !set.dependencies.is_empty() {
            println!("inlined deps: {}", set.dependencies.join(", "));
        }
        return Ok(());
    }

    let flattened = artist_canvas::export::export(&project, &slug).await?;
    std::fs::write(&out, &flattened.html)?;

    println!("{}", out.display());
    println!("modules: {}", flattened.modules.join(", "));
    println!("bytes: {}", flattened.html.len());
    if !flattened.dependencies.is_empty() {
        println!("inlined deps: {}", flattened.dependencies.join(", "));
    }
    Ok(())
}
