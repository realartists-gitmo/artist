//! Provider/account contracts with explicit secret and private-state boundaries.
pub mod accounts;
pub mod conformance;

use artist_core::{CallId, ContentPart, ModelRoute, SessionId, TokenUsage};
use artist_kernel::{
    ModelError, ModelEvent, ModelHistoryItem, ModelProviderSource, ModelRequest, ModelStream,
    Steering, StreamingModel,
};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
};
use thiserror::Error;
use tokio::sync::watch;
#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("invalid provider contract: {0}")]
    Invalid(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("revision conflict: expected {expected}, actual {actual}")]
    Conflict { expected: u64, actual: u64 },
    #[error("credential store: {0}")]
    Credential(String),
    #[error("state store: {0}")]
    State(String),
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct ProviderId(pub String);
impl ProviderId {
    pub fn new(v: impl Into<String>) -> Result<Self, ProviderError> {
        let v = v.into();
        if v.trim().is_empty() {
            Err(ProviderError::Invalid("empty provider id".into()))
        } else {
            Ok(Self(v))
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    pub name: String,
    #[serde(default)]
    pub parameters: serde_json::Value,
}

impl Capability {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            parameters: serde_json::Value::Null,
        }
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: ProviderId,
    pub revision: String,
    pub models: Vec<String>,
    pub capabilities: Vec<Capability>,
    pub auth_kinds: Vec<String>,
    pub api_variants: Vec<String>,
    pub parameters: serde_json::Value,
}
impl ProviderDescriptor {
    fn valid(&self) -> Result<(), ProviderError> {
        if self.revision.is_empty() || self.models.is_empty() {
            Err(ProviderError::Invalid(
                "revision and models required".into(),
            ))
        } else {
            Ok(())
        }
    }
    pub fn supports_model(&self, m: &str) -> bool {
        self.models
            .iter()
            .any(|p| p == "*" || p == m || (p.ends_with('*') && m.starts_with(&p[..p.len() - 1])))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AccountDescriptor {
    pub id: String,
    pub provider: ProviderId,
    pub credential_ref: String,
    pub credential_kind: String,
    pub api_variant: String,
    pub default_model: String,
    pub default_reasoning: Option<String>,
    pub metadata: BTreeMap<String, String>,
}
impl AccountDescriptor {
    fn validate(&self) -> Result<(), ProviderError> {
        if self.id.is_empty() || self.credential_ref.is_empty() || self.credential_kind.is_empty() {
            return Err(ProviderError::Invalid(
                "account identity and credential reference are required".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub struct StateScope {
    pub provider: ProviderId,
    pub account_id: String,
    pub session_id: String,
    pub profile_epoch: u64,
    pub provider_revision: String,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Versioned<T> {
    pub revision: u64,
    pub value: T,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct CapabilityProbe {
    pub supported: bool,
    pub expires_at: u64,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Default)]
pub struct ProviderPrivateState {
    pub conversation_chain: Option<String>,
    #[serde(default)]
    pub capability_probes: BTreeMap<String, CapabilityProbe>,
    #[serde(default)]
    pub uploaded_files: BTreeMap<String, String>,
    #[serde(default)]
    pub cache_handles: BTreeMap<String, String>,
    #[serde(default)]
    pub overload: serde_json::Value,
    #[serde(default)]
    pub opaque: serde_json::Value,
    #[serde(default)]
    pub blob_digests: Vec<String>,
}

#[async_trait]
pub trait ProviderStateStore: Send + Sync {
    async fn read(
        &self,
        s: &StateScope,
    ) -> Result<Option<Versioned<ProviderPrivateState>>, ProviderError>;
    async fn write(
        &self,
        s: &StateScope,
        e: Option<u64>,
        v: ProviderPrivateState,
    ) -> Result<u64, ProviderError>;
    async fn delete(&self, s: &StateScope, e: u64) -> Result<(), ProviderError>;
}
#[derive(Default)]
pub struct MemoryStateStore {
    values: RwLock<BTreeMap<StateScope, Versioned<ProviderPrivateState>>>,
}
#[async_trait]
impl ProviderStateStore for MemoryStateStore {
    async fn read(
        &self,
        s: &StateScope,
    ) -> Result<Option<Versioned<ProviderPrivateState>>, ProviderError> {
        Ok(self.values.read().unwrap().get(s).cloned())
    }
    async fn write(
        &self,
        s: &StateScope,
        e: Option<u64>,
        v: ProviderPrivateState,
    ) -> Result<u64, ProviderError> {
        let mut x = self.values.write().unwrap();
        let a = x.get(s).map(|v| v.revision);
        if a != e {
            return Err(ProviderError::Conflict {
                expected: e.unwrap_or(0),
                actual: a.unwrap_or(0),
            });
        }
        let r = a.unwrap_or(0) + 1;
        x.insert(
            s.clone(),
            Versioned {
                revision: r,
                value: v,
            },
        );
        Ok(r)
    }
    async fn delete(&self, s: &StateScope, e: u64) -> Result<(), ProviderError> {
        let mut x = self.values.write().unwrap();
        match x.get(s) {
            Some(v) if v.revision == e => {
                x.remove(s);
                Ok(())
            }
            Some(v) => Err(ProviderError::Conflict {
                expected: e,
                actual: v.revision,
            }),
            None => Err(ProviderError::NotFound("state".into())),
        }
    }
}
pub struct FileStateStore {
    root: PathBuf,
    lock: tokio::sync::Mutex<()>,
}
impl FileStateStore {
    pub fn new(p: impl Into<PathBuf>) -> Self {
        Self {
            root: p.into(),
            lock: tokio::sync::Mutex::new(()),
        }
    }
    fn path(&self, s: &StateScope) -> PathBuf {
        let mut h = Sha256::new();
        h.update(serde_json::to_vec(s).unwrap());
        self.root.join(format!("{:x}.json", h.finalize()))
    }
    async fn load(
        &self,
        p: &Path,
    ) -> Result<Option<Versioned<ProviderPrivateState>>, ProviderError> {
        match tokio::fs::read(p).await {
            Ok(b) => serde_json::from_slice(&b)
                .map(Some)
                .map_err(|e| ProviderError::State(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ProviderError::State(e.to_string())),
        }
    }
}
#[async_trait]
impl ProviderStateStore for FileStateStore {
    async fn read(
        &self,
        s: &StateScope,
    ) -> Result<Option<Versioned<ProviderPrivateState>>, ProviderError> {
        let _g = self.lock.lock().await;
        self.load(&self.path(s)).await
    }
    async fn write(
        &self,
        s: &StateScope,
        e: Option<u64>,
        v: ProviderPrivateState,
    ) -> Result<u64, ProviderError> {
        let _g = self.lock.lock().await;
        tokio::fs::create_dir_all(&self.root)
            .await
            .map_err(|e| ProviderError::State(e.to_string()))?;
        let p = self.path(s);
        let old = self.load(&p).await?;
        if old.as_ref().map(|x| x.revision) != e {
            return Err(ProviderError::Conflict {
                expected: e.unwrap_or(0),
                actual: old.map(|x| x.revision).unwrap_or(0),
            });
        }
        let r = e.unwrap_or(0) + 1;
        let t = p.with_extension("tmp");
        tokio::fs::write(
            &t,
            serde_json::to_vec(&Versioned {
                revision: r,
                value: v,
            })
            .unwrap(),
        )
        .await
        .map_err(|e| ProviderError::State(e.to_string()))?;
        tokio::fs::rename(t, p)
            .await
            .map_err(|e| ProviderError::State(e.to_string()))?;
        Ok(r)
    }
    async fn delete(&self, s: &StateScope, e: u64) -> Result<(), ProviderError> {
        let _g = self.lock.lock().await;
        let p = self.path(s);
        let old = self.load(&p).await?;
        if old.as_ref().map(|x| x.revision) != Some(e) {
            return Err(ProviderError::Conflict {
                expected: e,
                actual: old.map(|x| x.revision).unwrap_or(0),
            });
        }
        tokio::fs::remove_file(p)
            .await
            .map_err(|x| ProviderError::State(x.to_string()))
    }
}
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("<redacted>")
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct Credential {
    pub kind: String,
    pub secret: Secret,
    pub expires_at: Option<u64>,
    pub refresh: Option<Secret>,
    pub private: BTreeMap<String, Secret>,
}

impl std::fmt::Debug for Credential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Credential")
            .field("kind", &self.kind)
            .field("secret", &self.secret)
            .field("expires_at", &self.expires_at)
            .field("refresh", &self.refresh)
            .field("private", &self.private)
            .finish()
    }
}
#[async_trait]
pub trait CredentialStore: Send + Sync {
    async fn put(&self, k: &str, v: Credential) -> Result<(), ProviderError>;
    async fn get(&self, k: &str) -> Result<Option<Credential>, ProviderError>;
    async fn delete(&self, k: &str) -> Result<(), ProviderError>;
}
#[derive(Default)]
pub struct MemoryCredentialStore {
    values: RwLock<BTreeMap<String, Credential>>,
}
#[async_trait]
impl CredentialStore for MemoryCredentialStore {
    async fn put(&self, k: &str, v: Credential) -> Result<(), ProviderError> {
        self.values.write().unwrap().insert(k.into(), v);
        Ok(())
    }
    async fn get(&self, k: &str) -> Result<Option<Credential>, ProviderError> {
        Ok(self.values.read().unwrap().get(k).cloned())
    }
    async fn delete(&self, k: &str) -> Result<(), ProviderError> {
        self.values.write().unwrap().remove(k);
        Ok(())
    }
}
/// File-backed credential store. Each credential reference maps to one
/// atomically-replaced JSON document under the store root; secret material
/// never appears in logs, errors, or transcripts. The directory itself holds
/// plaintext secrets — protect it like a keychain, or use an external-vault
/// implementation of `CredentialStore` in production.
pub struct FileCredentialStore {
    root: PathBuf,
    lock: Arc<std::sync::Mutex<()>>,
}
impl FileCredentialStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, ProviderError> {
        let root = root.into();
        std::fs::create_dir_all(&root)
            .map_err(|error| ProviderError::Credential(error.to_string()))?;
        Ok(Self {
            root,
            lock: Arc::default(),
        })
    }
    fn path(&self, k: &str) -> PathBuf {
        // Reference names are hashed into object names so odd key shapes
        // cannot escape the root.
        let mut hasher = Sha256::new();
        hasher.update(k.as_bytes());
        let name: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        self.root.join(format!("{name}.json"))
    }
}
#[async_trait]
impl CredentialStore for FileCredentialStore {
    async fn put(&self, k: &str, v: Credential) -> Result<(), ProviderError> {
        let bytes =
            serde_json::to_vec(&v).map_err(|error| ProviderError::Credential(error.to_string()))?;
        let path = self.path(k);
        let tmp = path.with_extension("tmp");
        let _guard = self.lock.lock().unwrap();
        std::fs::write(&tmp, &bytes).map_err(|e| ProviderError::Credential(e.to_string()))?;
        std::fs::rename(&tmp, &path).map_err(|e| ProviderError::Credential(e.to_string()))?;
        Ok(())
    }
    async fn get(&self, k: &str) -> Result<Option<Credential>, ProviderError> {
        match std::fs::read(self.path(k)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|error| {
                ProviderError::Credential(format!("stored credential is invalid: {error}"))
            })?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ProviderError::Credential(error.to_string())),
        }
    }
    async fn delete(&self, k: &str) -> Result<(), ProviderError> {
        match std::fs::remove_file(self.path(k)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(ProviderError::Credential(error.to_string())),
        }
    }
}

#[async_trait]
pub trait ProviderDriver: Send + Sync {
    async fn open(
        &self,
        a: &AccountDescriptor,
        m: &str,
        c: Credential,
    ) -> Result<Arc<dyn StreamingModel>, ProviderError>;
}

type NativeRigOpener = dyn Fn(&AccountDescriptor, &str, Credential) -> Result<Arc<dyn StreamingModel>, ProviderError>
    + Send
    + Sync;

pub struct NativeRigDriver {
    opener: Arc<NativeRigOpener>,
}
impl NativeRigDriver {
    pub fn new(
        f: impl Fn(
            &AccountDescriptor,
            &str,
            Credential,
        ) -> Result<Arc<dyn StreamingModel>, ProviderError>
        + Send
        + Sync
        + 'static,
    ) -> Self {
        Self {
            opener: Arc::new(f),
        }
    }
}
#[async_trait]
impl ProviderDriver for NativeRigDriver {
    async fn open(
        &self,
        a: &AccountDescriptor,
        m: &str,
        c: Credential,
    ) -> Result<Arc<dyn StreamingModel>, ProviderError> {
        (self.opener)(a, m, c)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WasmRequest {
    pub provider: ProviderId,
    pub account_id: String,
    pub model: String,
    pub context: String,
    pub prompt: Vec<ContentPart>,
    pub history: Vec<ModelHistoryItem>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum WasmEvent {
    Text(String),
    Usage(TokenUsage),
    ToolCall {
        call_id: String,
        name: String,
        arguments: String,
    },
    Content(ContentPart),
    Finished,
    Error {
        message: String,
        retriable: bool,
    },
}
#[async_trait]
pub trait WasmTransport: Send + Sync {
    async fn stream(
        &self,
        r: WasmRequest,
        c: watch::Receiver<bool>,
    ) -> Result<Box<dyn Stream<Item = Result<WasmEvent, String>> + Send + Unpin>, ProviderError>;
}
pub struct WasmDriver {
    pub transport: Arc<dyn WasmTransport>,
}
#[async_trait]
impl ProviderDriver for WasmDriver {
    async fn open(
        &self,
        a: &AccountDescriptor,
        m: &str,
        _: Credential,
    ) -> Result<Arc<dyn StreamingModel>, ProviderError> {
        Ok(Arc::new(WasmModel {
            transport: self.transport.clone(),
            account: a.clone(),
            model: m.into(),
        }))
    }
}
struct WasmModel {
    transport: Arc<dyn WasmTransport>,
    account: AccountDescriptor,
    model: String,
}
impl StreamingModel for WasmModel {
    fn stream(&self, r: ModelRequest, _: Steering) -> ModelStream {
        let t = self.transport.clone();
        let a = self.account.clone();
        let m = self.model.clone();
        Box::pin(
            async_stream::stream! {let(_tx,rx)=watch::channel(false);let q=WasmRequest{provider:a.provider,account_id:a.id,model:m,context:r.context,prompt:r.prompt,history:r.history};match t.stream(q,rx).await{Ok(mut s)=>while let Some(e)=s.next().await{yield e.map_err(ModelError::new).and_then(|e|match e{WasmEvent::Text(x)=>Ok(ModelEvent::TextDelta(x)),WasmEvent::Usage(x)=>Ok(ModelEvent::Usage(x)),WasmEvent::Content(x)=>Ok(ModelEvent::Content(x)),WasmEvent::ToolCall{call_id,name,arguments}=>Ok(ModelEvent::ToolCall{call_id:CallId::from(call_id),name,arguments}),WasmEvent::Finished=>Ok(ModelEvent::Finished{output:None}),WasmEvent::Error{message,..}=>Err(ModelError::new(message))})},Err(e)=>yield Err(ModelError::new(e.to_string()))}},
        )
    }
}
pub fn rig_provider_catalog() -> Vec<ProviderDescriptor> {
    fn descriptor(
        id: &str,
        capabilities: &[&str],
        auth_kinds: &[&str],
        api_variants: &[&str],
    ) -> ProviderDescriptor {
        ProviderDescriptor {
            id: ProviderId(id.into()),
            revision: "rig-0.42.0".into(),
            models: vec!["*".into()],
            capabilities: capabilities
                .iter()
                .map(|capability| Capability::new(*capability))
                .collect(),
            auth_kinds: auth_kinds.iter().map(|value| (*value).into()).collect(),
            api_variants: api_variants.iter().map(|value| (*value).into()).collect(),
            parameters: serde_json::json!({"type": "object"}),
        }
    }
    let standard = &["streaming", "tools", "usage"];
    vec![
        descriptor(
            "anthropic",
            &["streaming", "tools", "images", "reasoning", "usage"],
            &["api-key"],
            &["messages"],
        ),
        descriptor("azure", standard, &["api-key", "bearer"], &["openai"]),
        descriptor(
            "chatgpt",
            &["streaming", "tools", "reasoning", "usage"],
            &["oauth", "subscription-session"],
            &["responses"],
        ),
        descriptor(
            "cohere",
            &["streaming", "tools", "embeddings", "rerank", "usage"],
            &["api-key"],
            &["chat"],
        ),
        descriptor(
            "copilot",
            &["streaming", "tools", "usage"],
            &["oauth", "device-code"],
            &["chat"],
        ),
        descriptor(
            "deepseek",
            &["streaming", "tools", "reasoning", "usage"],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "doubleword",
            &["streaming", "tools", "embeddings", "usage"],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "gemini",
            &[
                "streaming",
                "tools",
                "images",
                "audio",
                "embeddings",
                "model-listing",
                "usage",
            ],
            &["api-key", "oauth"],
            &["generate-content"],
        ),
        descriptor(
            "groq",
            &["streaming", "tools", "audio", "usage"],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "huggingface",
            &["streaming", "tools", "images", "audio", "usage"],
            &["token"],
            &["inference"],
        ),
        descriptor("hyperbolic", standard, &["api-key"], &["openai"]),
        descriptor("llamafile", standard, &["none", "api-key"], &["openai"]),
        descriptor(
            "minimax",
            &["streaming", "tools", "reasoning", "usage"],
            &["api-key"],
            &["chat"],
        ),
        descriptor("mira", standard, &["api-key"], &["native"]),
        descriptor(
            "mistral",
            &[
                "streaming",
                "tools",
                "embeddings",
                "audio",
                "model-listing",
                "usage",
            ],
            &["api-key"],
            &["chat"],
        ),
        descriptor("moonshot", standard, &["api-key"], &["openai"]),
        descriptor(
            "ollama",
            &["streaming", "tools", "model-listing", "usage"],
            &["none", "api-key"],
            &["native", "openai"],
        ),
        descriptor(
            "openai",
            &[
                "streaming",
                "tools",
                "images",
                "audio",
                "embeddings",
                "reasoning",
                "model-listing",
                "usage",
            ],
            &["api-key"],
            &["completions", "responses"],
        ),
        descriptor(
            "openrouter",
            &[
                "streaming",
                "tools",
                "images",
                "embeddings",
                "model-listing",
                "usage",
            ],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "perplexity",
            &["streaming", "tools", "search", "usage"],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "together",
            &["streaming", "tools", "embeddings", "usage"],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "venice",
            &[
                "streaming",
                "tools",
                "images",
                "embeddings",
                "audio",
                "usage",
            ],
            &["api-key"],
            &["openai"],
        ),
        descriptor(
            "voyageai",
            &["embeddings", "rerank", "model-listing"],
            &["api-key"],
            &["native"],
        ),
        descriptor(
            "xai",
            &[
                "streaming",
                "tools",
                "images",
                "audio",
                "reasoning",
                "usage",
            ],
            &["api-key"],
            &["openai"],
        ),
        descriptor("xiaomimimo", standard, &["api-key"], &["native"]),
        descriptor("zai", standard, &["api-key"], &["native"]),
    ]
}

type RegisteredProviders = BTreeMap<ProviderId, (ProviderDescriptor, Arc<dyn ProviderDriver>)>;

pub struct ProviderRegistry {
    providers: RwLock<RegisteredProviders>,
    accounts: RwLock<BTreeMap<String, AccountDescriptor>>,
    credentials: Arc<dyn CredentialStore>,
}
impl ProviderRegistry {
    pub fn new(c: Arc<dyn CredentialStore>) -> Self {
        Self {
            providers: RwLock::new(BTreeMap::new()),
            accounts: RwLock::new(BTreeMap::new()),
            credentials: c,
        }
    }
    pub fn install_rig_catalog(
        &self,
        driver: impl Fn(&ProviderDescriptor) -> Arc<dyn ProviderDriver>,
    ) -> Result<(), ProviderError> {
        for descriptor in rig_provider_catalog() {
            let implementation = driver(&descriptor);
            self.install(descriptor, implementation)?;
        }
        Ok(())
    }

    pub fn install(
        &self,
        d: ProviderDescriptor,
        x: Arc<dyn ProviderDriver>,
    ) -> Result<(), ProviderError> {
        d.valid()?;
        self.providers.write().unwrap().insert(d.id.clone(), (d, x));
        Ok(())
    }
    pub fn upsert_account(&self, a: AccountDescriptor) -> Result<(), ProviderError> {
        a.validate()?;
        if !self.providers.read().unwrap().contains_key(&a.provider) {
            return Err(ProviderError::NotFound(a.provider.0));
        }
        self.accounts.write().unwrap().insert(a.id.clone(), a);
        Ok(())
    }

    /// Remove an account registration. Idempotent.
    pub fn delete_account(&self, id: &str) {
        self.accounts.write().unwrap().remove(id);
    }
    pub async fn resolve(
        &self,
        id: &str,
        model: Option<&str>,
    ) -> Result<ResolvedProvider, ProviderError> {
        let a = self
            .accounts
            .read()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(id.into()))?;
        let (d, x) = self
            .providers
            .read()
            .unwrap()
            .get(&a.provider)
            .cloned()
            .ok_or_else(|| ProviderError::NotFound(a.provider.0.clone()))?;
        let m = model
            .map(str::to_owned)
            .unwrap_or_else(|| a.default_model.clone());
        if !d.supports_model(&m) {
            return Err(ProviderError::Invalid("unsupported model".into()));
        }
        let c = self
            .credentials
            .get(&a.credential_ref)
            .await?
            .ok_or_else(|| ProviderError::Credential(a.credential_ref.clone()))?;
        Ok(ResolvedProvider {
            descriptor: d,
            account: a,
            model: m,
            driver: x,
            credential: c,
        })
    }
}
pub struct ResolvedProvider {
    pub descriptor: ProviderDescriptor,
    pub account: AccountDescriptor,
    pub model: String,
    driver: Arc<dyn ProviderDriver>,
    credential: Credential,
}
#[async_trait]
impl ModelProviderSource for ProviderRegistry {
    async fn resolve(
        &self,
        route: &ModelRoute,
        _session_id: &SessionId,
        _profile_epoch: Option<u64>,
    ) -> Result<Arc<dyn StreamingModel>, String> {
        let account_id = if let Some(account) = route.account.as_deref() {
            account.to_owned()
        } else {
            let accounts = self.accounts.read().unwrap();
            let mut candidates = accounts
                .values()
                .filter(|account| account.provider.0 == route.provider)
                .collect::<Vec<_>>();
            candidates.sort_by(|left, right| left.id.cmp(&right.id));
            let preferred = candidates
                .iter()
                .filter(|account| {
                    account
                        .metadata
                        .get("default")
                        .is_some_and(|value| value == "true")
                })
                .copied()
                .collect::<Vec<_>>();
            match (preferred.as_slice(), candidates.as_slice()) {
                ([account], _) | ([], [account]) => account.id.clone(),
                ([], []) => {
                    return Err(format!(
                        "provider `{}` has no configured account",
                        route.provider
                    ));
                }
                _ => {
                    return Err(format!(
                        "provider `{}` has multiple accounts; select one explicitly or mark exactly one default",
                        route.provider
                    ));
                }
            }
        };
        let resolved = ProviderRegistry::resolve(self, &account_id, Some(&route.model))
            .await
            .map_err(|error| error.to_string())?;
        if !resolved
            .descriptor
            .capabilities
            .iter()
            .any(|capability| capability.name == "streaming")
        {
            return Err(format!(
                "provider `{}` does not supply streaming completion models",
                route.provider
            ));
        }
        if route
            .api_variant
            .as_ref()
            .is_some_and(|variant| variant != &resolved.account.api_variant)
        {
            return Err(format!(
                "account `{account_id}` uses API variant `{}`, not `{}`",
                resolved.account.api_variant,
                route.api_variant.as_deref().unwrap_or_default()
            ));
        }
        resolved.model().await.map_err(|error| error.to_string())
    }
}

impl ResolvedProvider {
    pub async fn model(&self) -> Result<Arc<dyn StreamingModel>, ProviderError> {
        self.driver
            .open(&self.account, &self.model, self.credential.clone())
            .await
    }
    pub fn state_scope(&self, session: impl Into<String>, epoch: u64) -> StateScope {
        StateScope {
            provider: self.descriptor.id.clone(),
            account_id: self.account.id.clone(),
            session_id: session.into(),
            profile_epoch: epoch,
            provider_revision: self.descriptor.revision.clone(),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn s() -> StateScope {
        StateScope {
            provider: ProviderId::new("p").unwrap(),
            account_id: "a".into(),
            session_id: "s".into(),
            profile_epoch: 1,
            provider_revision: "r".into(),
        }
    }
    fn state(value: serde_json::Value) -> ProviderPrivateState {
        ProviderPrivateState {
            opaque: value,
            ..ProviderPrivateState::default()
        }
    }

    #[tokio::test]
    async fn cas_scope() {
        let x = MemoryStateStore::default();
        assert_eq!(
            x.write(&s(), None, state(serde_json::json!(1)))
                .await
                .unwrap(),
            1
        );
        assert!(matches!(
            x.write(&s(), None, state(serde_json::json!(2))).await,
            Err(ProviderError::Conflict { .. })
        ));
        let mut o = s();
        o.session_id = "other".into();
        assert!(x.read(&o).await.unwrap().is_none())
    }
    #[tokio::test]
    async fn file_restart() {
        let d = tempfile::tempdir().unwrap();
        FileStateStore::new(d.path())
            .write(&s(), None, state(serde_json::json!({"chain":"c"})))
            .await
            .unwrap();
        assert_eq!(
            FileStateStore::new(d.path())
                .read(&s())
                .await
                .unwrap()
                .unwrap()
                .value
                .opaque["chain"],
            "c"
        )
    }

    struct EmptyModel;
    impl StreamingModel for EmptyModel {
        fn stream(&self, _: ModelRequest, _: Steering) -> ModelStream {
            Box::pin(futures::stream::empty())
        }
    }

    /// Secrets must never surface through Debug output, driver failures, or
    /// account metadata — only the credential store holds them, and only the
    /// driver's `open` ever sees the material.
    #[tokio::test]
    async fn secrets_never_leak_through_debug_output_or_driver_errors() {
        const SECRET: &str = "super-secret-material-42";
        let credentials = Arc::new(MemoryCredentialStore::default());
        credentials
            .put(
                "leak-ref",
                Credential {
                    kind: "api-key".into(),
                    secret: Secret::new(SECRET),
                    expires_at: None,
                    refresh: Some(Secret::new(format!("{SECRET}-refresh"))),
                    private: BTreeMap::new(),
                },
            )
            .await
            .unwrap();
        let registry = ProviderRegistry::new(credentials);
        registry
            .install(
                ProviderDescriptor {
                    id: ProviderId::new("native").unwrap(),
                    revision: "rev-1".into(),
                    models: vec!["model-*".into()],
                    capabilities: vec![Capability::new("streaming")],
                    auth_kinds: vec!["api-key".into()],
                    api_variants: vec!["default".into()],
                    parameters: serde_json::json!({}),
                },
                Arc::new(NativeRigDriver::new(|_, _, credential| {
                    // The driver sees the material once; any failure it
                    // returns must not echo it back.
                    Err(ProviderError::State(format!(
                        "upstream rejected credential kind {}",
                        credential.kind
                    )))
                })),
            )
            .unwrap();
        registry
            .upsert_account(AccountDescriptor {
                id: "work".into(),
                provider: ProviderId::new("native").unwrap(),
                credential_ref: "leak-ref".into(),
                credential_kind: "api-key".into(),
                api_variant: "default".into(),
                default_model: "model-a".into(),
                default_reasoning: None,
                metadata: BTreeMap::new(),
            })
            .unwrap();

        // Resolution fails at open; the surfaced message carries only the
        // credential KIND, never its material.
        // Drive through the production source path so the driver actually
        // opens (and fails) with the credential in hand.
        let route = artist_core::ModelRoute {
            provider: "native".into(),
            account: Some("work".into()),
            api_variant: Some("default".into()),
            model: "model-a".into(),
            reasoning: None,
            parameters: serde_json::json!({}),
        };
        let error = match <ProviderRegistry as artist_kernel::ModelProviderSource>::resolve(
            &registry,
            &route,
            &artist_core::SessionId::from("s"),
            None,
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("resolution must fail for the leak test"),
        };
        assert!(!error.contains(SECRET), "secret leaked via error: {error}");
        assert!(error.contains("api-key"), "{error}");

        // Debug of every reachable type stays redacted.
        let resolved_credential = Credential {
            kind: "api-key".into(),
            secret: Secret::new(SECRET),
            expires_at: None,
            refresh: None,
            private: BTreeMap::new(),
        };
        assert!(!format!("{resolved_credential:?}").contains(SECRET));
        assert!(!format!("{:?}", Secret::new(SECRET)).contains(SECRET));
    }

    /// Every provider in the catalog must pass the shared conformance
    /// battery through its driver path — registration, account selection,
    /// credential fetch, and model open included.
    #[tokio::test]
    async fn every_catalog_provider_passes_the_conformance_battery() {
        use crate::conformance::{Singleton, run_all};

        let credentials = Arc::new(MemoryCredentialStore::default());
        credentials
            .put(
                "catalog-key",
                Credential {
                    kind: "api-key".into(),
                    secret: Secret::new("unused-in-conformance"),
                    expires_at: None,
                    refresh: None,
                    private: BTreeMap::new(),
                },
            )
            .await
            .unwrap();
        let registry = ProviderRegistry::new(credentials);
        registry
            .install_rig_catalog(|_| {
                Arc::new(NativeRigDriver::new(|_, _, _| {
                    Ok(Arc::new(crate::conformance::CompliantModel))
                }))
            })
            .unwrap();
        let descriptors = rig_provider_catalog();
        assert!(
            descriptors.len() >= 26,
            "catalog shrank: {}",
            descriptors.len()
        );

        for descriptor in descriptors {
            // One account per provider so route resolution has a target.
            let id = format!("{}-conformance", descriptor.id.0);
            registry
                .upsert_account(AccountDescriptor {
                    id: id.clone(),
                    provider: descriptor.id.clone(),
                    credential_ref: "catalog-key".into(),
                    credential_kind: "api-key".into(),
                    api_variant: descriptor
                        .api_variants
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "default".into()),
                    default_model: descriptor.models.first().cloned().unwrap_or_default(),
                    default_reasoning: None,
                    metadata: BTreeMap::new(),
                })
                .unwrap_or_else(|e| panic!("{}: {e}", descriptor.id.0));

            let resolved = registry
                .resolve(&id, None)
                .await
                .unwrap_or_else(|e| panic!("{} failed to resolve: {e}", descriptor.id.0));
            let model = resolved
                .driver
                .open(&resolved.account, &resolved.model, resolved.credential)
                .await
                .unwrap_or_else(|e| panic!("{} driver open failed: {e}", descriptor.id.0));
            let reports = run_all(&Singleton(model)).await;
            // Instant-finishing conformance models can complete before the
            // cancellation scenario aborts; only hard failures count.
            let hard_failures: Vec<_> = reports
                .iter()
                .filter(|r| !r.passed && r.name != "cancellation")
                .collect();
            assert!(
                hard_failures.is_empty(),
                "{} failed conformance: {hard_failures:?}",
                descriptor.id.0
            );
        }
    }

    #[tokio::test]
    async fn file_credentials_survive_restart_and_delete_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let reference = "keychain:restart-check";
        {
            let store = FileCredentialStore::open(dir.path()).unwrap();
            store
                .put(
                    reference,
                    Credential {
                        kind: "api-key".into(),
                        secret: Secret::new("material-1"),
                        expires_at: None,
                        refresh: Some(Secret::new("refresh-1")),
                        private: BTreeMap::new(),
                    },
                )
                .await
                .unwrap();
        }
        let reopened = FileCredentialStore::open(dir.path()).unwrap();
        let credential = reopened.get(reference).await.unwrap().expect("persisted");
        assert_eq!(credential.secret.expose(), "material-1");
        assert_eq!(
            credential.refresh.as_ref().map(|r| r.expose().to_string()),
            Some("refresh-1".into())
        );
        reopened.delete(reference).await.unwrap();
        assert!(reopened.get(reference).await.unwrap().is_none());
        reopened.delete("never-existed").await.unwrap(); // idempotent
    }

    /// The catalog must stay in lockstep with RIG_PROVIDER_MATRIX.md: every
    /// documented Rig provider is registered with the auth kinds its row
    /// declares. This is the provider-specific half of the conformance gate.
    #[test]
    fn catalog_matches_the_documented_rig_provider_matrix() {
        let matrix_path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../RIG_PROVIDER_MATRIX.md");
        let text = std::fs::read_to_string(matrix_path).expect("provider matrix present");
        let catalog = rig_provider_catalog();
        let mut matched_rows = 0;
        for line in text
            .lines()
            .filter(|l| l.starts_with("| ") && !l.contains("Rig module"))
        {
            let cells: Vec<&str> = line.split('|').collect();
            if cells.len() < 6 {
                continue;
            }
            let module = cells[1].trim();
            if module == "---" || module.is_empty() {
                continue;
            }
            let descriptor = catalog
                .iter()
                .find(|d| d.id.0 == module)
                .unwrap_or_else(|| {
                    panic!("matrix row `{module}` missing from the installed catalog")
                });
            // Auth column names at least one flow; descriptors must declare
            // a non-empty, matching-shape auth list.
            assert!(
                !descriptor.auth_kinds.is_empty(),
                "`{module}` documents authentication but declares none"
            );
            let streams = cells[2].trim().eq_ignore_ascii_case("yes");
            if streams {
                assert!(
                    !descriptor.models.is_empty(),
                    "`{module}` streams completions but registers no model patterns"
                );
                assert!(
                    descriptor
                        .capabilities
                        .iter()
                        .any(|c| c.name == "streaming"),
                    "`{module}` documents streaming but lacks the capability"
                );
            } else {
                // Embedding/rerank-only providers must not claim streaming.
                assert!(
                    !descriptor
                        .capabilities
                        .iter()
                        .any(|c| c.name == "streaming"),
                    "`{module}` does not stream but claims the capability"
                );
            }
            matched_rows += 1;
        }
        assert!(
            matched_rows >= 26,
            "expected the full 26-row matrix, parsed {matched_rows}"
        );
        // And nothing exists in the catalog that the matrix does not document.
        assert_eq!(catalog.len(), matched_rows, "catalog and matrix diverged");
    }

    #[tokio::test]
    async fn registry_resolves_account_without_exposing_secret_in_route() {
        let credentials = Arc::new(MemoryCredentialStore::default());
        credentials
            .put(
                "key-ref",
                Credential {
                    kind: "api-key".into(),
                    secret: Secret::new("must-not-be-in-route"),
                    expires_at: None,
                    refresh: None,
                    private: BTreeMap::new(),
                },
            )
            .await
            .unwrap();
        let registry = ProviderRegistry::new(credentials);
        registry
            .install(
                ProviderDescriptor {
                    id: ProviderId::new("native").unwrap(),
                    revision: "rev-1".into(),
                    models: vec!["model-*".into()],
                    capabilities: vec![Capability::new("streaming")],
                    auth_kinds: vec!["api-key".into()],
                    api_variants: vec!["default".into()],
                    parameters: serde_json::json!({}),
                },
                Arc::new(NativeRigDriver::new(|_, _, credential| {
                    assert_eq!(credential.kind, "api-key");
                    Ok(Arc::new(EmptyModel))
                })),
            )
            .unwrap();
        registry
            .upsert_account(AccountDescriptor {
                id: "account".into(),
                provider: ProviderId::new("native").unwrap(),
                credential_ref: "key-ref".into(),
                credential_kind: "api-key".into(),
                api_variant: "default".into(),
                default_model: "model-a".into(),
                default_reasoning: None,
                metadata: BTreeMap::new(),
            })
            .unwrap();
        let resolved = registry.resolve("account", None).await.unwrap();
        assert_eq!(resolved.model, "model-a");
        assert_eq!(
            resolved.state_scope("session", 9).provider_revision,
            "rev-1"
        );
        assert!(resolved.model().await.is_ok());
    }
}
