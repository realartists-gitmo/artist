//! Durable storage primitives shared by resource providers.
//! The traits deliberately deal in bytes and stable keys; backends can be used by
//! native, embedded, or remote implementations without leaking a filesystem API.
use artist_core::{BlobRef, SessionId};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("storage I/O: {0}")]
    Io(#[from] io::Error),
    #[error("storage serialization: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("invalid key: {0}")]
    InvalidKey(String),
    #[error("resource conflict; current revision is {0}")]
    Conflict(String),
    #[error("blob not found: {0}")]
    BlobNotFound(String),
}

pub type StorageResult<T> = Result<T, StorageError>;

/// The host-computed identity of a blob. Canonical references live in
/// `artist-core`; stores use `BlobId` internally for validated lookup keys.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlobStat {
    pub id: BlobId,
    pub byte_length: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceDocument {
    pub scope: String,
    pub key: String,
    pub revision: u64,
    pub value: Vec<u8>,
}

#[async_trait]
pub trait ScopedResourceStore: Send + Sync {
    async fn read(&self, scope: &str, key: &str) -> StorageResult<Option<ResourceDocument>>;
    async fn write(
        &self,
        scope: &str,
        key: &str,
        value: Vec<u8>,
        expected_revision: Option<u64>,
    ) -> StorageResult<ResourceDocument>;
    async fn delete(
        &self,
        scope: &str,
        key: &str,
        expected_revision: Option<u64>,
    ) -> StorageResult<bool>;
    async fn list(&self, scope: &str) -> StorageResult<Vec<ResourceDocument>>;
}

#[derive(Clone, Default)]
pub struct MemoryResourceStore {
    data: Arc<RwLock<BTreeMap<(String, String), ResourceDocument>>>,
}
impl MemoryResourceStore {
    pub fn new() -> Self {
        Self::default()
    }
}
#[async_trait]
impl ScopedResourceStore for MemoryResourceStore {
    async fn read(&self, s: &str, k: &str) -> StorageResult<Option<ResourceDocument>> {
        Ok(self
            .data
            .read()
            .unwrap()
            .get(&(s.into(), k.into()))
            .cloned())
    }
    async fn write(
        &self,
        s: &str,
        k: &str,
        v: Vec<u8>,
        expected: Option<u64>,
    ) -> StorageResult<ResourceDocument> {
        validate_key(s)?;
        validate_key(k)?;
        let mut d = self.data.write().unwrap();
        let old = d.get(&(s.into(), k.into()));
        check_revision(old, expected)?;
        let x = ResourceDocument {
            scope: s.into(),
            key: k.into(),
            revision: old.map_or(1, |x| x.revision + 1),
            value: v,
        };
        d.insert((s.into(), k.into()), x.clone());
        Ok(x)
    }
    async fn delete(&self, s: &str, k: &str, expected: Option<u64>) -> StorageResult<bool> {
        let mut d = self.data.write().unwrap();
        let old = d.get(&(s.into(), k.into()));
        check_revision(old, expected)?;
        Ok(d.remove(&(s.into(), k.into())).is_some())
    }
    async fn list(&self, s: &str) -> StorageResult<Vec<ResourceDocument>> {
        Ok(self
            .data
            .read()
            .unwrap()
            .values()
            .filter(|x| x.scope == s)
            .cloned()
            .collect())
    }
}

