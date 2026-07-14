//! Durable, human-readable chat sessions stored below a configuration root.
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub role: Role,
    pub content: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub id: String,
    pub created_at_ms: u64,
    pub label: Option<String>,
    pub project: PathBuf,
    pub transcript: PathBuf,
}
#[derive(Default, Serialize, Deserialize)]
struct Index {
    #[serde(default)]
    projects: Vec<Project>,
}
#[derive(Serialize, Deserialize)]
struct Project {
    path: PathBuf,
    sessions: Vec<Session>,
}

/// A filesystem-backed session repository rooted at `<config_root>/sessions`.
pub struct SessionStore {
    root: PathBuf,
}
impl SessionStore {
    pub fn new(config_root: impl AsRef<Path>) -> Self {
        Self {
            root: config_root.as_ref().join("sessions"),
        }
    }

    /// Creates a session for an existing project directory (stored canonically).
    pub fn create(&self, project: impl AsRef<Path>, label: Option<&str>) -> Result<Session> {
        let project = fs::canonicalize(project).context("canonicalize project directory")?;
        if !project.is_dir() {
            bail!("project is not a directory")
        }
        let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let now = duration.as_millis() as u64;
        let id = format!("{:x}-{:x}", duration.as_nanos(), std::process::id());
        let dir = self.root.join(project_key(&project));
        fs::create_dir_all(&dir)?;
        let name = format!(
            "{}-{}{}.md",
            now,
            id,
            label
                .map(|v| format!("-{}", sanitize(v)))
                .unwrap_or_default()
        );
        let transcript = dir.join(name);
        fs::write(
            &transcript,
            format!("# Artist session\n\n- ID: `{id}`\n- Created: `{now}`\n\n"),
        )?;
        let session = Session {
            id,
            created_at_ms: now,
            label: label.map(str::to_owned),
            project: project.clone(),
            transcript,
        };
        let mut index = self.read_index()?;
        match index.projects.iter_mut().find(|p| p.path == project) {
            Some(p) => p.sessions.push(session.clone()),
            None => index.projects.push(Project {
                path: project,
                sessions: vec![session.clone()],
            }),
        }
        self.write_index(&index)?;
        Ok(session)
    }

    pub fn load(&self, id: &str) -> Result<(Session, Vec<Turn>)> {
        let session = self
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .context("session not found")?;
        let text = fs::read_to_string(&session.transcript).context("read transcript")?;
        let turns = text
            .lines()
            .filter_map(|line| {
                line.strip_prefix("<!-- artist-turn:")
                    .and_then(|v| v.strip_suffix(" -->"))
            })
            .map(serde_json::from_str)
            .collect::<std::result::Result<_, _>>()
            .context("parse transcript")?;
        Ok((session, turns))
    }

    /// Durably appends a turn. The JSON marker makes arbitrary Markdown content reversible.
    pub fn append(&self, id: &str, turn: &Turn) -> Result<()> {
        use std::io::Write;
        let (session, _) = self.load(id)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(session.transcript)?;
        writeln!(
            file,
            "<!-- artist-turn:{} -->\n\n## {}\n\n{}\n",
            serde_json::to_string(turn)?,
            match turn.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
            },
            turn.content
        )?;
        file.sync_all()?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Session>> {
        Ok(self
            .read_index()?
            .projects
            .into_iter()
            .flat_map(|p| p.sessions)
            .collect())
    }
    pub fn list_project(&self, project: impl AsRef<Path>) -> Result<Vec<Session>> {
        let path = fs::canonicalize(project)?;
        Ok(self
            .read_index()?
            .projects
            .into_iter()
            .find(|p| p.path == path)
            .map(|p| p.sessions)
            .unwrap_or_default())
    }
    fn read_index(&self) -> Result<Index> {
        let path = self.root.join("index.toml");
        if !path.exists() {
            return Ok(Index::default());
        }
        toml::from_str(&fs::read_to_string(path)?).context("parse session index")
    }
    fn write_index(&self, index: &Index) -> Result<()> {
        fs::create_dir_all(&self.root)?;
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let tmp = self
            .root
            .join(format!(".index-{}-{nonce}.tmp", std::process::id()));
        let mut file = fs::File::create(&tmp)?;
        use std::io::Write;
        file.write_all(toml::to_string_pretty(index)?.as_bytes())?;
        file.sync_all()?;
        fs::rename(tmp, self.root.join("index.toml")).context("atomically replace session index")
    }
}
fn sanitize(value: &str) -> String {
    let out: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect();
    if out.trim_matches('-').is_empty() {
        "session".into()
    } else {
        out
    }
}
fn project_key(path: &Path) -> String {
    let mut h = DefaultHasher::new();
    path.hash(&mut h);
    let base = path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("project");
    format!("{}-{:x}", sanitize(base), h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_and_grouping() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("weird project");
        fs::create_dir(&project)?;
        let store = SessionStore::new(temp.path().join("config"));
        let session = store.create(&project, Some("../../ hello 🌍"))?;
        let turns = [
            Turn {
                role: Role::User,
                content: "hello\n## Assistant".into(),
            },
            Turn {
                role: Role::Assistant,
                content: "world".into(),
            },
        ];
        for turn in &turns {
            store.append(&session.id, turn)?;
        }
        let (loaded, actual) = store.load(&session.id)?;
        assert_eq!(actual, turns);
        assert_eq!(loaded.project, fs::canonicalize(&project)?);
        assert_eq!(store.list_project(&project)?.len(), 1);
        assert!(
            session
                .transcript
                .starts_with(temp.path().join("config/sessions"))
        );
        assert!(temp.path().join("config/sessions/index.toml").exists());
        Ok(())
    }
}
