use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{Context, Result};
use artist::{
    Node, Runtime, SqliteStore,
    config::ConfigPaths,
    context::ContextBuilder,
    plugin::{PluginHost, PluginManifest},
    python::PythonRepl,
    scheduler::Scheduler,
};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

/// Durable append-only agent harness.
#[derive(usage::Cli)]
#[usage(
    bin = "artist",
    version,
    unknown_flags = "error",
    args_override_self = false
)]
struct Cli {
    #[usage(long, default = ".artist/artist.db", global)]
    database: PathBuf,
    #[usage(subcommand)]
    command: Command,
}

#[derive(usage::Subcommands)]
enum Command {
    /// Create local state and editable prompt files.
    Init,
    Node {
        #[usage(subcommand)]
        command: NodeCommand,
    },
    Run {
        #[usage(subcommand)]
        command: RunCommand,
    },
    /// Execute code in a fresh per-run Python process.
    Python {
        run_id: Uuid,
        code: String,
        /// Python executable containing artist-repl and fff-search.
        #[usage(long)]
        python: Option<PathBuf>,
    },
    Plugin {
        #[usage(subcommand)]
        command: PluginCommand,
    },
    /// Repair interrupted actions and wake completed joins.
    Schedule {
        #[usage(long)]
        recover: bool,
    },
}

#[derive(usage::Subcommands)]
enum NodeCommand {
    Create {
        name: String,
        instructions: PathBuf,
        #[usage(long, default = "{}")]
        input_schema: String,
        #[usage(long, default = "{}")]
        output_schema: String,
    },
    Add {
        file: PathBuf,
    },
    List,
    Show {
        node_id: Uuid,
    },
}

#[derive(usage::Subcommands)]
enum RunCommand {
    Start {
        node_id: Uuid,
        input: String,
    },
    List,
    Show {
        run_id: Uuid,
    },
    Events {
        run_id: Uuid,
    },
    Context {
        run_id: Uuid,
    },
    Append {
        run_id: Uuid,
        text: String,
    },
    Ask {
        run_id: Uuid,
        question: String,
    },
    Answer {
        run_id: Uuid,
        answer: String,
    },
    Yield {
        run_id: Uuid,
        value: String,
    },
    Spawn {
        parent_id: Uuid,
        node_id: Uuid,
        input: String,
    },
    Fork {
        parent_id: Uuid,
        /// JSON array containing one typed input per branch.
        inputs: String,
        #[usage(long)]
        no_join: bool,
    },
    Join {
        parent_id: Uuid,
    },
    Replace {
        run_id: Uuid,
        /// JSON array of {"node_id":"...", "input": ...}.
        successors: String,
    },
    Cancel {
        run_id: Uuid,
        reason: String,
    },
}

#[derive(usage::Subcommands)]
enum PluginCommand {
    /// Validate a WebAssembly component and its requested capabilities.
    Validate {
        manifest: PathBuf,
        component: PathBuf,
        #[usage(long = "allow")]
        allowed: Vec<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, Command::Init) {
        return init(&cli.database);
    }
    let runtime = open_runtime(&cli.database)?;
    match cli.command {
        Command::Init => unreachable!(),
        Command::Node { command } => node_command(&runtime, command)?,
        Command::Run { command } => run_command(&runtime, command)?,
        Command::Python {
            run_id,
            code,
            python,
        } => {
            let worker = Path::new(env!("CARGO_MANIFEST_DIR")).join("python/worker.py");
            let python = python.unwrap_or_else(default_python);
            runtime.note_repl_restart(run_id, "new CLI Python session")?;
            let mut repl = PythonRepl::start(
                python.to_str().context("Python path is not valid UTF-8")?,
                &worker,
                &std::env::current_dir()?,
            )
            .await?;
            if !repl.fff_available {
                anyhow::bail!(
                    "fff-search is required but unavailable: {}. Run `uv sync --project {}/python`",
                    repl.fff_error.as_deref().unwrap_or("unknown import error"),
                    env!("CARGO_MANIFEST_DIR")
                );
            }
            let output = runtime.execute_python(run_id, &mut repl, &code).await?;
            print_json(&output)?;
            repl.shutdown().await?;
        }
        Command::Plugin { command } => match command {
            PluginCommand::Validate {
                manifest,
                component,
                allowed,
            } => {
                let manifest: PluginManifest = serde_json::from_slice(&fs::read(manifest)?)?;
                let host = PluginHost::new(allowed)?;
                let loaded = host.load(manifest, component)?;
                print_json(&loaded.manifest)?;
            }
        },
        Command::Schedule { recover } => {
            let scheduler = Scheduler::new(&runtime);
            print_json(&if recover {
                scheduler.recover()?
            } else {
                scheduler.tick()?
            })?;
        }
    }
    Ok(())
}

