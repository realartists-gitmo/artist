use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::{Context, anyhow};
use artist_kernel::{Kernel, ProviderAttrs, ProviderEntry, ResourceError, ResourceUri};
use async_trait::async_trait;
use tokio::sync::Notify;
use wasmtime::{
    Engine, Store,
    component::{Component, Instance, Linker},
};

use crate::classify::ExtensionClass;
use crate::loader::Extension;
use crate::verb_registry::VerbRegistryView;

/// The trusted host environment shared by active components.
///
/// This is intentionally one universal host context rather than a per-component
/// capability set. Role-specific WIT binders can project this context into the
/// interfaces they implement.
#[async_trait]
pub trait HostEnvironment: Send + Sync {
    fn kernel(&self) -> Arc<Kernel>;

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.kernel().attrs_uri(uri).await
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        self.kernel().readdir_uri(uri).await
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        self.kernel().read_uri(uri, offset, size).await
    }

    /// Invoke a contract registered by an extension-defined noun. The runtime
    /// forwards opaque bytes and never interprets the contract's vocabulary.
    async fn invoke_contract(
        &self,
        name: &str,
        version: &str,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, ResourceError> {
        self.kernel()
            .invoke_extension_contract(name, version, operation, input)
            .await
    }
}

#[derive(Clone)]
pub struct KernelHostEnvironment {
    kernel: Arc<Kernel>,
}

impl KernelHostEnvironment {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self { kernel }
    }
}

impl HostEnvironment for KernelHostEnvironment {
    fn kernel(&self) -> Arc<Kernel> {
        Arc::clone(&self.kernel)
    }
}

/// Explicit URI capability set for a component host. An empty set denies all
/// resource access; trusted unrestricted hosts use [`KernelHostEnvironment`].
#[derive(Clone, Debug, Default)]
pub struct ResourceCapabilities {
    prefixes: Vec<ResourceUri>,
}

impl ResourceCapabilities {
    pub fn new(prefixes: impl IntoIterator<Item = ResourceUri>) -> Self {
        Self {
            prefixes: prefixes.into_iter().collect(),
        }
    }

    pub fn allows(&self, uri: &ResourceUri) -> bool {
        self.prefixes.iter().any(|prefix| uri.starts_with(prefix))
    }
}

/// Host environment that restricts resource access to explicit URI prefixes.
/// This is independent of any concrete noun, verb, or event contract.
#[derive(Clone)]
pub struct ScopedHostEnvironment {
    kernel: Arc<Kernel>,
    capabilities: ResourceCapabilities,
}

impl ScopedHostEnvironment {
    pub fn new(kernel: Arc<Kernel>, capabilities: ResourceCapabilities) -> Self {
        Self {
            kernel,
            capabilities,
        }
    }

    fn authorize(&self, uri: &ResourceUri) -> Result<(), ResourceError> {
        if self.capabilities.allows(uri) {
            Ok(())
        } else {
            Err(ResourceError::new(
                artist_kernel::ResourceErrorCode::PermissionDenied,
                format!("component is not authorized to access {uri}"),
            ))
        }
    }
}

#[async_trait]
impl HostEnvironment for ScopedHostEnvironment {
    fn kernel(&self) -> Arc<Kernel> {
        Arc::clone(&self.kernel)
    }

    async fn attrs(&self, uri: &ResourceUri) -> Result<ProviderAttrs, ResourceError> {
        self.authorize(uri)?;
        self.kernel.attrs_uri(uri).await
    }

    async fn readdir(&self, uri: &ResourceUri) -> Result<Vec<ProviderEntry>, ResourceError> {
        self.authorize(uri)?;
        self.kernel.readdir_uri(uri).await
    }

    async fn read(
        &self,
        uri: &ResourceUri,
        offset: u64,
        size: u32,
    ) -> Result<Vec<u8>, ResourceError> {
        self.authorize(uri)?;
        self.kernel.read_uri(uri, offset, size).await
    }

