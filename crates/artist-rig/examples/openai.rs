//! Opt-in live smoke test: `OPENAI_API_KEY=... cargo run -p artist-rig --example openai -- gpt-5-mini`.

use std::sync::Arc;

use artist_core::{
    InitialContext, ProfilePolicy, ProfileSnapshot, SessionId, Source, StreamEventKind,
    default_yield_schema,
};
use artist_kernel::{ProfileSource, SessionHandle};
use artist_rig::RigModel;
use artist_store::MemoryStore;
use rig_core::{
    client::{CompletionClient, ProviderClient},
    providers::openai,
};

struct SmokeProfile;

#[async_trait::async_trait]
impl ProfileSource for SmokeProfile {
    async fn load(&self, name: &str) -> Result<ProfileSnapshot, String> {
        Ok(ProfileSnapshot {
            name: name.into(),
            instructions: "Run the live streaming smoke test.".into(),
            yield_schema: default_yield_schema(),
            policy: ProfilePolicy::default(),
            models: Vec::new(),
            catalog: vec![name.into()],
        })
    }
}

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
        "smoke",
        Arc::new(SmokeProfile),
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
