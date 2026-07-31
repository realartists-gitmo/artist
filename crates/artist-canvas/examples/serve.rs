//! Serve a project's canvases until interrupted.
//!
//! `cargo run -p artist-canvas --example serve -- <project-dir> <slug>`
//!
//! This is the manual harness for the parts a unit test cannot reach: whether
//! the vendored bundles actually resolve through the import map and render.

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let slug = args.next().unwrap_or_else(|| "demo".to_owned());

    let server = artist_canvas::server::Server::start(project).await?;
    println!("{}", server.url(&slug));

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        for report in server.take_reports(None) {
            println!("[{}] {}: {}", report.slug, report.level, report.message);
        }
    }
}
