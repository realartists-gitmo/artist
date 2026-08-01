//! Scaffold every template into a directory, for manual render checks.
//!
//! Temporary harness: `cargo run -p artist-canvas --example scaffold_all -- <dir>`

fn main() -> anyhow::Result<()> {
    let root = std::env::args()
        .nth(1)
        .map(std::path::PathBuf::from)
        .expect("usage: scaffold_all <dir>");
    std::fs::create_dir_all(&root)?;
    for template in artist_canvas::TEMPLATES {
        let canvas = artist_canvas::registry::scaffold(
            &root,
            template.name,
            &format!("{} probe", template.name),
            template,
        )?;
        println!("{} -> {}", template.name, canvas.root.display());
    }
    Ok(())
}
