//! The trusted root's `.artist/url` composition source.
//!
//! This controller is the host-side implementation seam used while the root
//! component is being assembled. It deliberately does not interpret child
//! namespace behavior: it discovers complete extension packages, validates
//! the candidate set, and hands lifecycle operations to the component runtime.

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use artist_wasm::master::types;
use artist_wasm::{
    ComponentLoader, ComponentRecord, ExtensionDependency, ExtensionManager, ExtensionMetadata,
    GenerationHandle, RegistrationRecord,
};
use artist_wasm_url::{ClaimToken, UrlClaimRegistry};
use artist_wasm_verbs::component::WasmTool;
use async_trait::async_trait;
use notify::{RecursiveMode, Watcher};
use tokio::sync::mpsc;

use crate::{
    CompactionComponent, ComponentToolRegistry, ToolComponent, WasmCompactionComponent,
    WasmToolComponent,
};
use crate::{ExtensionCatalog, ExtensionPackage};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlCompositionSource {
    pub global: PathBuf,
    pub local: PathBuf,
}

impl UrlCompositionSource {
    pub fn new(global: impl Into<PathBuf>, local: impl Into<PathBuf>) -> Self {
        Self {
            global: global.into(),
            local: local.into(),
        }
    }
}

/// Candidate composition state. It is built completely before activation so
/// invalid WASM, manifests, or dependencies leave the previous tree intact.
pub struct UrlComposition {
    source: UrlCompositionSource,
    engine: wasmtime::Engine,
    manager: ExtensionManager,
    catalog: ExtensionCatalog,
    active: Mutex<BTreeSet<String>>,
    tools: Option<ComponentToolRegistry>,
    active_tools: Mutex<BTreeSet<String>>,
    active_routes: Mutex<BTreeSet<String>>,
}

/// The concrete loader installed into the generic master by the native
/// bootstrap. It knows how to find package artifacts, but it does not assign
/// meaning to their URI claims.
pub struct PackageComponentLoader {
    engine: wasmtime::Engine,
    manager: ExtensionManager,
    catalog: ExtensionCatalog,
    claims: UrlClaimRegistry,
    next_component: Mutex<u64>,
    packages: Mutex<BTreeMap<u64, PathBuf>>,
    active: Mutex<BTreeMap<u64, GenerationHandle>>,
    registrations: Mutex<BTreeMap<u64, ClaimToken>>,
}

impl PackageComponentLoader {
    pub fn new(
        engine: wasmtime::Engine,
        manager: ExtensionManager,
        catalog: ExtensionCatalog,
        claims: UrlClaimRegistry,
    ) -> Self {
        Self {
            engine,
            manager,
            catalog,
            claims,
            next_component: Mutex::new(1),
            packages: Mutex::new(BTreeMap::new()),
            active: Mutex::new(BTreeMap::new()),
            registrations: Mutex::new(BTreeMap::new()),
        }
    }

    fn descriptor(package: &ExtensionPackage) -> types::Descriptor {
        types::Descriptor {
            name: package.manifest().name.clone(),
            version: package.manifest().version.clone(),
            dependencies: package
                .manifest()
                .dependencies
                .iter()
                .map(|dependency| types::Dependency {
                    name: dependency.name.clone(),
                    version: dependency.version.clone(),
                })
                .collect(),
        }
    }
}

#[async_trait]
impl ComponentLoader for PackageComponentLoader {
    async fn load(&self, locator: &str) -> anyhow::Result<ComponentRecord> {
        let package = ExtensionPackage::open(&self.engine, Path::new(locator))?;
        let mut next = self.next_component.lock().unwrap();
        let id = *next;
        *next += 1;
        self.packages
            .lock()
            .unwrap()
            .insert(id, package.root().to_path_buf());
        Ok(ComponentRecord {
            id,
            descriptor: Self::descriptor(&package),
        })
    }

    async fn describe(&self, component: &ComponentRecord) -> anyhow::Result<types::Descriptor> {
        Ok(component.descriptor.clone())
    }

