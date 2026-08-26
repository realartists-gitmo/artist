use std::{collections::BTreeSet, path::Path};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wasmtime::{Config, Engine, Store, component::Component};
use wasmtime_wasi::p3;

wasmtime::component::bindgen!({
    path: "wit",
    world: "artist-plugin",
    imports: { default: async },
    exports: { default: async },
    require_store_data_send: true,
});

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub capabilities: BTreeSet<String>,
}

#[derive(Debug)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub component: Component,
}

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("failed to configure Wasmtime: {0}")]
    Wasmtime(#[from] wasmtime::Error),
    #[error("plugin requests capabilities that were not granted: {0:?}")]
    CapabilityDenied(BTreeSet<String>),
}

/// Wasmtime Component host configured for async WASI 0.3 (p3).
/// Components receive no ambient filesystem or network access.
pub struct PluginHost {
    engine: Engine,
    allowed_capabilities: BTreeSet<String>,
}

impl PluginHost {
    pub fn new(
        allowed_capabilities: impl IntoIterator<Item = String>,
    ) -> Result<Self, PluginError> {
        let mut config = Config::new();
        config.wasm_component_model_async(true);
        let engine = Engine::new(&config)?;
        // Constructing the p3 linker here verifies that this build really includes the
        // requested async WASI generation. Per-instance state is attached at execution.
        let mut linker = wasmtime::component::Linker::<PluginState>::new(&engine);
        p3::add_to_linker(&mut linker)?;
        Ok(Self {
            engine,
            allowed_capabilities: allowed_capabilities.into_iter().collect(),
        })
    }

    pub fn load(
        &self,
        manifest: PluginManifest,
        path: impl AsRef<Path>,
    ) -> Result<LoadedPlugin, PluginError> {
        let denied = manifest
            .capabilities
            .difference(&self.allowed_capabilities)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !denied.is_empty() {
            return Err(PluginError::CapabilityDenied(denied));
        }
        let component = Component::from_file(&self.engine, path)?;
        Ok(LoadedPlugin {
            manifest,
            component,
        })
    }

    /// Calls the plugin's stable JSON operation boundary in a fresh capability state.
    pub async fn call(
        &self,
        plugin: &LoadedPlugin,
        operation: &str,
        payload: &serde_json::Value,
    ) -> Result<serde_json::Value, PluginError> {
        let mut linker = wasmtime::component::Linker::<PluginState>::new(&self.engine);
        p3::add_to_linker(&mut linker)?;
        let mut store = Store::new(&self.engine, PluginState::default());
        let bindings =
            ArtistPlugin::instantiate_async(&mut store, &plugin.component, &linker).await?;
        let encoded = serde_json::to_string(payload)
            .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
        let returned = bindings.call_call(&mut store, operation, &encoded).await?;
        let returned = returned.map_err(wasmtime::Error::msg)?;
        serde_json::from_str(&returned)
            .map_err(|error| PluginError::Wasmtime(wasmtime::Error::msg(error.to_string())))
    }

    pub fn validate_bytes(
        &self,
        manifest: PluginManifest,
        bytes: impl AsRef<[u8]>,
    ) -> Result<LoadedPlugin, PluginError> {
        let denied = manifest
            .capabilities
            .difference(&self.allowed_capabilities)
            .cloned()
            .collect::<BTreeSet<_>>();
        if !denied.is_empty() {
            return Err(PluginError::CapabilityDenied(denied));
        }
        let component = Component::new(&self.engine, bytes)?;
        Ok(LoadedPlugin {
            manifest,
            component,
        })
    }
}

#[derive(Default)]
pub struct PluginState {
    ctx: wasmtime_wasi::WasiCtx,
    table: wasmtime::component::ResourceTable,
}

impl wasmtime_wasi::WasiView for PluginState {
    fn ctx(&mut self) -> wasmtime_wasi::WasiCtxView<'_> {
        wasmtime_wasi::WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_are_denied_by_default() {
        let host = PluginHost::new([]).unwrap();
        let manifest = PluginManifest {
            name: "unsafe".into(),
            version: "1".into(),
            capabilities: ["workspace.write".into()].into_iter().collect(),
        };
        assert!(matches!(
            host.validate_bytes(manifest, b"(component)"),
            Err(PluginError::CapabilityDenied(_))
        ));
    }
}
