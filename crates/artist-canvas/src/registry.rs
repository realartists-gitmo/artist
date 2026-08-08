//! Finding canvases on disk.
//!
//! Canvases live at `<project>/.artist/canvas/<slug>/`, and are meant to be
//! committable: a canvas the agent grew is worth sharing with a team.
//!
//! Only artist's own repository ignores `.artist/`, so nothing here may assume
//! a user's project does. Scaffolding writes a `.gitignore` covering the files
//! the harness rewrites on every interaction, leaving the canvas itself
//! trackable and the user's `git status` clean.

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

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
        let config_root = std::env::var_os("ARTIST_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|path| path.join("artist")));
        Self::discover_from(project, config_root.as_deref())
    }

    /// Merge global config canvases below project-local canvases. A later scope owns a
    /// slug even when its manifest is malformed, so a broken local canvas cannot reveal
    /// a same-named global canvas behind it.
    pub fn discover_from(project: &Path, config_root: Option<&Path>) -> Self {
        let mut registry = Registry::default();
        let mut canvases = BTreeMap::<String, Canvas>::new();
        if let Some(config_root) = config_root {
            scan_root(
                &config_root.join("canvas"),
                &mut canvases,
                &mut registry.diagnostics,
            );
        }
        scan_root(
            &project.join(CANVAS_DIR),
            &mut canvases,
            &mut registry.diagnostics,
        );
        registry.canvases = canvases.into_values().collect();
        registry
    }

    pub fn get(&self, slug: &str) -> Option<&Canvas> {
        self.canvases.iter().find(|canvas| canvas.slug == slug)
    }
}

fn scan_root(
    root: &Path,
    canvases: &mut BTreeMap<String, Canvas>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
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
            continue;
        };
        // A later scope owns the name whether valid or not.
        canvases.remove(&slug);
        match Manifest::parse(&source) {
            Ok(manifest) => {
                canvases.insert(
                    slug.clone(),
                    Canvas {
                        slug,
                        root: directory,
                        manifest,
                    },
                );
            }
            Err(error) => diagnostics.push(Diagnostic {
                slug,
                message: format!("{MANIFEST_FILE} is invalid: {error}"),
            }),
        }
    }
}

/// Scaffold a new canvas from a template.
///
/// Refuses to overwrite: a canvas is durable, and `create` on an existing slug
/// is far more likely to be the model forgetting it already made one than a
/// deliberate reset.
pub fn scaffold(
    project: &Path,
    slug: &str,
    title: &str,
    template: &crate::templates::Template,
) -> Result<Canvas, ScaffoldError> {
    let root = project.join(CANVAS_DIR).join(slug);
    if root.join(MANIFEST_FILE).exists() {
        return Err(ScaffoldError::Exists {
            slug: slug.to_owned(),
        });
    }
    std::fs::create_dir_all(&root)?;
    ignore_generated_files(&project.join(CANVAS_DIR))?;

    let manifest = crate::templates::manifest_for(title, template);
    std::fs::write(root.join(MANIFEST_FILE), manifest.render())?;
    for (relative, contents) in template.files {
        let target = root.join(relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, contents)?;
    }

    Ok(Canvas {
        slug: slug.to_owned(),
        root,
        manifest,
    })
}

/// Keep the harness's own output out of the user's diffs.
///
/// A canvas is meant to be committable — that is the point of it being durable
/// and project-local — but `state.json` is rewritten on every click, so leaving
/// it tracked would put the user's UI interactions in `git status`. Written
/// once, next to the canvases, and never overwritten if it already exists.
fn ignore_generated_files(canvas_root: &Path) -> std::io::Result<()> {
    let ignore = canvas_root.join(".gitignore");
    if ignore.exists() {
        return Ok(());
    }
    std::fs::write(
        ignore,
        "# Written by artist on every canvas interaction.\nstate.json\n*.json.tmp\n",
    )
}