    async fn activate(&self, component: &ComponentRecord) -> anyhow::Result<ComponentRecord> {
        let root = self
            .packages
            .lock()
            .unwrap()
            .get(&component.id)
            .cloned()
            .ok_or_else(|| anyhow!("unknown component record {}", component.id))?;
        let package = ExtensionPackage::open(&self.engine, root)?;
        let metadata = ExtensionMetadata {
            name: package.manifest().name.clone(),
            version: package.manifest().version.clone(),
            route_hints: package.manifest().route_hints.clone(),
            dependencies: package
                .manifest()
                .dependencies
                .iter()
                .map(|dependency| ExtensionDependency {
                    name: dependency.name.clone(),
                    version: dependency.version.clone(),
                })
                .collect(),
        };
        let handle = self
            .manager
            .activate_bytes(package.artifact().bytes(), metadata)
            .await?;
        self.catalog.publish(&package);
        self.active.lock().unwrap().insert(component.id, handle);
        Ok(component.clone())
    }

    async fn retire(&self, component: &ComponentRecord) -> anyhow::Result<()> {
        let handle = { self.active.lock().unwrap().remove(&component.id) };
        if let Some(handle) = handle {
            self.manager.retire(handle.name()).await;
        }
        Ok(())
    }

    async fn register(
        &self,
        _component: &ComponentRecord,
        uri: &str,
    ) -> anyhow::Result<RegistrationRecord> {
        let token = self.claims.register(uri)?;
        let id = token.id();
        self.registrations.lock().unwrap().insert(id, token);
        Ok(RegistrationRecord { id })
    }

    async fn unregister(&self, registration: &RegistrationRecord) -> anyhow::Result<()> {
        let token = self
            .registrations
            .lock()
            .unwrap()
            .remove(&registration.id)
            .ok_or_else(|| anyhow!("unknown registration {}", registration.id))?;
        self.claims.unregister(&token)
    }
}

impl UrlComposition {
    pub fn new(
        source: UrlCompositionSource,
        engine: wasmtime::Engine,
        manager: ExtensionManager,
        catalog: ExtensionCatalog,
    ) -> Self {
        Self {
            source,
            engine,
            manager,
            catalog,
            active: Mutex::new(BTreeSet::new()),
            tools: None,
            active_tools: Mutex::new(BTreeSet::new()),
            active_routes: Mutex::new(BTreeSet::new()),
        }
    }

    pub fn with_tools(mut self, tools: ComponentToolRegistry) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn source(&self) -> &UrlCompositionSource {
        &self.source
    }

    pub fn tools(&self) -> Option<ComponentToolRegistry> {
        self.tools.clone()
    }

    pub fn active_routes(&self) -> BTreeSet<String> {
        self.active_routes.lock().unwrap().clone()
    }

    pub fn has_route(&self, route: &str) -> bool {
        self.active_routes.lock().unwrap().contains(route)
    }

    pub fn compaction_components(
        &self,
        handles: &[GenerationHandle],
    ) -> anyhow::Result<Vec<Arc<dyn CompactionComponent>>> {
        let packages = self
            .discover()?
            .into_values()
            .map(|root| ExtensionPackage::open(&self.engine, root))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let mut components: Vec<Arc<dyn CompactionComponent>> = Vec::new();
        for package in packages {
            let Some(handle) = handles
                .iter()
                .find(|handle| handle.name() == package.manifest().name)
            else {
                continue;
            };
            for route in package
                .manifest()
                .route_hints
                .iter()
                .filter(|route| route.starts_with("compaction://"))
            {
                let tool_name = route.trim_start_matches("compaction://");
                if tool_name.is_empty() {
                    return Err(anyhow!("empty compaction component route"));
                }
                components.push(Arc::new(WasmCompactionComponent::new(
                    route.clone(),
                    WasmTool::new(tool_name, handle.clone()),
                )));
            }
        }
        Ok(components)
    }

    /// Scan global then local entries. A local package with the same manifest
    /// name replaces the global package before any WASM is compiled.
    pub fn discover(&self) -> anyhow::Result<BTreeMap<String, PathBuf>> {
        let mut packages = BTreeMap::new();
        for root in [&self.source.global, &self.source.local] {
            for package_root in package_roots(root)? {
                let package =
                    ExtensionPackage::open(&self.engine, &package_root).with_context(|| {
                        format!("open composition package {}", package_root.display())
                    })?;
                packages.insert(package.manifest().name.clone(), package_root);
            }
        }
        Ok(packages)
    }

