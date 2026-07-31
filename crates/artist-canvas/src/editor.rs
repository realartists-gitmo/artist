//! Opening a file where the user actually works.
//!
//! A canvas that shows `palette.rs:131` should be one click from the editor.
//! Nothing else in the kit can do that — it is the clearest case of a component
//! that exists only because a harness is underneath the page.
//!
//! # Why `$EDITOR` is not simply obeyed
//!
//! `$EDITOR` is conventionally a *terminal* editor, and artist's terminal is
//! already occupied: the TUI holds it in raw mode with an inline viewport. A
//! spawned `vim` would draw over the conversation and neither program would
//! recover. So `$VISUAL` — which exists precisely to mean "the one that opens
//! its own window" — is preferred, and an `$EDITOR` naming a known terminal
//! editor is skipped in favour of the platform opener.
//!
//! # Why the path is checked
//!
//! The request comes from a page, and the page is model-authored. Without a
//! check this is "spawn a program on an arbitrary path" with a friendly name.
//! Paths resolve inside the project and nowhere else.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// Editors that take over the terminal they are launched from.
const TERMINAL_EDITORS: &[&str] =
    &["vi", "vim", "nvim", "nano", "pico", "emacs", "helix", "hx", "kak", "micro", "ed"];

/// Editors that can be told to land on a line, and how to tell them.
///
/// An editor missing from this table still opens — it just opens at the top of
/// the file, which is the right failure: a file open at line 1 is useful, and a
/// flag guessed wrong is an error dialog or a file named `+131`.
const LINE_FLAGS: &[(&str, LineSyntax)] = &[
    ("code", LineSyntax::Goto),
    ("code-insiders", LineSyntax::Goto),
    ("cursor", LineSyntax::Goto),
    ("codium", LineSyntax::Goto),
    ("windsurf", LineSyntax::Goto),
    ("zed", LineSyntax::Suffix),
    ("subl", LineSyntax::Suffix),
    ("sublime_text", LineSyntax::Suffix),
    ("idea", LineSyntax::DashDashLine),
    ("pycharm", LineSyntax::DashDashLine),
    ("rustrover", LineSyntax::DashDashLine),
    ("gvim", LineSyntax::PlusLine),
    ("mvim", LineSyntax::PlusLine),
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LineSyntax {
    /// `code --goto file:line`
    Goto,
    /// `zed file:line`
    Suffix,
    /// `idea --line 131 file`
    DashDashLine,
    /// `gvim +131 file`
    PlusLine,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("`{0}` is outside the project")]
    Outside(String),
    #[error("{0} does not exist")]
    Missing(String),
    #[error("no editor found: set $VISUAL to one that opens its own window")]
    NoEditor,
    #[error("could not launch {program}: {source}")]
    Launch {
        program: String,
        #[source]
        source: std::io::Error,
    },
}

/// Resolve a canvas-supplied path against the project.
///
/// Symlinks are followed before the containment check, so a link planted inside
/// the project cannot be used to point out of it.
pub fn resolve(project: &Path, path: &str) -> Result<PathBuf, EditError> {
    let joined = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        project.join(path)
    };
    let real = joined
        .canonicalize()
        .map_err(|_| EditError::Missing(path.to_owned()))?;
    let root = project.canonicalize().unwrap_or_else(|_| project.to_owned());
    if !real.starts_with(&root) {
        return Err(EditError::Outside(path.to_owned()));
    }
    Ok(real)
}

/// The program and arguments that would open `path`, if anything can.
///
/// Split from the spawn so the choice is testable without launching a window.
fn invocation(path: &Path, line: Option<u32>, env: &dyn Fn(&str) -> Option<String>) -> Option<Vec<OsString>> {
    let configured = ["ARTIST_EDITOR", "VISUAL", "EDITOR"].into_iter().find_map(|name| {
        let value = env(name)?;
        let value = value.trim().to_owned();
        if value.is_empty() {
            return None;
        }
        // A terminal editor would draw over the TUI that is already using this
        // terminal, so it is passed over rather than obeyed.
        let program = value.split_whitespace().next()?;
        let stem = Path::new(program).file_stem()?.to_string_lossy().into_owned();
        (!(name == "EDITOR" && TERMINAL_EDITORS.contains(&stem.as_str()))).then_some(value)
    });

    let display = path.display().to_string();
    if let Some(configured) = configured {
        // The value may carry flags of its own (`code -n`), which is how people
        // write these variables; the program is the first word.
        let mut words: Vec<OsString> = configured.split_whitespace().map(OsString::from).collect();
        let stem = Path::new(&words[0])
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let syntax = line.and_then(|_| {
            LINE_FLAGS
                .iter()
                .find_map(|(name, syntax)| (*name == stem).then_some(*syntax))
        });
        match (syntax, line) {
            (Some(LineSyntax::Goto), Some(line)) => {
                words.push("--goto".into());
                words.push(format!("{display}:{line}").into());
            }
            (Some(LineSyntax::Suffix), Some(line)) => words.push(format!("{display}:{line}").into()),
            (Some(LineSyntax::DashDashLine), Some(line)) => {
                words.push("--line".into());
                words.push(line.to_string().into());
                words.push(display.into());
            }
            (Some(LineSyntax::PlusLine), Some(line)) => {
                words.push(format!("+{line}").into());
                words.push(display.into());
            }
            _ => words.push(display.into()),
        }
        return Some(words);
    }

    // Nothing configured: hand it to whatever the desktop already associates
    // with the file. No line, but the file opens, which is most of the value.
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    Some(vec![OsString::from(opener), display.into()])
}

