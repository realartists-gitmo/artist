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
}
fn version() -> u8 {
    1
}

impl ProviderStore {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self {
                version: 1,
                ..Self::default()
            });
        }
        let store: Self = toml::from_str(&fs::read_to_string(path).context("read providers.toml")?)
            .context("parse providers.toml")?;
        store.validate()?;
        Ok(store)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
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

pub fn config_path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("ARTIST_CONFIG_DIR") {
        return Ok(PathBuf::from(path).join("providers.toml"));
    }
    Ok(dirs::config_dir()
        .context("could not find user config directory")?
        .join(".artist/providers.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use llm_provider::{SavedProvider, Secret};
    use url::Url;
    #[test]
    fn saves_and_loads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("providers.toml");
        let mut store = ProviderStore {
            version: 1,
            ..Default::default()
        };
        store.add(
            SavedProvider::openai_compatible(
                ProviderId::new("one").unwrap(),
                "One",
                Url::parse("https://example.com/v1/").unwrap(),
                Secret::new("key"),
            )
            .unwrap(),
        );
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