    /// Validate and activate the complete candidate composition. Activation
    /// is performed only after every selected package has compiled and its
    /// metadata has validated.
    pub async fn reload(&self) -> anyhow::Result<Vec<GenerationHandle>> {
        let roots = self.discover()?;
        let mut packages = Vec::with_capacity(roots.len());
        for root in roots.values() {
            packages.push(ExtensionPackage::open(&self.engine, root)?);
        }
        let composition_count = packages
            .iter()
            .filter(|package| package.artifact().class().composition)
            .count();
        if composition_count != 1 {
            return Err(anyhow!(
                "a valid composition must contain exactly one composition extension; found {composition_count}"
            ));
        }

        let mut bundle = Vec::with_capacity(packages.len());
        for package in &packages {
            let metadata = ExtensionMetadata {
                name: package.manifest().name.clone(),
                version: package.manifest().version.clone(),
                route_hints: package.manifest().route_hints.clone(),
                dependencies: package
                    .manifest()
                    .dependencies
                    .iter()
                    .map(|dependency| ExtensionDependency {
                        name: dependency.name.clone(),
                        version: dependency.version.clone(),
                    })
                    .collect(),
            };
            metadata.validate()?;
            bundle.push((package.artifact().bytes().to_vec(), metadata));
        }

        let claims = UrlClaimRegistry::default();
        for package in &packages {
            for name in &package.manifest().route_hints {
                // The manifest also serves the model-tool registry, whose
                // stable public names are deliberately bare (`read`,
                // `grep`, ...).  Turn that shorthand into a real namespace
                // claim at the URL boundary; explicit URI claims remain
                // untouched and are validated by the URL registry itself.
                let claim = if name.contains("://") {
                    name.clone()
                } else {
                    format!("tool://{name}")
                };
                claims
                    .register(&claim)
                    .map_err(|error| anyhow!("extension namespace claim {name:?}: {error}"))?;
            }
        }

        // Validate the model-facing tool namespace before activating a new
        // runtime generation. Existing tools from this composition may be
        // replaced; tools owned by another component (notably process tools)
        // are a deterministic conflict.
        let old_tools = self.active_tools.lock().unwrap().clone();
        let mut candidate_tool_names = BTreeSet::new();
        for package in &packages {
            for name in package
                .manifest()
                .route_hints
                .iter()
                .filter_map(|name| bare_tool_name(name))
            {
                if !candidate_tool_names.insert(name.to_owned()) {
                    return Err(anyhow!("duplicate tool route claim {name:?}"));
                }
                if self
                    .tools
                    .as_ref()
                    .is_some_and(|tools| tools.all_names().iter().any(|existing| existing == name))
                    && !old_tools.contains(name)
                {
                    return Err(anyhow!(
                        "tool route {name:?} conflicts with an active non-composition tool"
                    ));
                }
            }
        }

        let handles = self.manager.activate_bundle(bundle).await?;
        for package in &packages {
            self.catalog.publish(package);
        }

        let desired: BTreeSet<_> = roots.keys().cloned().collect();
        let mut active_routes = BTreeSet::new();
        for package in &packages {
            for route in &package.manifest().route_hints {
                active_routes.insert(if route.contains("://") {
                    route.clone()
                } else {
                    format!("tool://{route}")
                });
            }
        }
        let old: Vec<_> = self
            .active
            .lock()
            .unwrap()
            .difference(&desired)
            .cloned()
            .collect();
        for name in old {
            self.catalog.unpublish(&name);
            self.manager.retire(&name).await;
        }
        if let Some(tools) = &self.tools {
            let mut additions: Vec<(String, Arc<dyn ToolComponent>)> = Vec::new();
            let mut current_tools = BTreeSet::new();
            for handle in &handles {
                if !handle.class().verb {
                    continue;
                }
                let Some(package) = packages
                    .iter()
                    .find(|package| package.manifest().name == handle.name())
                else {
                    continue;
                };
                let names = if package.manifest().route_hints.is_empty() {
                    vec![package.manifest().name.clone()]
                } else {
                    package
                        .manifest()
                        .route_hints
                        .iter()
                        .filter_map(|name| bare_tool_name(name).map(str::to_owned))
                        .collect()
                };
                let definitions = package.tool_definitions()?;
                for name in names {
                    let tool_name = name.clone();
                    let definition = definitions.get(&tool_name).cloned().ok_or_else(|| {
                        anyhow!(
                            "component {:?} exposes tool {:?} without a model-facing contract",
                            package.manifest().name,
                            tool_name
                        )
                    })?;
                    current_tools.insert(name.clone());
                    additions.push((
                        name,
                        Arc::new(WasmToolComponent::new(
                            WasmTool::new(tool_name, handle.clone()),
                            definition,
                        )) as Arc<dyn ToolComponent>,
                    ));
                }
            }
            tools
                .replace_generation(&old_tools, additions)
                .map_err(|error| anyhow!(error.to_string()))?;
            *self.active_tools.lock().unwrap() = current_tools;
        }
        *self.active.lock().unwrap() = desired;
        *self.active_routes.lock().unwrap() = active_routes;
        Ok(handles)
    }