/// Open `path` in the user's editor.
pub fn open(project: &Path, path: &str, line: Option<u32>) -> Result<String, EditError> {
    let resolved = resolve(project, path)?;
    let words = invocation(&resolved, line, &|name| std::env::var(name).ok())
        .ok_or(EditError::NoEditor)?;

    let program = words[0].to_string_lossy().into_owned();
    Command::new(&words[0])
        .args(&words[1..])
        // The TUI is drawing in this terminal; a launcher's warnings must not
        // land in the middle of it.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|source| EditError::Launch {
            program: program.clone(),
            source,
        })?;
    Ok(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |name| {
            pairs
                .iter()
                .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
        }
    }

    fn words(path: &str, line: Option<u32>, pairs: &'static [(&'static str, &'static str)]) -> Vec<String> {
        invocation(Path::new(path), line, &env(pairs))
            .expect("something should always open")
            .into_iter()
            .map(|word| word.to_string_lossy().into_owned())
            .collect()
    }

    /// Each editor gets the syntax it actually understands. A guessed flag is
    /// worse than no line: it becomes an error dialog or a file named `+131`.
    #[test]
    fn the_line_reaches_the_editor_in_its_own_dialect() {
        assert_eq!(
            words("/p/a.rs", Some(131), &[("VISUAL", "code")]),
            ["code", "--goto", "/p/a.rs:131"]
        );
        assert_eq!(words("/p/a.rs", Some(131), &[("VISUAL", "zed")]), ["zed", "/p/a.rs:131"]);
        assert_eq!(
            words("/p/a.rs", Some(131), &[("VISUAL", "idea")]),
            ["idea", "--line", "131", "/p/a.rs"]
        );
        assert_eq!(words("/p/a.rs", Some(131), &[("VISUAL", "gvim")]), ["gvim", "+131", "/p/a.rs"]);
    }

    /// An editor we do not know still opens the file, at the top.
    #[test]
    fn an_unknown_editor_gets_the_path_alone() {
        assert_eq!(words("/p/a.rs", Some(9), &[("VISUAL", "myeditor")]), ["myeditor", "/p/a.rs"]);
        assert_eq!(words("/p/a.rs", None, &[("VISUAL", "code")]), ["code", "/p/a.rs"]);
    }

    /// People write `VISUAL="code -n"`, and the program is the first word.
    #[test]
    fn flags_in_the_variable_survive() {
        assert_eq!(
            words("/p/a.rs", Some(4), &[("VISUAL", "code -n")]),
            ["code", "-n", "--goto", "/p/a.rs:4"]
        );
    }

    /// The TUI owns this terminal. Launching `vim` into it would draw over the
    /// conversation and leave neither program usable.
    #[test]
    fn a_terminal_editor_in_editor_is_passed_over() {
        let chosen = words("/p/a.rs", None, &[("EDITOR", "vim")]);
        assert_ne!(chosen[0], "vim");
        assert!(
            ["xdg-open", "open", "explorer"].contains(&chosen[0].as_str()),
            "{chosen:?}"
        );

        // $VISUAL means "opens its own window", so it is taken at its word even
        // when it names something we do not recognise.
        assert_eq!(words("/p/a.rs", None, &[("VISUAL", "nvim-qt")])[0], "nvim-qt");
        // A graphical $EDITOR is fine; only the terminal ones are skipped.
        assert_eq!(words("/p/a.rs", None, &[("EDITOR", "code")])[0], "code");
    }

    #[test]
    fn precedence_runs_artist_then_visual_then_editor() {
        assert_eq!(
            words("/p/a.rs", None, &[("ARTIST_EDITOR", "a"), ("VISUAL", "b"), ("EDITOR", "c")])[0],
            "a"
        );
        assert_eq!(words("/p/a.rs", None, &[("VISUAL", "b"), ("EDITOR", "c")])[0], "b");
        // An empty variable is not a choice.
        assert_eq!(words("/p/a.rs", None, &[("VISUAL", "  "), ("EDITOR", "c")])[0], "c");
    }

    /// The path arrives from a model-authored page, so this is otherwise
    /// "launch a program against any path on the machine".
    #[test]
    fn paths_resolve_inside_the_project_and_nowhere_else() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let project = temporary.path();
        std::fs::write(project.join("main.jsx"), "x").expect("write");
        std::fs::create_dir_all(project.join("src")).expect("mkdir");
        std::fs::write(project.join("src/lib.rs"), "x").expect("write");

        assert!(resolve(project, "main.jsx").is_ok());
        assert!(resolve(project, "src/lib.rs").is_ok());

        for outside in ["../../../etc/passwd", "/etc/passwd", "src/../../../etc/hosts"] {
            assert!(
                matches!(
                    resolve(project, outside),
                    Err(EditError::Outside(_) | EditError::Missing(_))
                ),
                "{outside} was allowed"
            );
        }
        assert!(matches!(resolve(project, "nope.rs"), Err(EditError::Missing(_))));
    }

    /// A symlink planted inside the project must not become a way out of it,
    /// which is why containment is checked after canonicalising.
    #[test]
    #[cfg(unix)]
    fn a_symlink_cannot_lead_out() {
        let temporary = tempfile::tempdir().expect("tempdir");
        let project = temporary.path();
        std::os::unix::fs::symlink("/etc/passwd", project.join("escape.rs")).expect("symlink");
        assert!(matches!(resolve(project, "escape.rs"), Err(EditError::Outside(_))));
    }
}
