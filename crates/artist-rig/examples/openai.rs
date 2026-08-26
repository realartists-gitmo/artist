//! Opt-in live smoke test through the provider contract:
//! `OPENAI_API_KEY=... cargo run -p artist-rig --example openai -- gpt-5-mini`.
//!
//! The application never constructs a model object: it installs an OpenAI
//! provider driver plus one account, then lets the profile router select and
//! open the streaming model per request — exactly how production runtimes
//! consume installed provider plugins.

use std::sync::Arc;

use artist_core::{
    InitialContext, ModelRoute, ProfilePolicy, ProfileSnapshot, SessionId, Source, StreamEventKind,
    default_yield_schema,
};
use artist_kernel::ProfileSource;
use artist_provider::{
    AccountDescriptor, Credential, CredentialStore, MemoryCredentialStore, NativeRigDriver,
    ProviderDescriptor, ProviderId, ProviderRegistry, Secret,
};
use artist_rig::RigModel;
use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::providers::openai;

struct RoutedProfile {
    route: ModelRoute,
}

#[async_trait::async_trait]
impl ProfileSource for RoutedProfile {
    async fn load(&self, name: &str) -> Result<ProfileSnapshot, String> {
        Ok(ProfileSnapshot {
            name: name.into(),
            instructions: "Run the live streaming smoke test.".into(),
            yield_schema: default_yield_schema(),
            policy: ProfilePolicy::default(),
            models: vec![self.route.clone()],
            catalog: vec![name.into()],
        })
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model_name = std::env::args()
        .nth(1)
        .ok_or("usage: openai <streaming-model-name>")?;
    let api_key = std::env::var("OPENAI_API_KEY")?;

    // Install the provider implementation and its account. In production
    // these come from activated provider plugins and the durable account
    // store; the shape of the contract is identical.
    let client = openai::Client::from_env()?;
    let credentials = Arc::new(MemoryCredentialStore::default());
    credentials
        .put(
            "env:openai",
            Credential {
                kind: "api-key".into(),
                secret: Secret::new(api_key),
                expires_at: None,
                refresh: None,
                private: Default::default(),
            },
        )
        .await?;
    let registry = Arc::new(ProviderRegistry::new(credentials));
    let driver_model = model_name.clone();
    registry.install(
        ProviderDescriptor {
            id: ProviderId("openai".into()),
            revision: "live-example".into(),
            models: vec![model_name.to_string(), "gpt-*".into(), "o*".into()],
            capabilities: vec![artist_provider::Capability::new("streaming")],
            auth_kinds: vec!["api-key".into()],
            api_variants: vec!["openai".into()],
            parameters: serde_json::json!({}),
        },
        Arc::new(NativeRigDriver::new(move |_, model, _| {
            let client = client.clone();
            Ok(Arc::new(RigModel::new(client.completion_model(model))) as Arc<_>)
        })),
    )?;
    let model_for_route = driver_model.clone();
    registry.upsert_account(AccountDescriptor {
        id: "env".into(),
        provider: ProviderId("openai".into()),
        credential_ref: "env:openai".into(),
        credential_kind: "api-key".into(),
        api_variant: "openai".into(),
        default_model: model_name,
        default_reasoning: None,
        metadata: Default::default(),
    })?;

    let route = ModelRoute {
        provider: "openai".into(),
        account: Some("env".into()),
        api_variant: Some("openai".into()),
        model: model_for_route,
        reasoning: None,
        parameters: serde_json::json!({}),
    };
    let profiles = Arc::new(RoutedProfile { route });

    // Production construction: the router resolves every request from the
    // installed provider source; no hand-built model reaches the session.
    let runtime = artist_runtime::SessionRuntime::from_provider_source(
        Arc::new(artist_store::MemoryStore::default()),
        Arc::new(artist_runtime::MemoryMetadataStore::default()),
        registry,
        profiles,
    );
    let session_id = SessionId::from("live-smoke");
    runtime
        .create(artist_runtime::CreateRequest {
            request_id: "smoke-1".into(),
            session_id: session_id.clone(),
            context: InitialContext { fragments: vec![] },
            metadata: artist_core::SessionMetadata {
                created_at_ms: 0,
                creator_plugin_id: None,
                lineage: None,
                initial_profile: Some("smoke".into()),
                attachment: artist_runtime::Attachment::Detached,
                recovery_policy: artist_runtime::RecoveryPolicy::RemainInterrupted,
            },
        })
        .await?;

    let mut events = runtime.subscribe();
    runtime
        .send(&session_id, Source::User, "Reply with: streaming works")
        .await?;

    loop {
        let event = events.recv().await?;
        let kind = match event {
            artist_runtime::RuntimeEvent::Stream(stream) => stream.kind,
            artist_runtime::RuntimeEvent::Replay { .. }
            | artist_runtime::RuntimeEvent::Lagged { .. } => continue,
        };
        match kind {
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
