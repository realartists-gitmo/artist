use std::{
    fs, io,
    path::{Path, PathBuf},
};

use thiserror::Error;

pub const BUILTIN_SYSTEM: &str = include_str!("../prompts/SYSTEM.md");
pub const BUILTIN_AGENTS: &str = include_str!("../prompts/AGENTS.md");

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("the operating system did not provide a global config directory")]
    NoGlobalConfigDirectory,
    #[error("failed to read or create config: {0}")]
    Io(#[from] io::Error),
}

/// Artist uses the same `.artist` shape globally and in each workspace.
/// A workspace file replaces the same global file; a global file replaces the
/// built-in default compiled from this repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigPaths {
    pub global: PathBuf,
    pub workspace: PathBuf,
}

impl ConfigPaths {
    pub fn discover(workspace_root: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let config = dirs::config_dir().ok_or(ConfigError::NoGlobalConfigDirectory)?;
        Ok(Self {
            global: config.join(".artist"),
            workspace: workspace_root.as_ref().join(".artist"),
        })
    }

    pub fn initialize(&self) -> Result<(), ConfigError> {
        fs::create_dir_all(&self.global)?;
        fs::create_dir_all(&self.workspace)?;
        write_if_missing(&self.global.join("SYSTEM.md"), BUILTIN_SYSTEM)?;
        write_if_missing(&self.workspace.join("AGENTS.md"), BUILTIN_AGENTS)?;
        Ok(())
    }

    pub fn system_instructions(&self) -> Result<String, ConfigError> {
        self.resolve("SYSTEM.md", BUILTIN_SYSTEM)
    }

    pub fn agents_instructions(&self) -> Result<String, ConfigError> {
        self.resolve("AGENTS.md", BUILTIN_AGENTS)
    }

    /// Local `.artist` oversumes global `.artist` by replacing matching files.
    fn resolve(&self, name: &str, builtin: &str) -> Result<String, ConfigError> {
        let local = self.workspace.join(name);
        if local.is_file() {
            return Ok(fs::read_to_string(local)?);
        }
        let global = self.global.join(name);
        if global.is_file() {
            return Ok(fs::read_to_string(global)?);
        }
        Ok(builtin.to_owned())
    }
}

fn write_if_missing(path: &Path, content: &str) -> io::Result<()> {
    if !path.exists() {
        fs::write(path, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_replaces_global_and_global_replaces_builtin() {
        let root = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            global: root.path().join("global/.artist"),
            workspace: root.path().join("workspace/.artist"),
        };
        fs::create_dir_all(&paths.global).unwrap();
        fs::create_dir_all(&paths.workspace).unwrap();

        assert_eq!(paths.system_instructions().unwrap(), BUILTIN_SYSTEM);
        fs::write(paths.global.join("SYSTEM.md"), "global").unwrap();
        assert_eq!(paths.system_instructions().unwrap(), "global");
        fs::write(paths.workspace.join("SYSTEM.md"), "local").unwrap();
        assert_eq!(paths.system_instructions().unwrap(), "local");
    }

    #[test]
    fn initialization_does_not_overwrite_user_files() {
        let root = tempfile::tempdir().unwrap();
        let paths = ConfigPaths {
            global: root.path().join("global/.artist"),
            workspace: root.path().join("workspace/.artist"),
        };
        fs::create_dir_all(&paths.global).unwrap();
        fs::write(paths.global.join("SYSTEM.md"), "mine").unwrap();
        paths.initialize().unwrap();
        assert_eq!(
            fs::read_to_string(paths.global.join("SYSTEM.md")).unwrap(),
            "mine"
        );
        assert_eq!(
            fs::read_to_string(paths.workspace.join("AGENTS.md")).unwrap(),
            BUILTIN_AGENTS
        );
    }
}