/// A restart-safe store. Each scope is a directory and each document is an
/// encoded file; replacement is temp-file + fsync + rename.
#[derive(Clone)]
pub struct FileResourceStore {
    root: Arc<PathBuf>,
    lock: Arc<std::sync::Mutex<()>>,
}
impl FileResourceStore {
    pub fn open(root: impl Into<PathBuf>) -> StorageResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        Ok(Self {
            root: Arc::new(root),
            lock: Arc::new(std::sync::Mutex::new(())),
        })
    }
    fn path(&self, s: &str, k: &str) -> StorageResult<PathBuf> {
        validate_key(s)?;
        validate_key(k)?;
        let dir = self.root.join(s);
        fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("{}.json", safe_name(k))))
    }
    fn load(&self, s: &str, k: &str) -> StorageResult<Option<ResourceDocument>> {
        let p = self.path(s, k)?;
        match fs::read(p) {
            Ok(x) => Ok(Some(serde_json::from_slice(&x)?)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    fn atomic_write(&self, p: &Path, x: &ResourceDocument) -> StorageResult<()> {
        let tmp = p.with_extension(format!("tmp-{}", Uuid::new_v4()));
        let bytes = serde_json::to_vec(x)?;
        {
            use std::io::Write;
            let mut f = fs::File::create(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
        }
        fs::rename(tmp, p)?;
        if let Some(parent) = p.parent() {
            let _ = fs::File::open(parent).and_then(|f| f.sync_all());
        }
        Ok(())
    }
}
#[async_trait]
impl ScopedResourceStore for FileResourceStore {
    async fn read(&self, s: &str, k: &str) -> StorageResult<Option<ResourceDocument>> {
        self.load(s, k)
    }
    async fn write(
        &self,
        s: &str,
        k: &str,
        v: Vec<u8>,
        expected: Option<u64>,
    ) -> StorageResult<ResourceDocument> {
        let _guard = self.lock.lock().unwrap();
        let process_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".store.lock"))?;
        fs2::FileExt::lock_exclusive(&process_lock)?;
        let old = self.load(s, k)?;
        check_revision(old.as_ref(), expected)?;
        let x = ResourceDocument {
            scope: s.into(),
            key: k.into(),
            revision: old.map_or(1, |x| x.revision + 1),
            value: v,
        };
        self.atomic_write(&self.path(s, k)?, &x)?;
        Ok(x)
    }
    async fn delete(&self, s: &str, k: &str, expected: Option<u64>) -> StorageResult<bool> {
        let _guard = self.lock.lock().unwrap();
        let process_lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(".store.lock"))?;
        fs2::FileExt::lock_exclusive(&process_lock)?;
        let old = self.load(s, k)?;
        check_revision(old.as_ref(), expected)?;
        if old.is_some() {
            match fs::remove_file(self.path(s, k)?) {
                Ok(()) => Ok(true),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
                Err(e) => Err(e.into()),
            }
        } else {
            Ok(false)
        }
    }
    async fn list(&self, s: &str) -> StorageResult<Vec<ResourceDocument>> {
        validate_key(s)?;
        let dir = self.root.join(s);
        let mut out: Vec<ResourceDocument> = Vec::new();
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd {
                let p = e?.path();
                if p.extension().and_then(|x| x.to_str()) == Some("json") {
                    out.push(serde_json::from_slice(&fs::read(p)?)?)
                }
            }
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }
}
fn check_revision(old: Option<&ResourceDocument>, expected: Option<u64>) -> StorageResult<()> {
    if expected != old.map(|x| x.revision) && expected.is_some() {
        return Err(StorageError::Conflict(
            old.map_or(0, |x| x.revision).to_string(),
        ));
    }
    Ok(())
}
fn validate_key(x: &str) -> StorageResult<()> {
    if x.is_empty()
        || x == "."
        || x == ".."
        || x.contains('/')
        || x.contains('\\')
        || x.contains('\0')
    {
        Err(StorageError::InvalidKey(x.into()))
    } else {
        Ok(())
    }
}
fn safe_name(x: &str) -> String {
    let mut h = Sha256::new();
    h.update(x.as_bytes());
    format!("{:x}", h.finalize())
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct BlobId(pub String);
impl BlobId {
    pub fn parse(x: impl Into<String>) -> StorageResult<Self> {
        let x = x.into();
        if x.len() != 64 || !x.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(StorageError::InvalidKey(x));
        }
        Ok(Self(x.to_ascii_lowercase()))
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlobLease {
    pub id: BlobId,
    pub lease_id: String,
    pub expires_at: u64,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum BlobRoot {
    DurableResource { scope: String, key: String },
    Transcript { session: String },
    ProviderState { provider: String },
    PluginPackage { plugin: String },
    Account { account: String },
    Other { namespace: String, key: String },
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct BlobReference {
    pub id: BlobId,
    pub root: BlobRoot,
}
#[async_trait]
pub trait BlobRootEnumerator: Send + Sync {
    async fn list_blob_refs(&self) -> StorageResult<Vec<BlobReference>>;
}
#[async_trait]
pub trait BlobStore: Send + Sync {
    async fn put(&self, data: Vec<u8>) -> StorageResult<BlobId>;
    async fn get(&self, id: &BlobId) -> StorageResult<Option<Vec<u8>>>;
    async fn contains(&self, id: &BlobId) -> StorageResult<bool>;
    async fn remove(&self, id: &BlobId) -> StorageResult<bool>;
    async fn list(&self) -> StorageResult<Vec<BlobId>>;
    async fn stat(&self, id: &BlobId) -> StorageResult<Option<BlobStat>> {
        Ok(self.get(id).await?.map(|data| BlobStat {
            id: id.clone(),
            byte_length: data.len() as u64,
        }))
    }
    async fn get_range(
        &self,
        id: &BlobId,
        start: u64,
        end_exclusive: u64,
    ) -> StorageResult<Option<Vec<u8>>> {
        let Some(data) = self.get(id).await? else {
            return Ok(None);
        };
        let start = usize::try_from(start)
            .map_err(|_| StorageError::InvalidKey("range start overflow".into()))?;
        let end = usize::try_from(end_exclusive)
            .map_err(|_| StorageError::InvalidKey("range end overflow".into()))?;
        if start > end || end > data.len() {
            return Err(StorageError::InvalidKey("invalid blob range".into()));
        }
        Ok(Some(data[start..end].to_vec()))
    }
    async fn put_ref(
        &self,
        data: Vec<u8>,
        media_type: String,
        logical_name: Option<String>,
    ) -> StorageResult<BlobRef> {
        let byte_length = data.len() as u64;
        let digest = self.put(data).await?;
        let reference = BlobRef {
            algorithm: "sha256".into(),
            digest: digest.0,
            byte_length,
            media_type,
            logical_name,
        };
        reference.validate().map_err(StorageError::InvalidKey)?;
        Ok(reference)
    }
}

#[derive(Clone, Default)]
pub struct MemoryBlobStore {
    blobs: Arc<RwLock<BTreeMap<BlobId, Vec<u8>>>>,
}
#[async_trait]
impl BlobStore for MemoryBlobStore {
    async fn put(&self, d: Vec<u8>) -> StorageResult<BlobId> {
        let id = hash_blob(&d);
        self.blobs.write().unwrap().entry(id.clone()).or_insert(d);
        Ok(id)
    }
    async fn get(&self, id: &BlobId) -> StorageResult<Option<Vec<u8>>> {
        Ok(self.blobs.read().unwrap().get(id).cloned())
    }
    async fn contains(&self, id: &BlobId) -> StorageResult<bool> {
        Ok(self.blobs.read().unwrap().contains_key(id))
    }
    async fn remove(&self, id: &BlobId) -> StorageResult<bool> {
        Ok(self.blobs.write().unwrap().remove(id).is_some())
    }
    async fn list(&self) -> StorageResult<Vec<BlobId>> {
        Ok(self.blobs.read().unwrap().keys().cloned().collect())
    }
}
fn hash_blob(d: &[u8]) -> BlobId {
    let mut h = Sha256::new();
    h.update(d);
    BlobId(format!("{:x}", h.finalize()))
}

#[derive(Clone)]
pub struct AtomicFileBlobStore {
    root: Arc<PathBuf>,
    leases: Arc<RwLock<BTreeMap<(BlobId, String), u64>>>,
    quarantine: Arc<RwLock<BTreeMap<BlobId, u64>>>,
}
#[derive(Serialize, Deserialize, Default)]
struct GcState {
    leases: Vec<BlobLease>,
    quarantine: Vec<(BlobId, u64)>,
}
impl AtomicFileBlobStore {
    pub fn open(root: impl Into<PathBuf>) -> StorageResult<Self> {
        let root = root.into();
        fs::create_dir_all(&root)?;
        let state_path = root.join(".gc-state.json");
        let state: GcState = match fs::read(&state_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => GcState::default(),
            Err(e) => return Err(e.into()),
        };
        let leases = state
            .leases
            .into_iter()
            .map(|x| ((x.id, x.lease_id), x.expires_at))
            .collect();
        let quarantine = state.quarantine.into_iter().collect();
        Ok(Self {
            root: Arc::new(root),
            leases: Arc::new(RwLock::new(leases)),
            quarantine: Arc::new(RwLock::new(quarantine)),
        })
    }
    fn path(&self, id: &BlobId) -> PathBuf {
        self.root.join(&id.0)
    }
    fn persist_gc_state(&self) -> StorageResult<()> {
        let leases = self
            .leases
            .read()
            .unwrap()
            .iter()
            .map(|((id, lease_id), expires_at)| BlobLease {
                id: id.clone(),
                lease_id: lease_id.clone(),
                expires_at: *expires_at,
            })
            .collect();
        let quarantine = self
            .quarantine
            .read()
            .unwrap()
            .iter()
            .map(|(id, due)| (id.clone(), *due))
            .collect();
        let bytes = serde_json::to_vec(&GcState { leases, quarantine })?;
        let tmp = self.root.join(format!(".gc-state.tmp-{}", Uuid::new_v4()));
        fs::write(&tmp, bytes)?;
        fs::rename(tmp, self.root.join(".gc-state.json"))?;
        Ok(())
    }
}
#[async_trait]
impl BlobStore for AtomicFileBlobStore {
    async fn put(&self, d: Vec<u8>) -> StorageResult<BlobId> {
        let id = hash_blob(&d);
        let p = self.path(&id);
        if !p.exists() {
            let t = p.with_extension(format!("tmp-{}", Uuid::new_v4()));
            fs::write(&t, &d)?;
            fs::rename(t, p)?;
        }
        Ok(id)
    }
    async fn get(&self, id: &BlobId) -> StorageResult<Option<Vec<u8>>> {
        match fs::read(self.path(id)) {
            Ok(x) => Ok(Some(x)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
    async fn contains(&self, id: &BlobId) -> StorageResult<bool> {
        Ok(self.path(id).is_file())
    }
    async fn remove(&self, id: &BlobId) -> StorageResult<bool> {
        match fs::remove_file(self.path(id)) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }
    async fn list(&self) -> StorageResult<Vec<BlobId>> {
        let mut x = Vec::new();
        for e in fs::read_dir(&*self.root)? {
            let p = e?.path();
            if p.is_file()
                && let Some(s) = p.file_name().and_then(|x| x.to_str())
                && let Ok(id) = BlobId::parse(s)
            {
                x.push(id)
            }
        }
        x.sort();
        Ok(x)
    }
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
impl AtomicFileBlobStore {
    pub fn lease(
        &self,
        id: BlobId,
        lease_id: impl Into<String>,
        ttl: Duration,
    ) -> StorageResult<()> {
        self.leases
            .write()
            .unwrap()
            .insert((id, lease_id.into()), now_secs() + ttl.as_secs());
        self.persist_gc_state()
    }
    pub fn release_lease(&self, id: &BlobId, lease_id: &str) -> StorageResult<()> {
        self.leases
            .write()
            .unwrap()
            .remove(&(id.clone(), lease_id.into()));
        self.persist_gc_state()
    }
    pub fn quarantine(&self, id: BlobId, grace: Duration) -> StorageResult<()> {
        self.quarantine
            .write()
            .unwrap()
            .insert(id, now_secs() + grace.as_secs());
        self.persist_gc_state()
    }
    pub async fn collect(
        &self,
        roots: &[BlobId],
        enumerators: &[Arc<dyn BlobRootEnumerator>],
    ) -> StorageResult<Vec<BlobId>> {
        let mut live = roots.iter().cloned().collect::<BTreeSet<_>>();
        for e in enumerators {
            for r in e.list_blob_refs().await? {
                live.insert(r.id);
            }
        }
        let now = now_secs();
        {
            // Scoped so the lock guard never spans an await point.
            let leases = self.leases.read().unwrap();
            for ((id, _), expiry) in leases.iter() {
                if *expiry > now {
                    live.insert(id.clone());
                }
            }
        }
        let ids = self.list().await?;
        let mut removed = Vec::new();
        for id in ids {
            if live.contains(&id) {
                if self.quarantine.write().unwrap().remove(&id).is_some() {
                    self.persist_gc_state()?;
                }
                continue;
            }
            let due = self.quarantine.read().unwrap().get(&id).copied();
            match due {
                None => {
                    self.quarantine(id.clone(), Duration::from_secs(3600))?;
                }
                Some(due) if due <= now => {
                    drop(self.quarantine.read().unwrap());
                    self.remove(&id).await?;
                    self.quarantine.write().unwrap().remove(&id);
                    self.persist_gc_state()?;
                    removed.push(id)
                }
                Some(_) => {}
            }
        }
        Ok(removed)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(tag = "scope", content = "id", rename_all = "snake_case")]
pub enum ResourceScope {
    Global,
    Account(String),
    Identity(String),
    Workspace(String),
    Profile(String),
    Session(SessionId),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ViewIdentity {
    pub session_id: SessionId,
    pub view_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceLayer {
    pub scope: ResourceScope,
    pub resource_root: String,
    pub revision: String,
    pub precedence: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceView {
    pub identity: ViewIdentity,
    pub layers: Vec<ResourceLayer>,
}
impl ResourceView {
    pub fn new(session_id: SessionId, view_id: impl Into<String>) -> StorageResult<Self> {
        let view_id = view_id.into();
        if view_id.is_empty() {
            return Err(StorageError::InvalidKey("resource view ID is empty".into()));
        }
        Ok(Self {
            identity: ViewIdentity {
                session_id,
                view_id,
            },
            layers: Vec::new(),
        })
    }

    pub fn compose(mut self, layer: ResourceLayer) -> StorageResult<Self> {
        if layer.resource_root.is_empty()
            || layer.revision.is_empty()
            || self.layers.iter().any(|existing| {
                existing.precedence == layer.precedence || existing.scope == layer.scope
            })
        {
            return Err(StorageError::InvalidKey(
                "resource view layers require roots/revisions and unique scopes/precedence".into(),
            ));
        }
        self.layers.push(layer);
        self.layers.sort_by_key(|layer| layer.precedence);
        Ok(self)
    }
    pub fn identity(&self) -> &ViewIdentity {
        &self.identity
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;
    #[tokio::test]
    async fn memory_cas_is_concurrent_and_scoped() {
        let s = MemoryResourceStore::new();
        let x = s.write("a", "k", b"one".into(), None).await.unwrap();
        assert!(matches!(
            s.write("a", "k", b"bad".into(), Some(9)).await,
            Err(StorageError::Conflict(_))
        ));
        let (a, b) = tokio::join!(
            s.write("a", "x", vec![1], None),
            s.write("b", "x", vec![2], None)
        );
        assert!(a.is_ok() && b.is_ok());
        assert_eq!(s.list("a").await.unwrap().len(), 2);
        assert_eq!(x.revision, 1);
        let (left, right) = tokio::join!(
            s.write("a", "k", b"left".into(), Some(x.revision)),
            s.write("a", "k", b"right".into(), Some(x.revision))
        );
        assert_eq!(left.is_ok() as u8 + right.is_ok() as u8, 1);
    }
    #[tokio::test]
    async fn file_store_survives_restart_and_conflicts() {
        let d = tempdir().unwrap();
        let s = FileResourceStore::open(d.path()).unwrap();
        let x = s.write("scope", "key", b"v".into(), None).await.unwrap();
        drop(s);
        let s = FileResourceStore::open(d.path()).unwrap();
        assert_eq!(s.read("scope", "key").await.unwrap().unwrap().value, b"v");
        assert!(matches!(
            s.write("scope", "key", b"x".into(), Some(0)).await,
            Err(StorageError::Conflict(_))
        ));
        assert!(
            s.write("scope", "key", b"x".into(), Some(x.revision))
                .await
                .is_ok()
        );
    }
    struct Roots(BlobId);
    #[async_trait]
    impl BlobRootEnumerator for Roots {
        async fn list_blob_refs(&self) -> StorageResult<Vec<BlobReference>> {
            Ok(vec![BlobReference {
                id: self.0.clone(),
                root: BlobRoot::Transcript {
                    session: "s".into(),
                },
            }])
        }
    }
    #[tokio::test]
    async fn blobs_are_deduplicated_and_quarantined_before_gc() {
        let d = tempdir().unwrap();
        let s = AtomicFileBlobStore::open(d.path()).unwrap();
        let live = s.put(b"live".to_vec()).await.unwrap();
        let dead = s.put(b"dead".to_vec()).await.unwrap();
        assert_eq!(s.put(b"live".to_vec()).await.unwrap(), live);
        assert_eq!(s.get_range(&live, 1, 3).await.unwrap().unwrap(), b"iv");
        assert_eq!(s.stat(&live).await.unwrap().unwrap().byte_length, 4);
        let refs: Vec<Arc<dyn BlobRootEnumerator>> = vec![Arc::new(Roots(live.clone()))];
        assert!(s.collect(&[], &refs).await.unwrap().is_empty());
        assert!(s.contains(&dead).await.unwrap());
        assert!(s.collect(&[live], &[]).await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn leases_survive_a_store_restart() {
        let d = tempdir().unwrap();
        let s = AtomicFileBlobStore::open(d.path()).unwrap();
        let id = s.put(b"leased".to_vec()).await.unwrap();
        s.lease(id.clone(), "upload", Duration::from_secs(600))
            .unwrap();
        drop(s);
        let s = AtomicFileBlobStore::open(d.path()).unwrap();
        assert!(s.collect(&[], &[]).await.unwrap().is_empty());
        s.release_lease(&id, "upload").unwrap();
        assert!(s.collect(&[], &[]).await.unwrap().is_empty());
    }
    #[test]
    fn view_identity_is_explicit_and_composable() {
        let layer = ResourceLayer {
            scope: ResourceScope::Workspace("project-a".into()),
            resource_root: "file:///project-a".into(),
            revision: "revision-1".into(),
            precedence: 10,
        };
        let v = ResourceView::new(SessionId::from("session-a"), "coding")
            .unwrap()
            .compose(layer.clone())
            .unwrap();
        assert_eq!(v.identity().view_id, "coding");
        assert_eq!(v.layers, vec![layer]);
        let other = ResourceView::new(SessionId::from("session-b"), "coding").unwrap();
        assert_ne!(v.identity(), other.identity());
    }
}
