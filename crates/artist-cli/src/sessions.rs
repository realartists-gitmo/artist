//! Session repository: the canonical record of a session is its
//! `events.jsonl` log (see `artist-session`); the markdown transcript is a
//! derived projection. Legacy markdown-only sessions are converted on first
//! open.

use anyhow::{Context, Result, bail};
use artist_session::{
    AttachmentStore, Envelope, EventLogReader, EventLogWriter, LegacyTurn, Recorder,
    SessionCreated, SessionEvent, WriterTask, spawn_writer,
};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

/// Legacy transcript roles, kept for migration of pre-event-log sessions.
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
    /// The markdown transcript (projection). For event-log sessions this
    /// lives inside the session directory; for unmigrated legacy sessions
    /// it is the flat per-project file.
    pub transcript: PathBuf,
    /// Session this one was forked from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

impl Session {
    /// The session directory (valid for event-log sessions and after
    /// migration).
    pub fn dir(&self) -> &Path {
        self.transcript.parent().unwrap_or(Path::new("."))
    }

    fn has_event_log(&self) -> bool {
        self.dir().join(artist_session::EVENTS_FILE).exists()
    }
}

/// An open session: the exclusive writer plus the handles a run needs.
/// Dropping the recorder and awaiting [`ActiveSession::close`] flushes the
/// log.
pub struct ActiveSession {
    pub session: Session,
    pub recorder: Recorder,
    pub attachments: AttachmentStore,
    task: WriterTask,
}

impl ActiveSession {
    /// Flush and release the session. Call after all recorder clones are
    /// dropped (i.e. after the last run finished).
    pub async fn close(self) -> Result<()> {
        drop(self.recorder);
        self.task.close().await
    }

    /// Current events on disk (flush the recorder first for read-your-writes).
    pub fn events(&self) -> Result<Vec<Envelope>> {
        EventLogReader::new(self.session.dir()).read_all()
    }
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

    /// Creates and opens a session for an existing project directory.
    pub fn create(&self, project: impl AsRef<Path>, label: Option<&str>) -> Result<ActiveSession> {
        let project = fs::canonicalize(project).context("canonicalize project directory")?;
        if !project.is_dir() {
            bail!("project is not a directory")
        }
        fs::create_dir_all(&self.root)?;
        let _lock = self.lock_index()?;
        let label = label.map(|value| value.chars().take(80).collect::<String>());
        let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let now = duration.as_millis() as u64;
        let id = format!("{:x}-{:x}", duration.as_nanos(), std::process::id());
        let dir = self.root.join(project_key(&project)).join(&id);
        fs::create_dir_all(&dir)?;
        let session = Session {
            id: id.clone(),
            created_at_ms: now,
            label: label.clone(),
            project: project.clone(),
            transcript: dir.join("transcript.md"),
            parent: None,
        };
        let mut index = self.read_index()?;
        match index.projects.iter_mut().find(|p| p.path == project) {
            Some(p) => p.sessions.push(session.clone()),
            None => index.projects.push(Project {
                path: project.clone(),
                sessions: vec![session.clone()],
            }),
        }
        self.write_index(&index)?;
        let active = open_session_dir(session)?;
        active.recorder.record(SessionCreated {
            project: project.display().to_string(),
            label,
            artist_version: env!("CARGO_PKG_VERSION").to_owned(),
            parent_session: None,
        });
        Ok(active)
    }

    /// Opens a session for writing, migrating legacy markdown-only sessions
    /// to the event log first. Returns the events for resume projections.
    pub fn open(&self, id: &str) -> Result<(ActiveSession, Vec<Envelope>)> {
        let session = self
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .context("session not found")?;
        let session = if session.has_event_log() {
            session
        } else {
            self.migrate_legacy(session)?
        };
        let events = EventLogReader::new(session.dir()).read_all()?;
        let active = open_session_dir(session)?;
        Ok((active, events))
    }

