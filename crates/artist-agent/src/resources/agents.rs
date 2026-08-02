use std::path::{Path, PathBuf};

const AGENTS_CAP: u64 = 128 * 1024;
const MAX_NESTED: usize = 100;
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug)]
pub struct AgentsFile {
    pub path: PathBuf,
    pub content: String,
    pub global: bool,
}

pub fn discover(workspace: &Path, diagnostics: &mut Vec<String>) -> Vec<AgentsFile> {
    let config_root = std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("artist")));
    discover_from(workspace, config_root.as_deref(), diagnostics)
}

pub(crate) fn discover_from(
    workspace: &Path,
    config_root: Option<&Path>,
    diagnostics: &mut Vec<String>,
) -> Vec<AgentsFile> {
    let mut files = Vec::new();
    if let Some(config_root) = config_root {
        load(
            &config_root.join("AGENTS.md"),
            true,
            &mut files,
            diagnostics,
        );
    }
    let start = git_root(workspace).unwrap_or_else(|| workspace.to_owned());
    for directory in ancestry(&start, workspace) {
        load(&directory.join("AGENTS.md"), false, &mut files, diagnostics);
    }
    files
}

/// Every nested `AGENTS.md` beneath the workspace, ignore-rules respected.
///
/// This used to be a hand-rolled walk that excluded `.git` and `node_modules`
/// and nothing else, under a global 5,000-entry budget. On any Rust project
/// that budget was consumed inside `target/` — 17,000 entries at depth three in
/// this repository alone — and because the walk was a stack popped in
/// `read_dir` order, *which* directories were reached before the budget ran out
/// depended on filesystem ordering. A nested `AGENTS.md` could be silently
/// dropped, and the set could change between turns with no file having changed,
/// which then moved the system prompt underneath the prompt cache.
///
/// Deferring to `ignore` fixes the cause rather than the symptom: build outputs
/// are gitignored, so they are skipped for the same reason the user's editor
/// skips them, and a sorted walk makes the result depend on the tree rather
/// than on the order the filesystem happened to return.
pub fn nested(workspace: &Path, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let walker = ignore::WalkBuilder::new(workspace)
        .max_depth(Some(MAX_DEPTH))
        .follow_links(false)
        // Dotfile directories are not hidden from this walk — `.artist/` and
        // friends may legitimately carry instructions — but `.git` is never
        // interesting and is enormous.
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        // Apply gitignore rules even outside a git checkout, so a tarball of a
        // project behaves the same as a clone of it.
        .require_git(false)
        .sort_by_file_name(std::ffi::OsStr::cmp)
        .build();

    let root_agents = workspace.join("AGENTS.md");
    for entry in walker.flatten() {
        if found.len() >= MAX_NESTED {
            diagnostics.push(format!("nested AGENTS.md scan capped at {MAX_NESTED} files"));
            break;
        }
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if path.file_name() == Some(std::ffi::OsStr::new("AGENTS.md")) && path != root_agents {
            found.push(path);
        }
    }
    found.sort();
    found
}
fn load(path: &Path, global: bool, files: &mut Vec<AgentsFile>, diagnostics: &mut Vec<String>) {
    if !path.exists() {
        return;
    }
    let result = (|| -> anyhow::Result<String> {
        let metadata = std::fs::metadata(path)?;
        anyhow::ensure!(metadata.is_file(), "not a regular file");
        anyhow::ensure!(metadata.len() <= AGENTS_CAP, "exceeds {AGENTS_CAP} bytes");
        Ok(std::fs::read_to_string(path)?)
    })();
    match result {
        Ok(content) => files.push(AgentsFile {
            path: path.to_owned(),
            content,
            global,
        }),
        Err(error) => diagnostics.push(format!("{}: {error}", path.display())),
    }
}

fn git_root(workspace: &Path) -> Option<PathBuf> {
    workspace
        .ancestors()
        .find(|path| path.join(".git").exists())
        .map(Path::to_owned)
}

fn ancestry(start: &Path, end: &Path) -> Vec<PathBuf> {
    let mut result = end
        .ancestors()
        .take_while(|path| path.starts_with(start))
        .map(Path::to_owned)
        .collect::<Vec<_>>();
    result.reverse();
    result
}
