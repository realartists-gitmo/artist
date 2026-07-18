use crate::Manifest;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DiscoveredExtension {
    pub manifest: Manifest,
    pub wasm: PathBuf,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub path: PathBuf,
    pub message: String,
}

pub fn default_root() -> Result<PathBuf> {
    Ok(dirs::home_dir()
        .context("could not find home directory")?
        .join(".artist/extensions"))
}

pub fn discover(root: &Path) -> (Vec<DiscoveredExtension>, Vec<Diagnostic>) {
    let mut found = Vec::new();
    let mut diagnostics = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return (found, diagnostics);
    };
    for entry in entries.flatten() {
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let Some(id) = directory.file_name().and_then(|v| v.to_str()) else {
            continue;
        };
        let manifest_path = directory.join("extension.toml");
        let wasm = directory.join("extension.wasm");
        match Manifest::load(&manifest_path, id) {
            Ok(_manifest) if !wasm.is_file() => diagnostics.push(Diagnostic {
                path: wasm,
                message: "extension.wasm is missing".into(),
            }),
            Ok(manifest) if manifest.enabled => found.push(DiscoveredExtension { manifest, wasm }),
            Ok(_) => {}
            Err(error) => diagnostics.push(Diagnostic {
                path: manifest_path,
                message: format!("{error:#}"),
            }),
        }
    }
    found.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    (found, diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovers_enabled_valid_layouts() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("demo");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("extension.toml"), "id='demo'\n").unwrap();
        std::fs::write(dir.join("extension.wasm"), b"wasm").unwrap();
        assert_eq!(discover(root.path()).0[0].manifest.id, "demo");
    }
}
