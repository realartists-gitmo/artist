//! Host invocation adapter for a composition WASM generation.

use artist_kernel::ResourceUri;
use artist_wasm::{GenerationHandle, HostEnvironment, RuntimeStore};
use artist_wasm_composition::{extension, resource_host, types};
use wasmtime::component::Linker;

#[derive(Clone)]
pub struct WasmComposition {
    generation: GenerationHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionUpdate {
    Context(artist_session::ContextEvent),
    ToolAvailable {
        name: String,
        description: Option<String>,
        input_schema: String,
    },
    ToolUnavailable {
        name: String,
        reason: Option<String>,
    },
}

impl WasmComposition {
    pub fn new(generation: GenerationHandle) -> Self {
        Self { generation }
    }

    pub async fn initial(&self, input: types::SessionInput) -> anyhow::Result<types::Snapshot> {
        let (mut store, guest) = self.instance().await?;
        guest
            .call_initial(&mut store, &input)
            .await?
            .map_err(|error| anyhow::anyhow!("composition extension failed: {error:?}"))
    }

    /// Ask the extension to reconcile its current policy inputs. The returned
    /// events are converted into the provider-neutral session contract at this
    /// boundary; provider adapters never need to understand WIT values.
    pub async fn update(
        &self,
        input: types::SessionInput,
    ) -> anyhow::Result<Vec<CompositionUpdate>> {
        let (mut store, guest) = self.instance().await?;
        let events = guest
            .call_update(&mut store, &input)
            .await?
            .map_err(|error| anyhow::anyhow!("composition extension failed: {error:?}"))?;
        events.into_iter().map(composition_update).collect()
    }

    async fn instance(
        &self,
    ) -> anyhow::Result<(wasmtime::Store<artist_wasm::RuntimeStore>, extension::Guest)> {
        let lease = self
            .generation
            .pin()
            .ok_or_else(|| anyhow::anyhow!("composition generation is unavailable"))?;
        let mut store = lease.store()?;
        let mut linker = Linker::new(lease.component().engine());
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        resource_host::add_to_linker::<_, CompositionHostMarker>(&mut linker, composition_host)?;
        let pre = linker.instantiate_pre(lease.component())?;
        let indices = extension::GuestIndices::new(&pre)?;
        let instance = pre.instantiate_async(&mut store).await?;
        let guest = indices.load(&mut store, &instance)?;
        Ok((store, guest))
    }
}

fn composition_update(event: types::CompositionUpdate) -> anyhow::Result<CompositionUpdate> {
    Ok(match event {
        types::CompositionUpdate::Context(event) => CompositionUpdate::Context(match event {
            types::Event::Replace(contribution) => artist_session::ContextEvent::Replace {
                contribution: contribution_to_session(contribution),
            },
            types::Event::Remove(id) => artist_session::ContextEvent::Remove { id },
            types::Event::Append(contribution) => artist_session::ContextEvent::Append {
                contribution: contribution_to_session(contribution),
            },
        }),
        types::CompositionUpdate::Tool(event) => match event {
            types::ToolEvent::Available(definition) => CompositionUpdate::ToolAvailable {
                name: definition.name,
                description: definition.description,
                input_schema: definition.input_schema,
            },
            types::ToolEvent::Unavailable(unavailable) => CompositionUpdate::ToolUnavailable {
                name: unavailable.name,
                reason: unavailable.reason,
            },
        },
    })
}

fn contribution_to_session(value: types::Contribution) -> artist_session::Contribution {
    artist_session::Contribution {
        id: value.id,
        source: value.source,
        slot: value.slot,
        order: value.order,
        revision: value.revision,
        content: value.content,
    }
}

struct CompositionHostMarker;
struct CompositionHostContext {
    host: std::sync::Arc<dyn HostEnvironment>,
}

impl wasmtime::component::HasData for CompositionHostMarker {
    type Data<'a> = CompositionHostContext;
}

fn composition_host(store: &mut RuntimeStore) -> CompositionHostContext {
    CompositionHostContext {
        host: std::sync::Arc::clone(&store.host),
    }
}

impl resource_host::Host for CompositionHostContext {
    async fn read(&mut self, uri: String) -> wasmtime::Result<Result<String, types::Error>> {
        let uri: ResourceUri = uri.parse().map_err(|_| types::Error::InvalidInput)?;
        let bytes = self
            .host
            .read(&uri, 0, u32::MAX)
            .await
            .map_err(|_| types::Error::Unavailable)?;
        String::from_utf8(bytes)
            .map(Ok)
            .map_err(|_| types::Error::Failed.into())
    }
}
