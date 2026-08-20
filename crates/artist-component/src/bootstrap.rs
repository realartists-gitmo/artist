//! Process bootstrap for the component architecture.
//!
//! This is the narrow native boundary: construct the kernel and Wasmtime
//! runtime, install the resource catalog, load the configured URL composition,
//! and expose the live session-local tool registry. Consumers do not assemble
//! these pieces independently.

use std::collections::BTreeSet;
use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::Context;
use artist_kernel::Kernel;
use artist_session::{Contribution, Snapshot};
use artist_wasm::{ExtensionManager, KernelHostEnvironment, Runtime, build_engine};
use artist_wasm_composition::types;
use artist_wasm_nouns::{RoutedNoun, WasmRoutedNoun};

use crate::{ComponentToolRegistry, ExtensionCatalog, UrlComposition, UrlCompositionSource};

pub struct ComponentHost {
    kernel: Arc<Kernel>,
    tools: ComponentToolRegistry,
    composition: Arc<UrlComposition>,
    composition_extension: Arc<RwLock<crate::WasmComposition>>,
    noun_providers: Arc<RwLock<BTreeSet<String>>>,
}

impl ComponentHost {
    pub async fn start(kernel: Arc<Kernel>, source: UrlCompositionSource) -> anyhow::Result<Self> {
        let engine = build_engine().context("build component engine")?;
        let catalog = ExtensionCatalog::default();
        catalog.install_into(&kernel);
        let runtime = Arc::new(Runtime::new(
            engine.clone(),
            Arc::new(KernelHostEnvironment::new(Arc::clone(&kernel))),
        ));
        let root = source
            .global
            .parent()
            .map(|parent| parent.join("system/root.wasm"));
        if let Some(root) = root.filter(|path| path.is_file()) {
            let bytes = tokio::fs::read(&root)
                .await
                .with_context(|| format!("read trusted root {}", root.display()))?;
            runtime
                .instantiate_root(&bytes)
                .await
                .with_context(|| format!("instantiate trusted root {}", root.display()))?;
        }
        let composition = Arc::new(
            UrlComposition::new(source, engine, ExtensionManager::new(runtime), catalog)
                .with_tools(ComponentToolRegistry::new()),
        );
        let handles = composition
            .reload()
            .await
            .context("load component composition")?;
        let noun_providers = Self::publish_nouns(&kernel, &handles, &BTreeSet::new());
        let composition_handles: Vec<_> = handles
            .into_iter()
            .filter(|handle| handle.class().composition)
            .collect();
        if composition_handles.len() != 1 {
            return Err(anyhow::anyhow!(
                "expected exactly one composition extension, found {}",
                composition_handles.len()
            ));
        }
        let composition_handle = composition_handles
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no composition extension is active"))?;
        let composition_extension = crate::WasmComposition::new(composition_handle);
        let tools = composition
            .tools()
            .expect("component host owns a tool registry");
        Ok(Self {
            kernel,
            tools,
            composition,
            composition_extension: Arc::new(RwLock::new(composition_extension)),
            noun_providers: Arc::new(RwLock::new(noun_providers)),
        })
    }

    pub fn kernel(&self) -> &Arc<Kernel> {
        &self.kernel
    }
    pub fn tools(&self) -> ComponentToolRegistry {
        self.tools.clone()
    }
    pub fn composition(&self) -> &Arc<UrlComposition> {
        &self.composition
    }

    /// Atomically refresh the active extension package set and the host's
    /// composition adapters. `UrlComposition::reload` preserves the last
    /// valid generation when callers choose to invoke this from a watcher.
    pub async fn reload(&self) -> anyhow::Result<()> {
        let handles = self
            .composition
            .reload()
            .await
            .context("reload component composition")?;
        let old = self.noun_providers.read().await.clone();
        let next = Self::publish_nouns(&self.kernel, &handles, &old);
        *self.noun_providers.write().await = next;
        let composition_handles: Vec<_> = handles
            .iter()
            .filter(|handle| handle.class().composition)
            .collect();
        if composition_handles.len() != 1 {
            return Err(anyhow::anyhow!(
                "expected exactly one composition extension, found {}",
                composition_handles.len()
            ));
        }
        let composition_handle = composition_handles
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no composition extension is active"))?;
        *self.composition_extension.write().await =
            crate::WasmComposition::new(composition_handle.clone());
        Ok(())
    }

    fn publish_nouns(
        kernel: &Arc<Kernel>,
        handles: &[artist_wasm::GenerationHandle],
        old: &BTreeSet<String>,
    ) -> BTreeSet<String> {
        for name in old {
            kernel.unregister_resource_provider(name);
        }
        let mut next = BTreeSet::new();
        for handle in handles.iter().filter(|handle| handle.class().noun) {
            let name = format!("noun:{}", handle.name());
            kernel.register_resource_provider(RoutedNoun::new(
                name.clone(),
                std::sync::Arc::new(WasmRoutedNoun::new(handle.clone())),
            ));
            next.insert(name);
        }
        next
    }

    pub async fn compose_initial(
        &self,
        mut input: types::SessionInput,
    ) -> anyhow::Result<Snapshot> {
        if input.system.is_none() {
            let uri: artist_kernel::ResourceUri = "prompt://SYSTEM.md".parse()?;
            if let Ok(bytes) = self.kernel.read_uri(&uri, 0, u32::MAX).await {
                input.system = Some(String::from_utf8(bytes)?);
            }
        }
        let extension = self.composition_extension.read().await.clone();
        let snapshot = extension.initial(input).await?;
        Ok(Snapshot::new(snapshot.contributions.into_iter().map(
            |item| Contribution {
                id: item.id,
                source: item.source,
                slot: item.slot,
                order: item.order,
                revision: item.revision,
                content: item.content,
            },
        )))
    }

    pub async fn compose_update(
        &self,
        input: types::SessionInput,
    ) -> anyhow::Result<Vec<crate::CompositionUpdate>> {
        let extension = self.composition_extension.read().await.clone();
        extension.update(input).await
    }
}

#[async_trait::async_trait]
impl crate::CompositionUpdater for ComponentHost {
    async fn update(
        &self,
        input: types::SessionInput,
    ) -> anyhow::Result<Vec<crate::CompositionUpdate>> {
        self.compose_update(input).await
    }
}