    /// Read a session's events without taking the writer lock (inspection
    /// commands). Legacy sessions are parsed via the migration path but not
    /// converted.
    pub fn peek(&self, id: &str) -> Result<(Session, Vec<Envelope>)> {
        let session = self
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .context("session not found")?;
        if session.has_event_log() {
            let events = EventLogReader::new(session.dir()).read_all()?;
            return Ok((session, events));
        }
        // Unmigrated legacy: synthesize envelopes in memory.
        let turns = parse_legacy(&session.transcript)?;
        let events = turns
            .into_iter()
            .enumerate()
            .map(|(seq, turn)| {
                let event = SessionEvent::LegacyTurn(LegacyTurn {
                    role: match turn.role {
                        Role::User => "user".into(),
                        Role::Assistant => "assistant".into(),
                    },
                    content: turn.content,
                });
                Envelope {
                    v: artist_session::SCHEMA_VERSION,
                    seq: seq as u64,
                    ts: 0,
                    session: session.id.clone(),
                    run: None,
                    lineage: artist_session::MAIN_LINEAGE.to_owned(),
                    kind: event.kind().to_owned(),
                    payload: event.payload(),
                }
            })
            .collect();
        Ok((session, events))
    }

    /// Fork a session at a point in its history: a new session whose log is
    /// the verbatim event prefix `seq <= up_to_seq` (identical seqs, so
    /// rewind references stay valid) with a fresh `session.created`
    /// carrying the parent pointer. The parent session is untouched.
    pub fn fork(&self, parent_id: &str, up_to_seq: u64) -> Result<ActiveSession> {
        let (parent, events) = self.peek(parent_id)?;
        let duration = SystemTime::now().duration_since(UNIX_EPOCH)?;
        let now = duration.as_millis() as u64;
        let id = format!("{:x}-{:x}", duration.as_nanos(), std::process::id());
        let dir = self.root.join(project_key(&parent.project)).join(&id);
        fs::create_dir_all(&dir)?;
        {
            let mut writer = EventLogWriter::open(&dir, &id)?;
            let mut wrote_created = false;
            for envelope in events.iter().filter(|envelope| envelope.seq <= up_to_seq) {
                if !wrote_created {
                    // Replace the parent's session.created (seq 0) with the
                    // fork's, keeping the seq slot.
                    writer.append(
                        None,
                        artist_session::MAIN_LINEAGE,
                        &SessionEvent::SessionCreated(SessionCreated {
                            project: parent.project.display().to_string(),
                            label: parent.label.clone(),
                            artist_version: env!("CARGO_PKG_VERSION").to_owned(),
                            parent_session: Some(parent.id.clone()),
                        }),
                    )?;
                    wrote_created = true;
                    if envelope.kind == "session.created" {
                        continue;
                    }
                }
                writer.append_raw(envelope)?;
            }
        }
        // Attachments are content-addressed; copy any the prefix may reference.
        let parent_attachments = parent.dir().join("attachments");
        if parent_attachments.is_dir() {
            let fork_attachments = dir.join("attachments");
            fs::create_dir_all(&fork_attachments)?;
            for entry in fs::read_dir(&parent_attachments)?.flatten() {
                let _ = fs::copy(entry.path(), fork_attachments.join(entry.file_name()));
            }
        }
        let fork_events = EventLogReader::new(&dir).read_all()?;
        fs::write(
            dir.join("transcript.md"),
            artist_session::render_markdown(&fork_events),
        )?;
        let session = Session {
            id,
            created_at_ms: now,
            label: parent.label.clone(),
            project: parent.project.clone(),
            transcript: dir.join("transcript.md"),
            parent: Some(parent.id.clone()),
        };
        let _lock = self.lock_index()?;
        let mut index = self.read_index()?;
        match index.projects.iter_mut().find(|p| p.path == parent.project) {
            Some(p) => p.sessions.push(session.clone()),
            None => index.projects.push(Project {
                path: parent.project.clone(),
                sessions: vec![session.clone()],
            }),
        }
        self.write_index(&index)?;
        open_session_dir(session)
    }

