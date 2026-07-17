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
        migrate_to_chatgpt_only(&mut document);
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

fn migrate_to_chatgpt_only(document: &mut toml::Value) {
    let Some(table) = document.as_table_mut() else {
        return;
    };
    table.insert("version".into(), toml::Value::Integer(2));
    if let Some(providers) = table
        .get_mut("providers")
        .and_then(toml::Value::as_array_mut)
    {
        providers.retain(|provider| {
            provider
                .get("auth")
                .and_then(|auth| auth.get("type"))
                .and_then(toml::Value::as_str)
                != Some("api_key")
        });
        let ids: Vec<String> = providers
            .iter()
            .filter_map(|provider| {
                provider
                    .get("id")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            })
            .collect();
        let invalid_default = table
            .get("default_provider")
            .and_then(toml::Value::as_str)
            .is_some_and(|id| !ids.iter().any(|candidate| candidate == id));
        if invalid_default {
            if let Some(first) = ids.first() {
                table.insert(
                    "default_provider".into(),
                    toml::Value::String(first.clone()),
                );
            } else {
                table.remove("default_provider");
            }
        }
    }
}

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path).join("providers.toml"));
    }
    Ok(dirs::config_dir()
        .context("could not find user config directory")?
        .join("artist/providers.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{Auth, SavedProvider, Secret};
    #[test]
    fn removes_legacy_api_key_providers() {
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
        assert!(store.providers.is_empty());
        assert!(store.default_provider.is_none());
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
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore {
            version: 1,
            ..Default::default()
        };
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
        let loaded = ProviderStore::load(&path).unwrap();
        assert_eq!(loaded.providers.len(), 1);
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
