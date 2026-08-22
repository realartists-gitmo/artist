//! Opt-in live smoke test: `OPENAI_API_KEY=... cargo run -p artist-rig --example openai -- gpt-5-mini`.

use std::sync::Arc;

use artist_core::{InitialContext, SessionId, Source, StreamEventKind};
use artist_kernel::SessionHandle;
use artist_rig::RigModel;
use artist_store::MemoryStore;
use rig_core::{
    client::{CompletionClient, ProviderClient},
    providers::openai,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model_name = std::env::args()
        .nth(1)
        .ok_or("usage: openai <streaming-model-name>")?;
    let client = openai::Client::from_env()?;
    let model = Arc::new(RigModel::new(client.completion_model(&model_name)));
    let session = SessionHandle::create(
        SessionId::from("live-smoke"),
        InitialContext { fragments: vec![] },
        Arc::new(MemoryStore::default()),
        model,
    )
    .await?;
    let mut events = session.subscribe();
    session
        .input(Source::User, "Reply with: streaming works")
        .await?;

    loop {
        let event = events.recv().await?;
        match event.kind {
            StreamEventKind::TextDelta { delta } => print!("{delta}"),
            StreamEventKind::Completed { .. } => {
                println!();
                return Ok(());
            }
            StreamEventKind::Failed { failure } => return Err(failure.message.into()),
            _ => {}
        }
    }
}
