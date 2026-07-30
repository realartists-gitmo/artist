use crate::status_bar::StatusBarConfig;
use anyhow::{Context, Result, bail};
use llm_provider::{ProviderId, SavedProvider};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
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
    4
}

impl ProviderStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: version(),
                ..Self::default()
            });
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))
                .context("secure providers.toml permissions")?;
        }
        let contents = fs::read_to_string(path).context("read providers.toml")?;
        let mut document: toml::Value =
            toml::from_str(&contents).context("parse providers.toml")?;
        let previous_version = document
            .get("version")
            .and_then(toml::Value::as_integer)
            .unwrap_or(1);
        migrate_provider_credentials(&mut document, previous_version);
        migrate_session_tokens(&mut document, previous_version);
        let store: Self = document.try_into().context("decode providers.toml")?;
        store.validate()?;
        Ok(store)
    }
    pub fn save(&mut self, path: &Path) -> Result<()> {
        self.version = version();
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

/// Upgrade pre-v4 records without discarding credentials. Older OAuth tables
/// were untagged, while old API-key tables already carried `type = "api_key"`.
fn migrate_provider_credentials(document: &mut toml::Value, previous_version: i64) {
    let Some(table) = document.as_table_mut() else {
        return;
    };
    // Version 4 credentials are already explicitly tagged. Reinterpreting
    // them would corrupt bearer, Copilot OAuth, and no-auth records.
    if previous_version >= i64::from(version()) {
        return;
    }
    table.insert("version".into(), toml::Value::Integer(i64::from(version())));
    let Some(providers) = table
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    for provider in providers {
        let Some(provider) = provider.as_table_mut() else {
            continue;
        };
        let credentials = provider
            .remove("credentials")
            .or_else(|| provider.remove("auth"));
        let Some(mut credentials) = credentials else {
            continue;
        };
        let credential_type = credentials.get("type").and_then(toml::Value::as_str);
        let is_legacy_chatgpt = credential_type.is_none();
        if is_legacy_chatgpt && let Some(auth) = credentials.as_table_mut() {
            auth.insert("type".into(), toml::Value::String("chatgpt".into()));
        }
        provider.insert("credentials".into(), credentials);
        provider.entry("provider").or_insert_with(|| {
            toml::Value::String(
                if is_legacy_chatgpt {
                    "chatgpt"
                } else {
                    "openai"
                }
                .into(),
            )
        });
    }
}

/// Version 2 rendered cumulative tokens as part of `context`. Preserve the
/// existing visible bar by enabling the new independent item once; version 3
/// then respects users who disable it.
fn migrate_session_tokens(document: &mut toml::Value, previous_version: i64) {
    if previous_version >= 3 {
        return;
    }
    let Some(items) = document
        .get_mut("status_bar")
        .and_then(|status| status.get_mut("items"))
        .and_then(toml::Value::as_array_mut)
    else {
        return;
    };
    let has_context = items.iter().any(|item| item.as_str() == Some("context"));
    let has_tokens = items
        .iter()
        .any(|item| item.as_str() == Some("session_tokens"));
    if has_context && !has_tokens {
        items.push(toml::Value::String("session_tokens".into()));
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path).join("providers.toml"));
    }
    let root = dirs::config_dir()
        .context("could not find config directory")?
        .join("artist");
    migrate_legacy_root(&root)?;
    Ok(root.join("providers.toml"))
}

/// Move the legacy `~/.artist` global state into the platform config directory.
/// Existing files in the destination win; legacy-only files are copied across.
fn migrate_legacy_root(root: &Path) -> Result<()> {
    let Some(home) = dirs::home_dir() else {
        return Ok(());
    };
    migrate_root_between(&home.join(".artist"), root)
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
            if !to.exists() {
                copy_tree(&from, &to)?;
            }
        } else if !to.exists() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::status_bar::StatusItem;
    use llm_provider::{Auth, Credentials, ProviderKind, SavedProvider, Secret};
    #[test]
    fn preserves_legacy_api_key_providers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(
            &path,
            r#"version = 1
default_provider = "api"
[[providers]]
id = "api"
name = "API"
base_url = "https://api.example/v1/"
[providers.auth]
type = "api_key"
api_key = "secret"
"#,
        )
        .unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert_eq!(store.providers.len(), 1);
        assert_eq!(store.providers[0].provider, ProviderKind::Openai);
        assert!(matches!(&store.providers[0].credentials,
            Credentials::ApiKey { api_key } if api_key.expose() == "secret"));
        assert_eq!(store.default_provider.unwrap().as_str(), "api");
    }

    #[test]
    fn preserves_all_v4_credential_variants() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(
            &path,
            r#"version = 4
[[providers]]
id = "bearer"
name = "Bearer"
provider = "azure"
base_url = "https://example.com/"
[providers.credentials]
type = "bearer_token"
token = "token"
[[providers]]
id = "copilot"
name = "Copilot"
provider = "copilot"
base_url = "https://example.com/"
[providers.credentials]
type = "copilot_oauth"
token_dir = "/tmp/tokens"
[[providers]]
id = "none"
name = "Local"
provider = "ollama"
base_url = "http://localhost:11434/"
[providers.credentials]
type = "none"
"#,
        )
        .unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert!(matches!(
            store.providers[0].credentials,
            Credentials::BearerToken { .. }
        ));
        assert!(matches!(
            store.providers[1].credentials,
            Credentials::CopilotOauth { .. }
        ));
        assert_eq!(store.providers[2].credentials, Credentials::None);
    }

    #[test]
    fn old_config_gets_default_status_bar() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(&path, "version = 2\nproviders = []\n").unwrap();
        let store = ProviderStore::load(&path).unwrap();
        assert_eq!(store.status_bar, StatusBarConfig::default());
    }

    #[test]
    fn migrates_combined_context_tokens_once_then_respects_disable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        fs::write(
            &path,
            "version = 2\nproviders = []\n[status_bar]\nitems = ['context']\n",
        )
        .unwrap();

        let mut store = ProviderStore::load(&path).unwrap();
        assert_eq!(
            store.status_bar.items,
            [StatusItem::Context, StatusItem::SessionTokens]
        );
        store
            .status_bar
            .items
            .retain(|item| *item != StatusItem::SessionTokens);
        store.save(&path).unwrap();

        let reloaded = ProviderStore::load(&path).unwrap();
        assert_eq!(reloaded.version, 4);
        assert_eq!(reloaded.status_bar.items, [StatusItem::Context]);
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
        assert_eq!(
            fs::read_to_string(root.join("rules/one.md")).unwrap(),
            "rule\n"
        );
    }

    #[test]
    fn merges_legacy_only_files_without_overwriting_destination() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("config/artist");
        let root = dir.path().join(".artist");
        // Destination already has providers.toml (must win) but no rules.
        write_file(&root.join("providers.toml"), "version = 2\nkept = true\n");
        write_file(
            &legacy.join("providers.toml"),
            "version = 1\nstale = true\n",
        );
        write_file(&legacy.join("rules/one.md"), "rule\n");

        migrate_root_between(&legacy, &root).unwrap();

        // Destination file preserved; legacy-only file copied in.
        assert_eq!(
            fs::read_to_string(root.join("providers.toml")).unwrap(),
            "version = 2\nkept = true\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("rules/one.md")).unwrap(),
            "rule\n"
        );
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
            Auth {
                access_token: Secret::new("access"),
                refresh_token: Secret::new("refresh"),
                account_id: "acct".into(),
                email: None,
                expires_at: None,
            },
        ));
        store.save(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
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