fn init(database: &Path) -> Result<()> {
    let root = database.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(root)?;
    SqliteStore::open(database)?;
    setup_python()?;
    let config = ConfigPaths::discover(std::env::current_dir()?)?;
    config.initialize()?;
    println!("initialized {}", database.display());
    println!("global config: {}", config.global.display());
    println!("workspace config: {}", config.workspace.display());
    Ok(())
}

fn setup_python() -> Result<()> {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("python");
    let status = ProcessCommand::new("uv")
        .arg("sync")
        .arg("--project")
        .arg(&project)
        .status()
        .context("Artist requires `uv` to install its Python REPL environment")?;
    if !status.success() {
        anyhow::bail!("failed to install artist-repl and fff-search with uv");
    }
    Ok(())
}

fn default_python() -> PathBuf {
    std::env::var_os("ARTIST_PYTHON")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("python/.venv/bin/python"))
}

fn open_runtime(database: &Path) -> Result<Runtime> {
    if let Some(parent) = database.parent() {
        fs::create_dir_all(parent)?;
    }
    let config = ConfigPaths::discover(std::env::current_dir()?)?;
    Ok(Runtime::new(
        SqliteStore::open(database)?,
        config.system_instructions()?,
        config.agents_instructions()?,
    ))
}

fn node_command(runtime: &Runtime, command: NodeCommand) -> Result<()> {
    match command {
        NodeCommand::Create {
            name,
            instructions,
            input_schema,
            output_schema,
        } => {
            let node = Node::new(
                name,
                fs::read_to_string(instructions)?,
                parse_json(&input_schema)?,
                parse_json(&output_schema)?,
            );
            runtime.register_node(&node)?;
            print_json(&node)?;
        }
        NodeCommand::Add { file } => {
            let node: Node = serde_json::from_slice(&fs::read(file)?)?;
            runtime.register_node(&node)?;
            print_json(&node)?;
        }
        NodeCommand::List => print_json(&runtime.store().list_nodes()?)?,
        NodeCommand::Show { node_id } => print_json(&runtime.store().get_node(node_id)?)?,
    }
    Ok(())
}

fn run_command(runtime: &Runtime, command: RunCommand) -> Result<()> {
    match command {
        RunCommand::Start { node_id, input } => {
            print_json(&runtime.start(node_id, parse_json(&input)?)?)?
        }
        RunCommand::List => print_json(&runtime.store().list_runs()?)?,
        RunCommand::Show { run_id } => print_json(&runtime.store().run(run_id)?)?,
        RunCommand::Events { run_id } => print_json(&runtime.store().events(run_id)?)?,
        RunCommand::Context { run_id } => {
            print_json(&ContextBuilder::new(runtime.store()).build(run_id)?)?
        }
        RunCommand::Append { run_id, text } => runtime.append_context(run_id, text)?,
        RunCommand::Ask { run_id, question } => runtime.ask(run_id, question)?,
        RunCommand::Answer { run_id, answer } => runtime.answer(run_id, answer)?,
        RunCommand::Yield { run_id, value } => runtime.yield_value(run_id, parse_json(&value)?)?,
        RunCommand::Spawn {
            parent_id,
            node_id,
            input,
        } => print_json(&runtime.spawn(parent_id, node_id, parse_json(&input)?)?)?,
        RunCommand::Fork {
            parent_id,
            inputs,
            no_join,
        } => {
            let inputs = parse_json(&inputs)?;
            let values = inputs
                .as_array()
                .cloned()
                .context("inputs must be a JSON array")?;
            let handles = if no_join {
                runtime.fork(parent_id, values)?
            } else {
                runtime.fork_and_join(parent_id, values)?
            };
            print_json(&handles)?;
        }
        RunCommand::Join { parent_id } => print_json(&runtime.try_join(parent_id)?)?,
        RunCommand::Replace { run_id, successors } => {
            let values = parse_json(&successors)?;
            let values = values
                .as_array()
                .context("successors must be a JSON array")?;
            let mut parsed = Vec::new();
            for value in values {
                let node_id = value
                    .get("node_id")
                    .and_then(Value::as_str)
                    .context("successor node_id must be a string")?;
                parsed.push((
                    Uuid::parse_str(node_id)?,
                    value.get("input").cloned().unwrap_or(Value::Null),
                ));
            }
            print_json(&runtime.replace(run_id, parsed)?)?;
        }
        RunCommand::Cancel { run_id, reason } => runtime.cancel(run_id, reason)?,
    }
    Ok(())
}

fn parse_json(argument: &str) -> Result<Value> {
    let source = if let Some(path) = argument.strip_prefix('@') {
        fs::read_to_string(path).with_context(|| format!("failed to read {path}"))?
    } else {
        argument.to_owned()
    };
    serde_json::from_str(&source).with_context(|| "invalid JSON")
}

fn print_json(value: &impl Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}