#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("a canvas named `{slug}` already exists — edit it, or pick another name")]
    Exists { slug: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Reduce a title to a directory-safe slug.
///
/// The result becomes a path segment and a URL segment, so it is restricted to
/// characters that need no escaping in either.
/// Is this already a slug, rather than something that would become one?
///
/// For input that arrives from somewhere other than the model — a ticket
/// pasted from a chat message — where the right move is to refuse rather than
/// to coerce. `slugify` would happily turn `../../etc` into `etc` and hand back
/// something that looks fine, which is exactly the wrong shape of helpful when
/// the value is about to become a path segment.
pub fn is_slug(value: &str) -> bool {
    !value.is_empty() && value.len() <= 64 && value == slugify(value)
}

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

    /// A temp directory that removes itself when the test ends. Derefs to
    /// `Path`, so the owning bind is the only thing callers must keep.
    ///
    /// Previously a pid-and-thread-named path cleaned at the *start* of the
    /// next run, so every run left its directories behind — invisible on a
    /// tmpfs that empties at reboot, permanent on disk.
    struct Temp(tempfile::TempDir);

    impl std::ops::Deref for Temp {
        type Target = Path;
        fn deref(&self) -> &Path {
            self.0.path()
        }
    }

    fn temp() -> Temp {
        Temp(
            tempfile::Builder::new()
                .prefix("artist-canvas-registry-")
                .tempdir()
                .expect("temp dir"),
        )
    }

    #[test]
    fn a_project_without_canvases_is_empty_not_an_error() {
        let project = temp();
        let registry = Registry::discover(&project);
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
    fn global_canvases_are_visible_and_project_local_canvases_shadow_them() {
        let project = temp();
        let config = temp();
        let global = config.join("canvas").join("shared");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join(MANIFEST_FILE), "title = \"Global\"").unwrap();

        let from_global = Registry::discover_from(&project, Some(&config));
        let shared = from_global.get("shared").expect("global canvas visible");
        assert_eq!(shared.manifest.title, "Global");
        assert_eq!(shared.root, global);

        write(&project, "shared", "title = \"Local\"");
        let merged = Registry::discover_from(&project, Some(&config));
        let shared = merged.get("shared").expect("local canvas visible");
        assert_eq!(shared.manifest.title, "Local");
        assert!(shared.root.starts_with(project.join(CANVAS_DIR)));
    }

    #[test]
    fn invalid_project_local_manifest_still_shadows_same_named_global_canvas() {
        let project = temp();
        let config = temp();
        let global = config.join("canvas").join("shared");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::write(global.join(MANIFEST_FILE), "title = \"Global\"").unwrap();
        write(&project, "shared", "entry = [1, 2]");

        let merged = Registry::discover_from(&project, Some(&config));
        assert!(merged.get("shared").is_none());
        assert!(
            merged
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.slug == "shared")
        );
    }

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
    fn scaffolding_produces_a_canvas_that_discovery_finds() {
        let project = temp();
        let template = crate::templates::find("dashboard").expect("template");

        let created = scaffold(&project, "perf", "Perf", template).expect("scaffold");
        assert_eq!(created.manifest.title, "Perf");
        assert!(created.entry_path().exists(), "entry was not written");

        let found = Registry::discover(&project);
        assert_eq!(found.canvases.len(), 1);
        assert_eq!(found.get("perf").expect("found").manifest.title, "Perf");
    }

    /// A canvas is durable. Recreating one is almost always the model having
    /// forgotten it already exists, not a deliberate reset.
    /// `.artist/` is gitignored in artist's own repo, not in a user's project,
    /// so scaffolding has to keep its own churn out of their diffs.
    #[test]
    fn scaffolding_keeps_generated_state_out_of_git() {
        let project = temp();
        let template = crate::templates::find("blank").expect("template");
        scaffold(&project, "perf", "Perf", template).expect("scaffold");

        let ignore = project.join(CANVAS_DIR).join(".gitignore");
        let contents = std::fs::read_to_string(&ignore).expect("gitignore written");
        assert!(contents.contains("state.json"), "{contents}");

        // A user's own edits to it survive a second scaffold.
        std::fs::write(&ignore, "mine\n").expect("edit");
        scaffold(&project, "other", "Other", template).expect("second");
        assert_eq!(std::fs::read_to_string(&ignore).expect("read"), "mine\n");
    }

    #[test]
    fn scaffolding_refuses_to_overwrite_an_existing_canvas() {
        let project = temp();
        let template = crate::templates::find("blank").expect("template");
        scaffold(&project, "perf", "First", template).expect("first");

        let error = scaffold(&project, "perf", "Second", template).expect_err("refused");
        assert!(matches!(error, ScaffoldError::Exists { .. }));
        // The original survives untouched.
        assert_eq!(
            Registry::discover(&project)
                .get("perf")
                .expect("still there")
                .manifest
                .title,
            "First"
        );
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