    async fn invoke_contract(
        &self,
        name: &str,
        version: &str,
        operation: &str,
        input: &[u8],
    ) -> Result<Vec<u8>, ResourceError> {
        self.kernel
            .invoke_extension_contract(name, version, operation, input)
            .await
    }
}

#[derive(Clone, Debug, Default)]
pub struct ExtensionMetadata {
    pub name: String,
    pub version: String,
    /// Canonical URI prefixes claimed by this component.
    pub claims: Vec<String>,
    /// Other active extensions required by this component.
    pub dependencies: Vec<ExtensionDependency>,
}

/// A required extension and the compatible versions it accepts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionDependency {
    pub name: String,
    pub version: String,
}

impl ExtensionDependency {
    fn validate(&self) -> anyhow::Result<semver::VersionReq> {
        if self.name.trim().is_empty() {
            return Err(anyhow!("extension dependency name is empty"));
        }
        semver::VersionReq::parse(&self.version)
            .with_context(|| format!("invalid version requirement for {}", self.name))
    }
}

impl ExtensionMetadata {
    pub fn validate(&self) -> anyhow::Result<Vec<ResourceUri>> {
        if self.name.trim().is_empty() {
            return Err(anyhow!("component metadata name is empty"));
        }
        if self.version.trim().is_empty() {
            return Err(anyhow!("component metadata version is empty"));
        }
        semver::Version::parse(&self.version)
            .with_context(|| format!("invalid semantic version {}", self.version))?;
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        self.claims
            .iter()
            .map(|claim| {
                claim
                    .parse()
                    .with_context(|| format!("invalid resource claim: {claim}"))
            })
            .collect()
    }
}

/// Store data used by component instances. Role-specific crates add their
/// WIT host implementations to a `Linker<RuntimeStore>` before instantiation.
pub struct RuntimeStore {
    pub host: Arc<dyn HostEnvironment>,
}

impl VerbRegistryView for RuntimeStore {
    fn verb_registry(&mut self) -> crate::verb_registry::VerbRegistryContext<'_> {
        crate::verb_registry::VerbRegistryContext::new(self.host.kernel().verb_registry())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub fuel: u64,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self { fuel: 10_000_000 }
    }
}

struct GenerationInner {
    id: u64,
    engine: Engine,
    metadata: ExtensionMetadata,
    claims: Vec<ResourceUri>,
    extension: Arc<Extension>,
    host: Arc<dyn HostEnvironment>,
    limits: RuntimeLimits,
    state: AtomicU8,
    in_flight: AtomicUsize,
    drained: Notify,
    cleanup: Mutex<Vec<Box<dyn FnOnce() + Send + 'static>>>,
}

const ACTIVE: u8 = 0;
const RETIRING: u8 = 1;
const RETIRED: u8 = 2;

impl GenerationInner {
    fn try_pin(self: &Arc<Self>) -> Option<GenerationLease> {
        loop {
            if self.state.load(Ordering::Acquire) != ACTIVE {
                return None;
            }
            self.in_flight.fetch_add(1, Ordering::AcqRel);
            if self.state.load(Ordering::Acquire) == ACTIVE {
                return Some(GenerationLease {
                    generation: Arc::clone(self),
                });
            }
            self.release();
        }
    }

    fn release(&self) {
        if self.in_flight.fetch_sub(1, Ordering::AcqRel) == 1
            && self.state.load(Ordering::Acquire) != ACTIVE
        {
            self.drained.notify_waiters();
        }
    }

    fn begin_retirement(&self) {
        let _ = self
            .state
            .compare_exchange(ACTIVE, RETIRING, Ordering::AcqRel, Ordering::Acquire);
        if self.in_flight.load(Ordering::Acquire) == 0 {
            self.drained.notify_waiters();
        }
    }

