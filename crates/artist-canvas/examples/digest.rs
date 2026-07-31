//! Ask a live page what it is showing.
//! `cargo run -p artist-canvas --example digest -- <project> <slug>`
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args.next().map(std::path::PathBuf::from).expect("project");
    let slug = args.next().unwrap_or_else(|| "demo".to_owned());

    let server = artist_canvas::server::Server::start(project).await?;
    println!("{}", server.url(&slug));
    // Give a browser time to be pointed at it.
    tokio::time::sleep(std::time::Duration::from_secs(12)).await;
    match server.request_digest(&slug).await {
        Some(digest) => println!("DIGEST {}", serde_json::to_string_pretty(&digest)?),
        None => println!("DIGEST none — no page open"),
    }
    Ok(())
}
