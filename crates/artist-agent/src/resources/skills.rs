use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

const SKILL_CAP: u64 = 256 * 1024;
const MAX_DEPTH: usize = 6;
const MAX_ENTRIES: usize = 2_000;
const MAX_SKILLS: usize = 200;

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub file: PathBuf,
    pub base: PathBuf,
}

#[derive(Deserialize)]
struct Frontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(flatten)]
    _other: BTreeMap<String, serde_yaml::Value>,
}
pub fn discover(workspace: &Path, diagnostics: &mut Vec<String>) -> BTreeMap<String, Skill> {
    let mut roots = Vec::new();
    if let Some(config_root) = std::env::var_os("ARTIST_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::config_dir().map(|path| path.join("artist")))
    {
        roots.push(config_root.join("skills"));
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".agents/skills"));
    }

    let start = workspace
        .ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(workspace);
    let mut directories = workspace
        .ancestors()
        .take_while(|path| path.starts_with(start))
        .collect::<Vec<_>>();
    directories.reverse();
    for directory in directories {
        roots.push(directory.join(".artist/skills"));
        roots.push(directory.join(".agents/skills"));
    }
    discover_roots(roots, diagnostics)
}

pub(crate) fn discover_roots(
    roots: Vec<PathBuf>,
    diagnostics: &mut Vec<String>,
) -> BTreeMap<String, Skill> {
    let mut skills = BTreeMap::new();
    for root in roots {
        for file in skill_files(&root, diagnostics) {
            if let Some(skill) = parse(&file, diagnostics) {
                if skills.len() >= MAX_SKILLS && !skills.contains_key(&skill.name) {
                    diagnostics.push(format!("skill catalog capped; skipped {}", file.display()));
                    continue;
                }
                if skills.insert(skill.name.clone(), skill).is_some() {
                    diagnostics.push(format!(
                        "skill collision resolved by later scope: {}",
                        file.display()
                    ));
                }
            }
        }
    }
    skills
}

fn skill_files(root: &Path, diagnostics: &mut Vec<String>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![(root.to_owned(), 0usize)];
    let mut visited = 0;
    while let Some((directory, depth)) = pending.pop() {
        if depth > MAX_DEPTH || visited >= MAX_ENTRIES {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries.flatten().collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            visited += 1;
            if visited > MAX_ENTRIES {
                diagnostics.push(format!("skill scan capped below {}", root.display()));
                break;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_file() && entry.file_name() == "SKILL.md" {
                found.push(path);
            } else if kind.is_dir() && depth < MAX_DEPTH {
                pending.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

fn parse(file: &Path, diagnostics: &mut Vec<String>) -> Option<Skill> {
    let result = (|| -> anyhow::Result<Skill> {
        let metadata = std::fs::metadata(file)?;
        anyhow::ensure!(metadata.len() <= SKILL_CAP, "exceeds {SKILL_CAP} bytes");
        let text = std::fs::read_to_string(file)?;
        let (yaml, _) = frontmatter(&text)?;
        let parsed: Frontmatter = serde_yaml::from_str(yaml)?;
        let name = parsed
            .name
            .filter(|name| !name.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing name"))?;
        let description = parsed
            .description
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("missing description"))?;
        let base = file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("missing parent"))?
            .canonicalize()?;
        validate_cosmetic(&name, &description, &base, diagnostics);
        Ok(Skill {
            name,
            description,
            file: file.canonicalize()?,
            base,
        })
    })();
    match result {
        Ok(skill) => Some(skill),
        Err(error) => {
            diagnostics.push(format!("{}: {error}", file.display()));
            None
        }
    }
}

pub fn frontmatter(text: &str) -> anyhow::Result<(&str, &str)> {
    let (rest, delimiter) = if let Some(rest) = text.strip_prefix("---\n") {
        (rest, "\n---")
    } else if let Some(rest) = text.strip_prefix("---\r\n") {
        (rest, "\r\n---")
    } else {
        anyhow::bail!("missing YAML frontmatter")
    };
    let split = rest
        .find(delimiter)
        .ok_or_else(|| anyhow::anyhow!("unterminated YAML frontmatter"))?;
    let tail = &rest[split + delimiter.len()..];
    let body = tail
        .strip_prefix("\r\n")
        .or_else(|| tail.strip_prefix('\n'))
        .unwrap_or(tail);
    Ok((&rest[..split], body))
}

fn validate_cosmetic(name: &str, description: &str, base: &Path, diagnostics: &mut Vec<String>) {
    let valid = name.len() <= 64
        && !name.starts_with('-')
        && !name.ends_with('-')
        && !name.contains("--")
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid {
        diagnostics.push(format!("skill `{name}` has a non-standard name"));
    }
    if description.len() > 1024 {
        diagnostics.push(format!("skill `{name}` description exceeds 1024 bytes"));
    }
    if base.file_name().and_then(|v| v.to_str()) != Some(name) {
        diagnostics.push(format!("skill `{name}` does not match parent directory"));
    }
}