    async fn retire(&self) {
        self.begin_retirement();
        while self.in_flight.load(Ordering::Acquire) != 0 {
            self.drained.notified().await;
        }
        let cleanup = {
            let mut cleanup = self.cleanup.lock().unwrap();
            self.state.store(RETIRED, Ordering::Release);
            std::mem::take(&mut *cleanup)
        };
        for callback in cleanup {
            callback();
        }
    }

    fn register_cleanup(&self, callback: Box<dyn FnOnce() + Send + 'static>) {
        let mut cleanup = self.cleanup.lock().unwrap();
        if self.state.load(Ordering::Acquire) == RETIRED {
            drop(cleanup);
            callback();
        } else {
            cleanup.push(callback);
        }
    }
}

/// A prepared, immutable component activation.
pub struct PreparedGeneration {
    id: u64,
    metadata: ExtensionMetadata,
    claims: Vec<ResourceUri>,
    extension: Arc<Extension>,
    host: Arc<dyn HostEnvironment>,
}

/// An active generation that can be pinned for an invocation.
#[derive(Clone)]
pub struct GenerationHandle {
    inner: Arc<GenerationInner>,
}

impl GenerationHandle {
    pub fn id(&self) -> u64 {
        self.inner.id
    }
    pub fn name(&self) -> &str {
        &self.inner.metadata.name
    }
    pub fn metadata(&self) -> &ExtensionMetadata {
        &self.inner.metadata
    }
    pub fn class(&self) -> ExtensionClass {
        self.inner.extension.class
    }
    pub fn claims(&self) -> &[ResourceUri] {
        &self.inner.claims
    }
    pub fn is_retiring(&self) -> bool {
        self.inner.state.load(Ordering::Acquire) != ACTIVE
    }
    pub fn in_flight(&self) -> usize {
        self.inner.in_flight.load(Ordering::Acquire)
    }

    pub fn pin(&self) -> Option<GenerationLease> {
        self.inner.try_pin()
    }

    pub async fn retire(&self) {
        self.inner.retire().await;
    }

    /// Register generation-owned cleanup such as an event subscription or
    /// background task shutdown hook. It runs exactly once at retirement.
    pub fn on_retire(&self, callback: impl FnOnce() + Send + 'static) {
        self.inner.register_cleanup(Box::new(callback));
    }
}

/// A lease that keeps one generation alive for the duration of an invocation.
pub struct GenerationLease {
    generation: Arc<GenerationInner>,
}

impl GenerationLease {
    pub fn id(&self) -> u64 {
        self.generation.id
    }
    pub fn name(&self) -> &str {
        &self.generation.metadata.name
    }
    pub fn class(&self) -> ExtensionClass {
        self.generation.extension.class
    }
    pub fn extension(&self) -> &Extension {
        &self.generation.extension
    }
    pub fn component(&self) -> &Component {
        &self.generation.extension.component
    }
    pub fn host(&self) -> &Arc<dyn HostEnvironment> {
        &self.generation.host
    }

    pub fn store(&self) -> anyhow::Result<Store<RuntimeStore>> {
        let mut store = Store::new(
            &self.generation.engine,
            RuntimeStore {
                host: Arc::clone(&self.generation.host),
            },
        );
        store.set_fuel(self.generation.limits.fuel)?;
        Ok(store)
    }

    /// Apply the generation's guest execution budget to a caller-owned store.
    pub fn configure_store(&self, store: &mut Store<RuntimeStore>) -> anyhow::Result<()> {
        store.set_fuel(self.generation.limits.fuel)?;
        Ok(())
    }

    pub async fn instantiate(&self) -> anyhow::Result<(Store<RuntimeStore>, Instance)> {
        let mut store = self.store()?;
        let linker = Linker::new(&self.generation.engine);
        let instance = linker
            .instantiate_async(&mut store, &self.generation.extension.component)
            .await?;
        Ok((store, instance))
    }

    pub async fn instantiate_with_linker(
        &self,
        store: &mut Store<RuntimeStore>,
        linker: &Linker<RuntimeStore>,
    ) -> anyhow::Result<Instance> {
        self.configure_store(store)?;
        Ok(linker
            .instantiate_async(store, &self.generation.extension.component)
            .await?)
    }
}

