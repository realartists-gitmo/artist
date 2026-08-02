use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "artist", version, about = "The Artist coding agent")]
pub struct Cli {
    /// Prompt to send immediately, or a project directory to open.
    #[arg(value_name = "PROMPT_OR_PROJECT")]
    pub prompt: Option<String>,
    /// Execute one prompt and print the response without opening the chat UI.
    #[arg(short = 'p', long, value_name = "PROMPT")]
    pub print_prompt: Option<String>,
    /// Resume a session by ID, or select one interactively when no ID is given.
    #[arg(short = 'r', long = "resume", value_name = "SESSION_ID", num_args = 0..=1, default_missing_value = "")]
    pub resume: Option<String>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Select the model and reasoning effort for the default provider.
    Model,
    /// Manage stream rules.
    Rules(RulesArgs),
    /// Inspect and maintain stored sessions.
    Sessions(SessionsArgs),
    /// Inspect agent profiles.
    Profiles(ProfilesArgs),
    /// Inspect and maintain durable memory.
    Memory(MemoryArgs),
    /// Inspect what the agent did with computer use.
    Computer(ComputerArgs),
}

#[derive(Debug, Args)]
pub struct ComputerArgs {
    #[command(subcommand)]
    pub action: ComputerCommand,
}

#[derive(Debug, Subcommand)]
pub enum ComputerCommand {
    /// Check whether computer use will work on this machine.
    ///
    /// Every check here already existed as a runtime error thrown at the moment
    /// a task failed. This is the same information, before anything fails, with
    /// the fix attached to each problem.
    Doctor {
        /// Apply the repairs this can perform itself.
        ///
        /// Only ever our own state — fetching model weights into our model
        /// directory. Anything needing a package manager and root is reported
        /// with the command to run, never run for you.
        #[arg(long)]
        fix: bool,
    },
    /// Print the observe/act sequence for a session.
    ///
    /// The event log already holds everything an inspector would show —
    /// anchors, the label the model claimed, the name the element actually had,
    /// settle timings and frame digests — so this is the debugging surface for
    /// computer use.
    Log {
        /// Session id. Defaults to the most recent session for this project.
        id: Option<String>,
    },
    /// Emit a replayable macro from what the agent already did.
    ///
    /// Every successful program is in the log as `(anchor, action, expect)`
    /// triples, so a trajectory that worked once can be replayed
    /// deterministically instead of being rediscovered by the model.
    Distill {
        /// Session id. Defaults to the most recent session for this project.
        id: Option<String>,
        /// Include programs that failed or whose expectation was not met.
        #[arg(long)]
        include_failed: bool,
    },
    /// Replay a distilled macro against a fresh application.
    ///
    /// A macro records *what was done*, never what to do it to: the event log
    /// has no launch command, and the surface id it carries belonged to a
    /// session that is gone. So the program to run is given here, and the macro
    /// supplies the steps.
    /// Hold a computer-use session open so it can be driven from outside.
    ///
    /// The registry and its stage live for the life of *one process* — a stage
    /// is killed on `Drop`, deliberately — so a one-shot command could never
    /// carry a surface from one call to the next. This keeps one alive and
    /// takes tool calls over a socket, which is what makes the tool drivable by
    /// anything that can run a command: a test, a script, or a model that is
    /// not this harness.
    Serve {
        /// Socket path. Defaults to `$XDG_RUNTIME_DIR/artist-computer.sock`.
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
    },
    /// Send one `computer` tool call to a running `serve`, and print the reply.
    ///
    /// The argument is the tool's own JSON, exactly as a model would emit it —
    /// so what comes back is exactly what a model would read, including the
    /// error wording. That is the point: no other test exercises the words.
    Call {
        /// The tool arguments as JSON, e.g. `{"mode":"surfaces"}`.
        json: String,
        #[arg(long)]
        socket: Option<std::path::PathBuf>,
    },
    /// Write a session's computer-use trajectory as one JSON document.
    ///
    /// A trajectory is the whole record — every observation and every action, in
    /// order, with the frame digests that name the screenshots. `log` renders
    /// that for a person and `distill` reduces it to a replayable macro; this is
    /// the lossless middle, which is what a benchmark harness reads and what a
    /// future fine-tune trains on.
    Export {
        /// Session id. Defaults to the most recent for this project.
        id: Option<String>,
        /// Write here instead of standard output.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
    Replay {
        /// A macro file produced by `artist computer distill`.
        file: std::path::PathBuf,
        /// The program to run it against, e.g. `chromium https://example.com`.
        ///
        /// Optional when the macro records its own launch, which distilled
        /// macros now do. Given here it overrides the recorded one.
        #[arg(long)]
        launch: Option<String>,
        /// Allow a step whose element has been renamed to run anyway.
        ///
        /// Off by default. A macro is trusted because it is the path that was
        /// worked out; running it against a merely similar element turns a
        /// script back into a guess. When on, every substitution is reported.
        #[arg(long)]
        heal: bool,
    },
    /// Write a captured frame out by its digest, for an image viewer.
    Frame {
        /// The `img:<sha>` digest from a log line or transcript.
        digest: String,
        /// Where to write it. Defaults to `<digest>.png` in the current directory.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

#[derive(Debug, Args)]
pub struct MemoryArgs {
    #[command(subcommand)]
    pub action: MemoryCommand,
}

#[derive(Debug, Subcommand)]
pub enum MemoryCommand {
    /// List the facts remembered for this project and globally.
    List,
    /// Search memory the way the agent would.
    Search { query: String },
    /// Write a logical JSON dump to stdout.
    ///
    /// The store is a rebuildable projection and the pinned backend commits
    /// without syncing its WAL, so take one of these before upgrading.
    Export,
    /// Load a dump produced by `export`, then rebuild every index.
    Import { path: String },
    /// Drop and recreate every search index.
    ///
    /// Bulk imports bypass index maintenance, and a full drop-and-create is far
    /// faster than the incremental reindex path.
    Reindex,
    /// Report store health: counts, schema version, and whether the store
    /// agrees with the session log.
    Verify,
}

#[derive(Debug, Args)]
pub struct ProfilesArgs {
    #[command(subcommand)]
    pub action: ProfilesCommand,
}

#[derive(Debug, Subcommand)]
pub enum ProfilesCommand {
    /// List the profiles available in this project and where each comes from.
    List,
    /// Print a built-in profile, ready to save as an override.
    ///
    /// Nothing is written to your config until you do it: redirect this into
    /// .artist/profiles/<name>.md and edit from there.
    Show { name: String },
}

#[derive(Debug, Args)]
pub struct SessionsArgs {
    #[command(subcommand)]
    pub action: SessionsCommand,
}

#[derive(Debug, Subcommand)]
pub enum SessionsCommand {
    /// List sessions for the current project with on-disk sizes.
    List,
    /// Regenerate a session's markdown transcript from its event log.
    Render { id: String },
    /// Delete old sessions (never the N most recent per project).
    Gc {
        /// Keep this many recent sessions per project.
        #[arg(long, default_value_t = 10)]
        keep: usize,
        /// Only delete sessions older than this many days.
        #[arg(long, default_value_t = 30)]
        older_than_days: u64,
        /// Show what would be deleted without deleting.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Debug, Args)]
pub struct RulesArgs {
    #[command(subcommand)]
    pub action: RulesCommand,
}

#[derive(Debug, Subcommand)]
pub enum RulesCommand {
    /// Scaffold a new declarative rule in .artist/rules/.
    New { name: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_requested_forms() {
        let provider = Cli::try_parse_from(["artist", "provider"]).unwrap();
        assert!(provider.command.is_none());
        assert_eq!(provider.prompt.as_deref(), Some("provider"));
        assert!(Cli::try_parse_from(["artist", "model"]).is_ok());
        let cli = Cli::try_parse_from(["artist", "-p", "reply OK"]).unwrap();
        assert_eq!(cli.print_prompt.as_deref(), Some("reply OK"));
        let cli = Cli::try_parse_from(["artist", "-p", "reply OK", "/tmp"]).unwrap();
        assert_eq!(cli.print_prompt.as_deref(), Some("reply OK"));
        assert_eq!(cli.prompt.as_deref(), Some("/tmp"));
        assert_eq!(
            Cli::try_parse_from(["artist", "hello"])
                .unwrap()
                .prompt
                .as_deref(),
            Some("hello")
        );
        let resumed = Cli::try_parse_from(["artist", "hello", "-r", "abc"]).unwrap();
        assert_eq!(resumed.prompt.as_deref(), Some("hello"));
        assert_eq!(resumed.resume.as_deref(), Some("abc"));
        assert_eq!(
            Cli::try_parse_from(["artist", "-r"])
                .unwrap()
                .resume
                .as_deref(),
            Some("")
        );
        assert_eq!(
            Cli::try_parse_from(["artist", "-p", "next", "-r"])
                .unwrap()
                .resume
                .as_deref(),
            Some("")
        );
        assert_eq!(
            Cli::try_parse_from(["artist", "-p", "next", "-r", "abc"])
                .unwrap()
                .resume
                .as_deref(),
            Some("abc")
        );
    }
}