    /// Keep all active sessions synchronized with the global/local URL source.
    /// The watcher keeps the last valid composition when a write is partial or
    /// invalid; the next filesystem event retries the candidate.
    pub fn watch(self: Arc<Self>) -> anyhow::Result<CompositionWatcher> {
        self.watch_with(|_| async {})
    }

    /// Watch and report only complete, valid replacement generations. The
    /// callback runs after `reload` has atomically prepared the candidate, so
    /// host-side adapters can move to the same generation without exposing a
    /// partially written composition.
    pub fn watch_with<F, Fut>(self: Arc<Self>, callback: F) -> anyhow::Result<CompositionWatcher>
    where
        F: Fn(Vec<GenerationHandle>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let mut watcher = notify::recommended_watcher(move |event| {
            let _ = sender.send(event);
        })?;
        for path in [&self.source.global, &self.source.local] {
            if path.exists() {
                watcher.watch(path, RecursiveMode::Recursive)?;
            }
        }

        let task = tokio::spawn(async move {
            let _watcher = watcher;
            while receiver.recv().await.is_some() {
                tokio::time::sleep(Duration::from_millis(75)).await;
                while receiver.try_recv().is_ok() {}
                if let Ok(handles) = self.reload().await {
                    callback(handles).await;
                }
            }
        });
        Ok(CompositionWatcher { task })
    }
}

pub struct CompositionWatcher {
    task: tokio::task::JoinHandle<()>,
}

impl CompositionWatcher {
    pub fn abort(self) {
        self.task.abort();
    }
}

fn package_roots(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    if !root.is_dir() {
        return Err(anyhow!(
            "composition root is not a directory: {}",
            root.display()
        ));
    }
    let mut roots = Vec::new();
    for entry in std::fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() && path.join("extension.toml").is_file() {
            roots.push(path);
        }
    }
    roots.sort();
    Ok(roots)
}

fn bare_tool_name(value: &str) -> Option<&str> {
    (!value.trim().is_empty()
        && !value.contains("://")
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0'))
    .then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::Kernel;
    use artist_wasm::{KernelHostEnvironment, Runtime, build_engine};

    const COMPONENT: &str = r#"(component
        (core module $m (func (export "f")))
        (export "artist:nouns/provider@2.0.0" (core module $m))
    )"#;

    fn package(root: &Path, name: &str, version: &str) {
        let path = root.join(name);
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(
            path.join("extension.toml"),
            format!("name = '{name}'\nversion = '{version}'\n"),
        )
        .unwrap();
        std::fs::write(path.join("extension.wasm"), COMPONENT).unwrap();
        std::fs::write(path.join("README.md"), name).unwrap();
        std::fs::write(path.join("src/main.rs"), "fn main() {}").unwrap();
    }

    #[test]
    fn local_url_package_replaces_global_package_by_manifest_name() {
        let global = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        package(global.path(), "shared", "1.0.0");
        package(local.path(), "shared", "2.0.0");
        package(global.path(), "global-only", "1.0.0");

        let engine = build_engine().unwrap();
        let runtime = Runtime::new(
            engine.clone(),
            Arc::new(KernelHostEnvironment::new(Arc::new(Kernel::empty()))),
        );
        let composition = UrlComposition::new(
            UrlCompositionSource::new(global.path(), local.path()),
            engine,
            ExtensionManager::new(Arc::new(runtime)),
            ExtensionCatalog::default(),
        );
        let discovered = composition.discover().unwrap();
        assert_eq!(discovered.len(), 2);
        let selected =
            ExtensionPackage::open(&build_engine().unwrap(), discovered.get("shared").unwrap())
                .unwrap();
        assert_eq!(selected.manifest().version, "2.0.0");
    }
}