impl Drop for GenerationLease {
    fn drop(&mut self) {
        self.generation.release();
    }
}

/// The component catalog and generation lifecycle manager.
pub struct Runtime {
    engine: Engine,
    host: Arc<dyn HostEnvironment>,
    limits: RuntimeLimits,
    next_id: AtomicUsize,
    active: RwLock<BTreeMap<String, Arc<GenerationInner>>>,
}

/// Runtime control surface for extensions. This deliberately contains no
/// persistent configuration: callers may activate, retire, or replace an
/// extension while the process remains alive.
#[derive(Clone)]
pub struct ExtensionManager {
    runtime: Arc<Runtime>,
}

impl ExtensionManager {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }

    pub async fn activate_bytes(
        &self,
        bytes: &[u8],
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<GenerationHandle> {
        let prepared = self.runtime.prepare(bytes, metadata)?;
        self.runtime.activate(prepared).await
    }

    pub async fn activate_file(
        &self,
        path: impl AsRef<std::path::Path>,
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<GenerationHandle> {
        let bytes = tokio::fs::read(path).await?;
        self.activate_bytes(&bytes, metadata).await
    }

    /// Replace an active generation or activate the extension if it is absent.
    pub async fn replace_bytes(
        &self,
        bytes: &[u8],
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<GenerationHandle> {
        self.activate_bytes(bytes, metadata).await
    }

    pub async fn replace_file(
        &self,
        path: impl AsRef<std::path::Path>,
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<GenerationHandle> {
        self.activate_file(path, metadata).await
    }

    pub async fn retire(&self, name: &str) -> bool {
        self.runtime.retire(name).await
    }

    pub fn active_names(&self) -> Vec<String> {
        self.runtime.active_names()
    }

    /// Activate a set of artifacts in dependency order. Dependencies may be
    /// supplied in any order; already-active compatible dependencies are
    /// reused. Missing dependencies and cycles are rejected before activation.
    pub async fn activate_bundle(
        &self,
        extensions: Vec<(Vec<u8>, ExtensionMetadata)>,
    ) -> anyhow::Result<Vec<GenerationHandle>> {
        let mut pending = BTreeMap::new();
        for (bytes, metadata) in extensions {
            metadata.validate()?;
            if pending
                .insert(metadata.name.clone(), (bytes, metadata))
                .is_some()
            {
                return Err(anyhow!("duplicate extension in bundle"));
            }
        }
        for (_, metadata) in pending.values() {
            for dependency in &metadata.dependencies {
                let requirement = dependency.validate()?;
                if let Some((_, candidate)) = pending.get(&dependency.name) {
                    let version = semver::Version::parse(&candidate.version)?;
                    if !requirement.matches(&version) {
                        return Err(anyhow!(
                            "extension {} requires {} {}, bundle provides {}",
                            metadata.name,
                            dependency.name,
                            dependency.version,
                            version
                        ));
                    }
                } else if !self.runtime.dependency_is_active(dependency)? {
                    return Err(anyhow!(
                        "extension {} requires unavailable dependency {} ({})",
                        metadata.name,
                        dependency.name,
                        dependency.version
                    ));
                }
            }
        }

        let mut ordered = Vec::new();
        let mut resolved = std::collections::BTreeSet::new();
        while !pending.is_empty() {
            let candidate = pending.iter().find_map(|(name, (_, metadata))| {
                metadata
                    .dependencies
                    .iter()
                    .all(|dependency| {
                        resolved.contains(&dependency.name)
                            || self
                                .runtime
                                .dependency_is_active(dependency)
                                .unwrap_or(false)
                    })
                    .then_some(name.clone())
            });
            let name = candidate.ok_or_else(|| {
                anyhow!("extension bundle contains a missing dependency or cycle")
            })?;
            let (bytes, metadata) = pending.remove(&name).unwrap();
            resolved.insert(name);
            ordered.push((bytes, metadata));
        }
        let prepared = ordered
            .into_iter()
            .map(|(bytes, metadata)| self.runtime.prepare_unchecked(&bytes, metadata))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.runtime.activate_batch(prepared).await
    }
}

impl Runtime {
    pub fn new(engine: Engine, host: Arc<dyn HostEnvironment>) -> Self {
        Self::with_limits(engine, host, RuntimeLimits::default())
    }

    pub fn with_limits(
        engine: Engine,
        host: Arc<dyn HostEnvironment>,
        limits: RuntimeLimits,
    ) -> Self {
        Self {
            engine,
            host,
            limits,
            next_id: AtomicUsize::new(1),
            active: RwLock::new(BTreeMap::new()),
        }
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn prepare(
        &self,
        bytes: &[u8],
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<PreparedGeneration> {
        metadata.validate()?;
        self.require_dependencies(&metadata)?;
        self.prepare_unchecked(bytes, metadata)
    }

    fn prepare_unchecked(
        &self,
        bytes: &[u8],
        metadata: ExtensionMetadata,
    ) -> anyhow::Result<PreparedGeneration> {
        let claims = metadata.validate()?;
        let extension = Arc::new(
            Extension::load(&self.engine, bytes).context("compile and classify component")?,
        );
        if extension.class.is_empty() {
            return Err(anyhow!("component exports no Artist contract family"));
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) as u64;
        Ok(PreparedGeneration {
            id,
            metadata,
            claims,
            extension,
            host: Arc::clone(&self.host),
        })
    }

    fn require_dependencies(&self, metadata: &ExtensionMetadata) -> anyhow::Result<()> {
        let active = self.active.read().unwrap();
        for dependency in &metadata.dependencies {
            let requirement = dependency.validate()?;
            let generation = active.get(&dependency.name).ok_or_else(|| {
                anyhow!(
                    "extension {} requires active dependency {} ({})",
                    metadata.name,
                    dependency.name,
                    dependency.version
                )
            })?;
            let version =
                semver::Version::parse(&generation.metadata.version).with_context(|| {
                    format!("active dependency {} has invalid version", dependency.name)
                })?;
            if !requirement.matches(&version) {
                return Err(anyhow!(
                    "extension {} requires {} {}, active version is {}",
                    metadata.name,
                    dependency.name,
                    dependency.version,
                    version
                ));
            }
        }
        Ok(())
    }

    fn dependency_is_active(&self, dependency: &ExtensionDependency) -> anyhow::Result<bool> {
        let requirement = dependency.validate()?;
        let active = self.active.read().unwrap();
        let Some(generation) = active.get(&dependency.name) else {
            return Ok(false);
        };
        let version = semver::Version::parse(&generation.metadata.version)?;
        Ok(requirement.matches(&version))
    }

    fn conflicts(&self, prepared: &[PreparedGeneration]) -> Option<String> {
        let active = self.active.read().unwrap();
        for (index, candidate) in prepared.iter().enumerate() {
            if let Some(conflict) = active.values().find_map(|generation| {
                if generation.metadata.name == candidate.metadata.name {
                    return None;
                }
                candidate
                    .claims
                    .iter()
                    .any(|claim| {
                        generation
                            .claims
                            .iter()
                            .any(|other| claim.starts_with(other) || other.starts_with(claim))
                    })
                    .then(|| generation.metadata.name.clone())
            }) {
                return Some(conflict);
            }
            if prepared[index + 1..].iter().any(|other| {
                candidate.claims.iter().any(|claim| {
                    other.claims.iter().any(|other_claim| {
                        claim.starts_with(other_claim) || other_claim.starts_with(claim)
                    })
                })
            }) {
                return Some(candidate.metadata.name.clone());
            }
        }
        None
    }

    /// Atomically publish a prepared generation, then retire the replaced
    /// generation after no pinned calls remain.
    pub async fn activate(&self, prepared: PreparedGeneration) -> anyhow::Result<GenerationHandle> {
        let mut handles = self.activate_batch(vec![prepared]).await?;
        Ok(handles.remove(0))
    }

    async fn activate_batch(
        &self,
        prepared: Vec<PreparedGeneration>,
    ) -> anyhow::Result<Vec<GenerationHandle>> {
        if let Some(conflict) = self.conflicts(&prepared) {
            return Err(anyhow!(
                "resource claims conflict with active component {conflict}"
            ));
        }
        let generations: Vec<_> = prepared
            .into_iter()
            .map(|prepared| {
                Arc::new(GenerationInner {
                    id: prepared.id,
                    engine: self.engine.clone(),
                    metadata: prepared.metadata,
                    claims: prepared.claims,
                    extension: prepared.extension,
                    host: prepared.host,
                    limits: self.limits,
                    state: AtomicU8::new(ACTIVE),
                    in_flight: AtomicUsize::new(0),
                    drained: Notify::new(),
                    cleanup: Mutex::new(Vec::new()),
                })
            })
            .collect();
        let olds = {
            let mut active = self.active.write().unwrap();
            let mut olds = Vec::new();
            for generation in &generations {
                if let Some(old) = active.remove(&generation.metadata.name) {
                    old.begin_retirement();
                    olds.push(old);
                }
                active.insert(generation.metadata.name.clone(), Arc::clone(generation));
            }
            olds
        };
        for old in olds {
            old.retire().await;
        }
        Ok(generations
            .into_iter()
            .map(|generation| GenerationHandle { inner: generation })
            .collect())
    }

    pub fn get(&self, name: &str) -> Option<GenerationHandle> {
        self.active
            .read()
            .unwrap()
            .get(name)
            .cloned()
            .map(|inner| GenerationHandle { inner })
    }

    pub fn pin(&self, name: &str) -> Option<GenerationLease> {
        self.active
            .read()
            .unwrap()
            .get(name)
            .and_then(|generation| generation.try_pin())
    }

    pub fn active_names(&self) -> Vec<String> {
        self.active.read().unwrap().keys().cloned().collect()
    }

    pub async fn retire(&self, name: &str) -> bool {
        let generation = self.active.write().unwrap().remove(name);
        if let Some(generation) = generation {
            generation.retire().await;
            true
        } else {
            false
        }
    }
}

/// Compatibility alias for callers that use the design's terminology.
pub type ComponentRuntime = Runtime;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_engine;
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn generations_pin_and_retire() {
        let kernel = Arc::new(Kernel::empty());
        let runtime = Runtime::new(
            build_engine().unwrap(),
            Arc::new(KernelHostEnvironment::new(kernel)),
        );
        let wat = r#"(component
            (core module $m (func (export "f")))
            (export "artist:nouns/namespace@1.0.0" (core module $m))
        )"#;
        let metadata = ExtensionMetadata {
            name: "demo".into(),
            version: "1.0.0".into(),
            claims: vec!["demo:///".into()],
            dependencies: Vec::new(),
        };
        let prepared = runtime.prepare(wat.as_bytes(), metadata).unwrap();
        let handle = runtime.activate(prepared).await.unwrap();
        let lease = handle.pin().unwrap();
        let store = lease.store().unwrap();
        assert_eq!(store.get_fuel().unwrap(), RuntimeLimits::default().fuel);
        let (_store, _instance) = lease.instantiate().await.unwrap();
        assert_eq!(lease.id(), handle.id());
        assert_eq!(handle.in_flight(), 1);
        let old_id = handle.id();
        drop(lease);
        assert_eq!(handle.in_flight(), 0);
        assert!(!handle.is_retiring());
        assert_eq!(runtime.get("demo").unwrap().id(), old_id);
        assert_eq!(runtime.next_id.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn scoped_host_denies_unlisted_resources() {
        let kernel = Arc::new(Kernel::empty());
        let allowed: ResourceUri = "files:///workspace".parse().unwrap();
        let host = ScopedHostEnvironment::new(kernel, ResourceCapabilities::new([allowed]));
        let denied: ResourceUri = "files:///secrets/token".parse().unwrap();
        let error = host.attrs(&denied).await.unwrap_err();
        assert_eq!(
            error.code,
            artist_kernel::ResourceErrorCode::PermissionDenied
        );
    }

    #[tokio::test]
    async fn extension_manager_replaces_and_retires_live_generations() {
        let kernel = Arc::new(Kernel::empty());
        let runtime = Arc::new(Runtime::new(
            build_engine().unwrap(),
            Arc::new(KernelHostEnvironment::new(kernel)),
        ));
        let manager = ExtensionManager::new(runtime);
        let wat = r#"(component
            (core module $m (func (export "f")))
            (export "artist:nouns/namespace@1.0.0" (core module $m))
        )"#;
        let metadata = || ExtensionMetadata {
            name: "read".into(),
            version: "1.0.0".into(),
            claims: Vec::new(),
            dependencies: Vec::new(),
        };

        let first = manager
            .activate_bytes(wat.as_bytes(), metadata())
            .await
            .unwrap();
        let second = manager
            .replace_bytes(wat.as_bytes(), metadata())
            .await
            .unwrap();
        assert!(second.id() > first.id());
        assert_eq!(manager.active_names(), vec!["read"]);
        assert!(manager.retire("read").await);
        assert!(manager.active_names().is_empty());
    }

    #[tokio::test]
    async fn dependencies_must_be_active_and_version_compatible() {
        let kernel = Arc::new(Kernel::empty());
        let runtime = Arc::new(Runtime::new(
            build_engine().unwrap(),
            Arc::new(KernelHostEnvironment::new(kernel)),
        ));
        let manager = ExtensionManager::new(Arc::clone(&runtime));
        let wat = r#"(component
            (core module $m (func (export "f")))
            (export "artist:nouns/namespace@1.0.0" (core module $m))
        )"#;

        let child = ExtensionMetadata {
            name: "child".into(),
            version: "1.0.0".into(),
            claims: Vec::new(),
            dependencies: vec![ExtensionDependency {
                name: "base".into(),
                version: ">=1.0.0, <2.0.0".into(),
            }],
        };
        assert!(
            manager
                .activate_bytes(wat.as_bytes(), child.clone())
                .await
                .is_err()
        );

        manager
            .activate_bytes(
                wat.as_bytes(),
                ExtensionMetadata {
                    name: "base".into(),
                    version: "1.4.0".into(),
                    claims: Vec::new(),
                    dependencies: Vec::new(),
                },
            )
            .await
            .unwrap();
        assert!(manager.activate_bytes(wat.as_bytes(), child).await.is_ok());
    }

    #[tokio::test]
    async fn bundles_activate_dependencies_before_dependents() {
        let kernel = Arc::new(Kernel::empty());
        let runtime = Arc::new(Runtime::new(
            build_engine().unwrap(),
            Arc::new(KernelHostEnvironment::new(kernel)),
        ));
        let manager = ExtensionManager::new(runtime);
        let wat = r#"(component
            (core module $m (func (export "f")))
            (export "artist:nouns/namespace@1.0.0" (core module $m))
        )"#;
        let base = ExtensionMetadata {
            name: "base".into(),
            version: "1.0.0".into(),
            claims: Vec::new(),
            dependencies: Vec::new(),
        };
        let child = ExtensionMetadata {
            name: "child".into(),
            version: "1.0.0".into(),
            claims: Vec::new(),
            dependencies: vec![ExtensionDependency {
                name: "base".into(),
                version: "^1".into(),
            }],
        };
        let handles = manager
            .activate_bundle(vec![
                (wat.as_bytes().to_vec(), child),
                (wat.as_bytes().to_vec(), base),
            ])
            .await
            .unwrap();
        assert_eq!(
            handles
                .iter()
                .map(GenerationHandle::name)
                .collect::<Vec<_>>(),
            ["base", "child"]
        );
    }
}