    /// One-shot conversion of a legacy markdown session into an event-log
    /// session directory. Idempotent (keyed on events.jsonl existence); the
    /// old markdown becomes the session's transcript projection.
    fn migrate_legacy(&self, session: Session) -> Result<Session> {
        let turns = parse_legacy(&session.transcript)?;
        let dir = self
            .root
            .join(project_key(&session.project))
            .join(&session.id);
        fs::create_dir_all(&dir)?;
        {
            let mut writer = EventLogWriter::open(&dir, &session.id)?;
            writer.append(
                None,
                artist_session::MAIN_LINEAGE,
                &SessionEvent::SessionCreated(SessionCreated {
                    project: session.project.display().to_string(),
                    label: session.label.clone(),
                    artist_version: env!("CARGO_PKG_VERSION").to_owned(),
                    parent_session: None,
                }),
            )?;
            for turn in turns {
                writer.append(
                    None,
                    artist_session::MAIN_LINEAGE,
                    &SessionEvent::LegacyTurn(LegacyTurn {
                        role: match turn.role {
                            Role::User => "user".into(),
                            Role::Assistant => "assistant".into(),
                        },
                        content: turn.content,
                    }),
                )?;
            }
        }
        let new_transcript = dir.join("transcript.md");
        fs::rename(&session.transcript, &new_transcript)
            .or_else(|_| fs::copy(&session.transcript, &new_transcript).map(|_| ()))
            .context("relocate legacy transcript")?;
        let migrated = Session {
            transcript: new_transcript,
            ..session
        };
        let _lock = self.lock_index()?;
        let mut index = self.read_index()?;
        for project in &mut index.projects {
            for entry in &mut project.sessions {
                if entry.id == migrated.id {
                    *entry = migrated.clone();
                }
            }
        }
        self.write_index(&index)?;
        Ok(migrated)
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

    /// Delete a session's files and index entry. Refuses when the session is
    /// active in another process.
    pub fn remove(&self, id: &str) -> Result<()> {
        let session = self
            .list()?
            .into_iter()
            .find(|s| s.id == id)
            .context("session not found")?;
        if session.has_event_log() {
            // Probe the writer lock so we never delete under an active writer.
            let probe = EventLogWriter::open(session.dir(), &session.id)
                .context("session is active in another process")?;
            drop(probe);
            fs::remove_dir_all(session.dir()).context("remove session directory")?;
        } else {
            let _ = fs::remove_file(&session.transcript);
        }
        let _lock = self.lock_index()?;
        let mut index = self.read_index()?;
        for project in &mut index.projects {
            project.sessions.retain(|entry| entry.id != id);
        }
        index
            .projects
            .retain(|project| !project.sessions.is_empty());
        self.write_index(&index)
    }

    fn lock_index(&self) -> Result<fs::File> {
        fs::create_dir_all(&self.root)?;
        let lock = fs::File::create(self.root.join("index.lock"))?;
        lock.lock_exclusive()?;
        Ok(lock)
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

/// Open a session directory's writer and spawn its writer task.
fn open_session_dir(session: Session) -> Result<ActiveSession> {
    let dir = session.dir().to_owned();
    let writer = EventLogWriter::open(&dir, &session.id)?;
    let attachments = AttachmentStore::new(dir.join("attachments"));
    let (recorder, task) = spawn_writer(writer, Some(session.transcript.clone()));
    Ok(ActiveSession {
        session,
        recorder,
        attachments,
        task,
    })
}

/// Parse the legacy `<!-- artist-turn:{json} -->` transcript format.
fn parse_legacy(transcript: &Path) -> Result<Vec<Turn>> {
    let text = fs::read_to_string(transcript).context("read legacy transcript")?;
    text.lines()
        .filter_map(|line| {
            line.strip_prefix("<!-- artist-turn:")
                .and_then(|v| v.strip_suffix(" -->"))
        })
        .map(|json| serde_json::from_str(json).context("parse legacy transcript turn"))
        .collect()
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
    use artist_session::{ContentBlock, TurnUser};

    fn user_turn(text: &str) -> TurnUser {
        TurnUser {
            content: vec![ContentBlock::Text { text: text.into() }],
            display: None,
            source: "prompt".into(),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn create_record_reopen_round_trip() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("proj");
        fs::create_dir(&project)?;
        let store = SessionStore::new(temp.path().join("config"));

        let active = store.create(&project, Some("hello"))?;
        let id = active.session.id.clone();
        active.recorder.record(user_turn("first prompt"));
        active.recorder.flush().await;
        let transcript = active.session.transcript.clone();
        active.close().await?;

        // transcript projection was appended incrementally
        let markdown = fs::read_to_string(&transcript)?;
        assert!(markdown.contains("# Artist session"));
        assert!(markdown.contains("first prompt"));

        let (active, events) = store.open(&id)?;
        assert_eq!(events.len(), 2); // session.created + turn.user
        assert!(matches!(events[0].event(), SessionEvent::SessionCreated(_)));
        active.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn legacy_session_migrates_on_open() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("proj");
        fs::create_dir(&project)?;
        let store = SessionStore::new(temp.path().join("config"));

        // Fabricate a legacy session: flat markdown + index entry.
        let project_canonical = fs::canonicalize(&project)?;
        let key_dir = store.root.join(project_key(&project_canonical));
        fs::create_dir_all(&key_dir)?;
        let transcript = key_dir.join("123-legacy.md");
        fs::write(
            &transcript,
            "# Artist session\n\n<!-- artist-turn:{\"role\":\"user\",\"content\":\"old question\"} -->\n\n## User\n\nold question\n\n<!-- artist-turn:{\"role\":\"assistant\",\"content\":\"old answer\"} -->\n\n## Assistant\n\nold answer\n",
        )?;
        let legacy = Session {
            id: "legacy-1".into(),
            created_at_ms: 1,
            label: Some("old".into()),
            project: project_canonical.clone(),
            transcript,
            parent: None,
        };
        let index = Index {
            projects: vec![Project {
                path: project_canonical,
                sessions: vec![legacy],
            }],
        };
        store.write_index(&index)?;

        let (active, events) = store.open("legacy-1")?;
        let kinds: Vec<_> = events
            .iter()
            .map(|envelope| envelope.kind.clone())
            .collect();
        assert_eq!(kinds, ["session.created", "legacy.turn", "legacy.turn"]);
        assert!(
            active
                .session
                .dir()
                .join(artist_session::EVENTS_FILE)
                .exists()
        );
        assert!(active.session.transcript.ends_with("transcript.md"));
        active.close().await?;

        // Idempotent: reopening does not duplicate events.
        let (active, events) = store.open("legacy-1")?;
        assert_eq!(events.len(), 3);
        active.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn history_rebuilds_from_open_events() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("proj");
        fs::create_dir(&project)?;
        let store = SessionStore::new(temp.path().join("config"));
        let active = store.create(&project, None)?;
        let id = active.session.id.clone();
        active.recorder.record(user_turn("q"));
        active.recorder.record(artist_session::ModelTurn {
            turn: 1,
            content: vec![ContentBlock::Text { text: "a".into() }],
            total_tokens: 10,
            partial: false,
        });
        active.recorder.flush().await;
        active.close().await?;

        let (active, events) = store.open(&id)?;
        let history = artist_session::build_history(
            &events,
            &active.attachments,
            &artist_session::HistoryOptions::default(),
        )?;
        assert_eq!(history.len(), 2);
        active.close().await?;
        Ok(())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fork_copies_prefix_with_stable_seqs_and_parent_pointer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let project = temp.path().join("proj");
        fs::create_dir(&project)?;
        let store = SessionStore::new(temp.path().join("config"));
        let active = store.create(&project, Some("root"))?;
        let parent_id = active.session.id.clone();
        active.recorder.record(user_turn("one"));
        active.recorder.record(artist_session::ModelTurn {
            turn: 1,
            content: vec![ContentBlock::Text { text: "a1".into() }],
            total_tokens: 0,
            partial: false,
        });
        active.recorder.record(user_turn("two"));
        active.recorder.record(artist_session::ModelTurn {
            turn: 1,
            content: vec![ContentBlock::Text { text: "a2".into() }],
            total_tokens: 0,
            partial: false,
        });
        active.recorder.flush().await;
        // Fork before "two" (seq 3): keep seqs 0..=2.
        let fork = store.fork(&parent_id, 2)?;
        assert_eq!(fork.session.parent.as_deref(), Some(parent_id.as_str()));
        let events = fork.events()?;
        assert_eq!(events.len(), 3);
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
        match events[0].event() {
            SessionEvent::SessionCreated(created) => {
                assert_eq!(created.parent_session.as_deref(), Some(parent_id.as_str()));
            }
            other => panic!("unexpected {other:?}"),
        }
        let history = artist_session::build_history(
            &events,
            &fork.attachments,
            &artist_session::HistoryOptions::default(),
        )?;
        assert_eq!(history.len(), 2, "user one + assistant a1 only");
        fork.close().await?;
        // Parent untouched and still openable.
        active.close().await?;
        let (parent, parent_events) = store.open(&parent_id)?;
        assert_eq!(parent_events.len(), 5);
        parent.close().await?;
        Ok(())
    }
}
