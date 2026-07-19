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

pub fn nested(workspace: &Path, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(workspace.to_owned(), 0usize)];
    let mut visited = 0usize;
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_DEPTH || found.len() >= MAX_NESTED || visited >= 5_000 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            visited += 1;
            if visited >= 5_000 {
                diagnostics.push("nested AGENTS.md scan capped at 5000 entries".into());
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_file()
                && entry.file_name() == "AGENTS.md"
                && path != workspace.join("AGENTS.md")
            {
                found.push(path);
            } else if kind.is_dir()
                && depth < MAX_DEPTH
                && entry.file_name() != ".git"
                && entry.file_name() != "node_modules"
            {
                pending.push((path, depth + 1));
            }
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
