//! Finding canvases on disk.
//!
//! Canvases live at `<project>/.artist/canvas/<slug>/`, which is already
//! gitignored — so they persist across sessions without being committed by
//! accident, and a team that wants to share one can un-ignore it deliberately.

use std::path::{Path, PathBuf};

use crate::manifest::Manifest;

pub const CANVAS_DIR: &str = ".artist/canvas";
pub const MANIFEST_FILE: &str = "canvas.toml";

/// One canvas as found on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Canvas {
    pub slug: String,
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Canvas {
    pub fn entry_path(&self) -> PathBuf {
        self.root.join(self.manifest.entry.trim_start_matches("./"))
    }
}

/// A problem with one canvas that must not take down the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub slug: String,
    pub message: String,
}

#[derive(Clone, Debug, Default)]
pub struct Registry {
    pub canvases: Vec<Canvas>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Registry {
    /// Scan a project. A malformed manifest becomes a diagnostic rather than an
    /// error, mirroring extension discovery: one broken canvas should not stop
    /// the others from opening.
    pub fn discover(project: &Path) -> Self {
        let root = project.join(CANVAS_DIR);
        let mut registry = Registry::default();

        let Ok(entries) = std::fs::read_dir(&root) else {
            return registry;
        };
        let mut found: Vec<_> = entries.flatten().collect();
        found.sort_by_key(std::fs::DirEntry::file_name);

        for entry in found {
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let slug = entry.file_name().to_string_lossy().into_owned();
            let directory = entry.path();
            let manifest_path = directory.join(MANIFEST_FILE);
            let Ok(source) = std::fs::read_to_string(&manifest_path) else {
                // A directory without a manifest is not a canvas; say nothing.
                continue;
            };
            match Manifest::parse(&source) {
                Ok(manifest) => registry.canvases.push(Canvas {
                    slug,
                    root: directory,
                    manifest,
                }),
                Err(error) => registry.diagnostics.push(Diagnostic {
                    slug,
                    message: format!("{MANIFEST_FILE} is invalid: {error}"),
                }),
            }
        }
        registry
    }

    pub fn get(&self, slug: &str) -> Option<&Canvas> {
        self.canvases.iter().find(|canvas| canvas.slug == slug)
    }
}

/// Reduce a title to a directory-safe slug.
///
/// The result becomes a path segment and a URL segment, so it is restricted to
/// characters that need no escaping in either.
pub fn slugify(value: &str) -> String {
    let mut slug = String::new();
    let mut hyphen_pending = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() {
            if hyphen_pending && !slug.is_empty() {
                slug.push('-');
            }
            hyphen_pending = false;
            slug.extend(character.to_lowercase());
        } else {
            hyphen_pending = true;
        }
    }
    if slug.is_empty() {
        "canvas".to_owned()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, slug: &str, manifest: &str) {
        let directory = root.join(CANVAS_DIR).join(slug);
        std::fs::create_dir_all(&directory).expect("create canvas dir");
        std::fs::write(directory.join(MANIFEST_FILE), manifest).expect("write manifest");
    }

    fn temp() -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "artist-canvas-registry-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).expect("temp dir");
        base
    }

    #[test]
    fn a_project_without_canvases_is_empty_not_an_error() {
        let registry = Registry::discover(&temp());
        assert!(registry.canvases.is_empty());
        assert!(registry.diagnostics.is_empty());
    }

    #[test]
    fn canvases_are_discovered_in_a_stable_order() {
        let project = temp();
        write(&project, "zebra", "title = \"Z\"");
        write(&project, "alpha", "title = \"A\"");

        let registry = Registry::discover(&project);
        let slugs: Vec<_> = registry.canvases.iter().map(|c| c.slug.as_str()).collect();
        assert_eq!(slugs, ["alpha", "zebra"]);
    }

    /// One canvas with a typo must not hide the ones that are fine.
    #[test]
    fn a_broken_manifest_is_reported_without_losing_its_neighbours() {
        let project = temp();
        write(&project, "good", "title = \"Good\"");
        write(&project, "bad", "entry = [1, 2]");

        let registry = Registry::discover(&project);
        assert_eq!(registry.canvases.len(), 1);
        assert_eq!(registry.canvases[0].slug, "good");
        assert_eq!(registry.diagnostics.len(), 1);
        assert_eq!(registry.diagnostics[0].slug, "bad");
    }

    #[test]
    fn slugs_are_safe_as_both_a_path_and_a_url_segment() {
        assert_eq!(slugify("Test Dashboard"), "test-dashboard");
        assert_eq!(slugify("perf/../etc"), "perf-etc");
        assert_eq!(slugify("  spaced  out  "), "spaced-out");
        assert_eq!(slugify("émoji 🎨 name"), "moji-name");
        assert_eq!(slugify("!!!"), "canvas");
    }
}
