use crate::status_bar::StatusBarConfig;
use anyhow::{Context, Result, bail};
use llm_provider::{ProviderId, SavedProvider};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ProviderStore {
    #[serde(default = "version")]
    pub version: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_provider: Option<ProviderId>,
    #[serde(default)]
    pub providers: Vec<SavedProvider>,
    #[serde(default)]
    pub status_bar: StatusBarConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disabled_tools: Vec<String>,
}
fn version() -> u8 {
    2
}

impl ProviderStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: 2,
                ..Self::default()
            });
        }
        let contents = fs::read_to_string(path).context("read providers.toml")?;
        let mut document: toml::Value =
            toml::from_str(&contents).context("parse providers.toml")?;
        migrate_legacy_providers(&mut document);
        let store: Self = document.try_into().context("decode providers.toml")?;
        store.validate()?;
        Ok(store)
    }
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.version = 2;
        self.validate()?;
        let parent = path.parent().context("providers path has no parent")?;
        fs::create_dir_all(parent).context("create Artist config directory")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let temp = path.with_extension("toml.tmp");
        fs::write(&temp, toml::to_string_pretty(self)?).context("write providers file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        }
        fs::rename(temp, path).context("replace providers file")?;
        Ok(())
    }
    pub fn add(&mut self, provider: SavedProvider) {
        if self.default_provider.is_none() {
            self.default_provider = Some(provider.id.clone());
        }
        self.providers.push(provider);
    }
    fn validate(&self) -> Result<()> {
        for (i, p) in self.providers.iter().enumerate() {
            if self.providers[..i].iter().any(|other| other.id == p.id) {
                bail!("duplicate provider id: {}", p.id.as_str());
            }
        }
        if let Some(id) = &self.default_provider
            && !self.providers.iter().any(|p| &p.id == id)
        {
            bail!("default provider does not exist: {}", id.as_str());
        }
        Ok(())
    }
}

/// Bring a `providers.toml` up to the current schema. Pre-multi-provider
/// ChatGPT rows predate the tagged `Auth` enum: their `[providers.auth]` table
/// has no `type` key, so stamp `type = "chat_gpt"` to let it deserialize. The
/// `kind` field defaults to `chat_gpt` via serde, so nothing to stamp there.
///
/// (This used to *strip* every API-key provider to force ChatGPT-only; now that
/// multiple providers are supported again, it preserves them.)
fn migrate_legacy_providers(document: &mut toml::Value) {
    let Some(table) = document.as_table_mut() else {
        return;
    };
    table.insert("version".into(), toml::Value::Integer(2));
    if let Some(providers) = table
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    {
        for provider in providers.iter_mut() {
            if let Some(auth) = provider.get_mut("auth").and_then(toml::Value::as_table_mut)
                && !auth.contains_key("type")
            {
                auth.insert("type".into(), toml::Value::String("chat_gpt".into()));
            }
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path).join("providers.toml"));
    }
    let home = dirs::home_dir().context("could not find home directory")?;
    let root = home.join(".artist");
    migrate_legacy_root(&root)?;
    Ok(root.join("providers.toml"))
}

/// Move the pre-.artist global state into the conventional Artist home once.
/// Existing files in the destination win; legacy-only files are copied across.
fn migrate_legacy_root(root: &Path) -> Result<()> {
    let Some(config) = dirs::config_dir() else {
        return Ok(());
    };
    migrate_root_between(&config.join("artist"), root)
}

/// The migration mechanics, split out from the `dirs`-derived paths so the
/// merge behaviour is unit-testable. When the destination is absent the legacy
/// tree is moved wholesale (falling back to a copy across filesystems); when
/// both exist, only legacy-only files are copied in — destination files always
/// win, so a partially-migrated home is never clobbered.
fn migrate_root_between(legacy: &Path, root: &Path) -> Result<()> {
    if !legacy.is_dir() || legacy == root {
        return Ok(());
    }
    if !root.exists() {
        fs::rename(legacy, root).or_else(|_| copy_tree(legacy, root))?;
        return Ok(());
    }
    copy_tree(legacy, root)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            // Recurse even into an existing destination dir so legacy-only
            // nested files merge in; the per-file guard below keeps destination
            // files winning.
            copy_tree(&from, &to)?;
        } else if !to.exists() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{Auth, ProviderKind, SavedProvider, Secret};
    #[test]
    fn keeps_api_key_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(
            &path,
            r#"version = 2
default_provider = "api"
[[providers]]
id = "api"
name = "API"
base_url = "https://api.example/v1/"
kind = "open_ai"
[providers.auth]
type = "api_key"
api_key = "secret"
"#,
        )
        .unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert_eq!(store.providers.len(), 1);
        assert_eq!(store.providers[0].kind, ProviderKind::OpenAi);
        assert_eq!(store.providers[0].auth.api_key(), Some("secret"));
    }

    #[test]
    fn stamps_type_on_legacy_chatgpt_row() {
        // A pre-tagged-enum ChatGPT row: no `auth.type`, no `kind`.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(
            &path,
            r#"version = 1
default_provider = "chatgpt"
[[providers]]
id = "chatgpt"
name = "ChatGPT"
base_url = "https://chatgpt.com/backend-api/codex/"
[providers.auth]
access_token = "a"
refresh_token = "r"
account_id = "acct"
"#,
        )
        .unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert_eq!(store.providers.len(), 1);
        assert_eq!(store.providers[0].kind, ProviderKind::ChatGpt);
        assert_eq!(store.providers[0].auth.account_id(), Some("acct"));
    }

    #[test]
    fn old_config_gets_default_status_bar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(&path, "version = 2\nproviders = []\n").unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert_eq!(store.status_bar, StatusBarConfig::default());
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    #[test]
    fn migrates_legacy_root_when_destination_absent() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("config/artist");
        let root = dir.path().join(".artist");
        write_file(&legacy.join("providers.toml"), "version = 2\n");
        write_file(&legacy.join("rules/one.md"), "rule\n");

        migrate_root_between(&legacy, &root).unwrap();

        assert!(!legacy.exists(), "legacy root should be moved away");
        assert_eq!(
            fs::read_to_string(root.join("providers.toml")).unwrap(),
            "version = 2\n"
        );
        assert_eq!(fs::read_to_string(root.join("rules/one.md")).unwrap(), "rule\n");
    }

    #[test]
    fn merges_legacy_only_files_without_overwriting_destination() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("config/artist");
        let root = dir.path().join(".artist");
        // Destination already has providers.toml (must win) but no rules.
        write_file(&root.join("providers.toml"), "version = 2\nkept = true\n");
        write_file(&legacy.join("providers.toml"), "version = 1\nstale = true\n");
        write_file(&legacy.join("rules/one.md"), "rule\n");

        migrate_root_between(&legacy, &root).unwrap();

        // Destination file preserved; legacy-only file copied in.
        assert_eq!(
            fs::read_to_string(root.join("providers.toml")).unwrap(),
            "version = 2\nkept = true\n"
        );
        assert_eq!(fs::read_to_string(root.join("rules/one.md")).unwrap(), "rule\n");
        // Legacy is left in place when merging (both existed).
        assert!(legacy.exists());
    }

    #[test]
    fn migration_is_a_noop_without_legacy_root() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("config/artist");
        let root = dir.path().join(".artist");
        write_file(&root.join("providers.toml"), "version = 2\n");

        migrate_root_between(&legacy, &root).unwrap();

        assert!(!legacy.exists());
        assert_eq!(
            fs::read_to_string(root.join("providers.toml")).unwrap(),
            "version = 2\n"
        );
    }

    #[test]
    fn copy_tree_recurses_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        let destination = dir.path().join("dst");
        write_file(&source.join("a.txt"), "from-source\n");
        write_file(&source.join("nested/b.txt"), "nested\n");
        write_file(&destination.join("a.txt"), "from-destination\n");

        copy_tree(&source, &destination).unwrap();

        // Existing destination file wins; new files and subdirs are copied.
        assert_eq!(
            fs::read_to_string(destination.join("a.txt")).unwrap(),
            "from-destination\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("nested/b.txt")).unwrap(),
            "nested\n"
        );
    }

    #[test]
    fn copy_tree_merges_into_existing_subdirectory() {
        // When the destination subdir already exists, legacy-only files under
        // it must still merge in (rather than the whole subtree being skipped).
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        let destination = dir.path().join("dst");
        write_file(&source.join("rules/legacy.md"), "legacy\n");
        write_file(&destination.join("rules/kept.md"), "kept\n");

        copy_tree(&source, &destination).unwrap();

        assert_eq!(
            fs::read_to_string(destination.join("rules/legacy.md")).unwrap(),
            "legacy\n"
        );
        assert_eq!(
            fs::read_to_string(destination.join("rules/kept.md")).unwrap(),
            "kept\n"
        );
    }

    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore {
            version: 1,
            ..Default::default()
        };
        store.disabled_tools = vec!["bash".into()];
        store.add(SavedProvider::chatgpt(
            ProviderId::new("one").unwrap(),
            "ChatGPT",
            Auth::ChatGpt {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: None,
                expires_at: None,
            },
        ));
        store.save(&path).unwrap();
        let loaded = ProviderStore::load(&path).unwrap();
        assert_eq!(loaded.providers.len(), 1);
        assert_eq!(loaded.disabled_tools, ["bash"]);
        assert_eq!(loaded.default_provider.unwrap().as_str(), "one");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
