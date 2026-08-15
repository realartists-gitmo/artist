use crate::args::ResourceVerb as Verb;
use anyhow::{Context, Result, anyhow, bail};
use artist_component::resources::{
    DynamicResourcesProvider, ResourceVerbBindings as ComponentResourceVerbBindings,
    ResourcesHandler,
};
use artist_component::tools::{DynamicToolsProvider, ToolsHandler, ToolsVerbBindings};
use artist_component::watcher::SharedWatcher;
use artist_kernel::{
    DynamicValue, FileHandler, FileResourceProvider, FileVerbBindings, Kernel, ProcessManager,
    ProcessResourceProvider, ProcessVerbBindings, RepositoryHandler, RepositoryResourceProvider,
    RepositoryVerbBindings, ResourceAddress, ResourceUri, SessionHandler, SessionResourceProvider,
    SessionVerbBindings, VerbDefinition, VerbId,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

pub async fn build(root: &Path) -> Result<Kernel> {
    let kernel = Kernel::new();

    // The invocation stream is a first-class resource namespace. Its
    // channel verbs are activated once here; individual tool packages never
    // need to know how invocation resources are stored or polled.
    for function in ["read", "write", "poll", "abort", "delete"] {
        let identity = native_verb("invocations", function);
        kernel.activate_verb(VerbDefinition::new(
            identity,
            function,
            "invocation-channel",
            "Read or control one invocation channel",
        ))?;
    }

    // Native implementations publish their package-owned dynamic identities
    // at the kernel boundary.
    kernel.register_dynamic_resource_provider(Arc::new(FileResourceProvider::new(
        Arc::new(FileHandler::new(root)?),
        FileVerbBindings {
            read: native_verb("filesystem", "read"),
            write: native_verb("filesystem", "write"),
            edit: native_verb("filesystem", "edit"),
            insert: native_verb("filesystem", "insert"),
            delete: native_verb("filesystem", "delete"),
            find: native_verb("filesystem", "find"),
            grep: native_verb("filesystem", "grep"),
        },
    )))?;
    kernel.register_dynamic_resource_provider(Arc::new(SessionResourceProvider::new(
        SessionHandler::new(),
        SessionVerbBindings {
            read: native_verb("session", "read"),
            write: native_verb("session", "write"),
            poll: native_verb("session", "poll"),
            abort: native_verb("session", "abort"),
            delete: native_verb("session", "delete"),
        },
    )))?;
    kernel.register_dynamic_resource_provider(Arc::new(RepositoryResourceProvider::new(
        Arc::new(RepositoryHandler::new(root)?),
        RepositoryVerbBindings {
            read: native_verb("repository", "read"),
            find: native_verb("repository", "find"),
            grep: native_verb("repository", "grep"),
        },
    )))?;
    let process_bindings = ProcessVerbBindings {
        run: native_verb("process", "run"),
        write: native_verb("process", "write"),
        read: native_verb("process", "read"),
        poll: native_verb("process", "poll"),
        abort: native_verb("process", "abort"),
        delete: native_verb("process", "delete"),
    };
    for definition in process_bindings.definitions() {
        kernel.route_registry().register(
            definition.identity.clone(),
            Arc::new(artist_kernel::ResourceUriValueExtractor),
        )?;
        kernel.activate_verb(definition)?;
    }
    kernel.register_dynamic_resource_provider(Arc::new(ProcessResourceProvider::new(
        ProcessManager::new(),
        process_bindings,
    )))?;
    let tools_root = root.join("tools");
    std::fs::create_dir_all(&tools_root)
        .with_context(|| format!("initialize tools root at {}", tools_root.display()))?;
    seed_universal_tools(&tools_root)
        .with_context(|| format!("seed universal tools at {}", tools_root.display()))?;
    let resources_root = root.join("resources");
    std::fs::create_dir_all(&resources_root)
        .with_context(|| format!("initialize resources root at {}", resources_root.display()))?;
    seed_ast_resource(&resources_root)
        .with_context(|| format!("seed AST resource at {}", resources_root.display()))?;
    let shared_watcher = SharedWatcher::new();
    let resources = Arc::new(
        ResourcesHandler::new_with_watcher(&resources_root, Some(&shared_watcher))
            .with_context(|| format!("load resources at {}", resources_root.display()))?
            .without_file_package("artist-ast"),
    );
    kernel.register_dynamic_resource_provider(Arc::new(DynamicResourcesProvider::new(
        resources.clone(),
        ComponentResourceVerbBindings {
            read: native_verb("resources", "read"),
            write: native_verb("resources", "write"),
            edit: native_verb("resources", "edit"),
            insert: native_verb("resources", "insert"),
            poll: native_verb("resources", "poll"),
            run: native_verb("resources", "run"),
            abort: native_verb("resources", "abort"),
            delete: native_verb("resources", "delete"),
            find: native_verb("resources", "find"),
            grep: native_verb("resources", "grep"),
        },
    )))?;
    let tools =
        ToolsHandler::new_with_watcher(&tools_root, Vec::<String>::new(), Some(&shared_watcher))
            .with_context(|| format!("load tools at {}", tools_root.display()))?;
    kernel.register_dynamic_resource_provider(Arc::new(DynamicToolsProvider::new(
        Arc::new(tools.clone()),
        ToolsVerbBindings {
            read: native_verb("tools", "read"),
            write: native_verb("tools", "write"),
            edit: native_verb("tools", "edit"),
            insert: native_verb("tools", "insert"),
            delete: native_verb("tools", "delete"),
            find: native_verb("tools", "find"),
            grep: native_verb("tools", "grep"),
        },
    )))?;
    let dynamic_definitions = tools
        .dynamic_verb_definitions()
        .map_err(|error| anyhow!(error.to_string()))?;
    let route_registry = kernel.route_registry();
    for definition in &dynamic_definitions {
        route_registry
            .register(
                definition.identity.clone(),
                Arc::new(artist_kernel::ResourceUriValueExtractor),
            )
            .map_err(|error| anyhow!(error.to_string()))?;
    }
    kernel
        .activate_verbs(dynamic_definitions)
        .map_err(|error| anyhow!(error.to_string()))?;
    let package_watcher = shared_watcher.start([&tools_root, &resources_root])?;
    kernel.retain_background(package_watcher);
    kernel.retain_background(DynamicVerbCatalogWatcher::start(
        kernel.clone(),
        tools.clone(),
    ));
    kernel.register_tool_provider(tools).await;
    Ok(kernel)
}

fn native_verb(namespace: &str, function: &str) -> VerbId {
    VerbId::new(format!("artist:{namespace}/{function}@1.0.0"))
        .expect("native verb identities are canonical")
}

/// Keeps the model-visible dynamic verb catalog synchronized with editable
/// tool packages. Component generation reloads remain owned by `ToolsHandler`;
/// this companion only publishes the package-definition set atomically.
struct DynamicVerbCatalogWatcher {
    stop: Option<mpsc::Sender<()>>,
    join: Option<thread::JoinHandle<()>>,
}

impl DynamicVerbCatalogWatcher {
    fn start(kernel: Kernel, tools: ToolsHandler) -> Self {
        let (stop, receiver) = mpsc::channel();
        let join = thread::spawn(move || {
            loop {
                if receiver.recv_timeout(Duration::from_millis(250)).is_ok() {
                    break;
                }
                let Ok(definitions) = tools.dynamic_verb_definitions() else {
                    continue;
                };
                let route_registry = kernel.route_registry();
                for definition in &definitions {
                    let _ = route_registry.register(
                        definition.identity.clone(),
                        Arc::new(artist_kernel::ResourceUriValueExtractor),
                    );
                }
                let _ = kernel.reconcile_verbs(definitions);
            }
        });
        Self {
            stop: Some(stop),
            join: Some(join),
        }
    }
}

impl Drop for DynamicVerbCatalogWatcher {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn seed_ast_resource(root: &Path) -> Result<()> {
    let package = root.join("ast");
    std::fs::create_dir_all(package.join("src"))?;
    std::fs::create_dir_all(package.join("deps/resource"))?;
    std::fs::create_dir_all(package.join("deps/tool"))?;
    let manifest = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/Cargo.toml"
    ));
    let guest = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/src/lib.rs"
    ));
    let resource_md = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/resource.md"
    ));
    let resource_wit = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/resource.wit"
    ));
    let lock = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/Cargo.lock"
    ));
    let resource_dependency = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/deps/resource/world.wit"
    ));
    let tool_dependency = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/deps/tool/world.wit"
    ));
    for (relative, bytes) in [
        ("Cargo.toml", manifest.as_bytes()),
        ("Cargo.lock", lock.as_slice()),
        ("resource.md", resource_md.as_slice()),
        ("resource.wit", resource_wit.as_slice()),
        ("src/lib.rs", guest.as_bytes()),
        ("deps/resource/world.wit", resource_dependency.as_slice()),
        ("deps/tool/world.wit", tool_dependency.as_slice()),
    ] {
        let path = package.join(relative);
        if !path.exists() {
            std::fs::write(path, bytes)?;
        }
    }
    Ok(())
}

fn seed_universal_tools(root: &Path) -> Result<()> {
    let source_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../artist-component/conformance/verbs");
    let guest = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/typed-guest/src/lib.rs"
    ));
    for entry in std::fs::read_dir(&source_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || !entry.path().join("Cargo.toml").is_file() {
            continue;
        }
        let name = entry.file_name();
        let package = root.join(&name);
        std::fs::create_dir_all(package.join("src"))?;
        std::fs::create_dir_all(package.join("typed-guest/src"))?;
        std::fs::create_dir_all(package.join("deps/resource"))?;
        let manifest = std::fs::read_to_string(entry.path().join("Cargo.toml"))?.replace(
            "path = \"../../typed-guest/src/lib.rs\"",
            "path = \"src/lib.rs\"",
        );
        let source = std::fs::read_to_string(entry.path().join("src/lib.rs"))?.replace(
            "../../../typed-guest/src/lib.rs",
            "../typed-guest/src/lib.rs",
        );
        let tool_wit = std::fs::read(entry.path().join("tool.wit"))?;
        let dependency_wit = std::fs::read(entry.path().join("deps/resource/world.wit"))
            .or_else(|_| std::fs::read(source_root.join("read/deps/resource/world.wit")))?;
        let cargo_lock = std::fs::read(entry.path().join("Cargo.lock"))
            .or_else(|_| std::fs::read(source_root.join("read/Cargo.lock")))?;
        for (relative, bytes) in [
            ("Cargo.toml", manifest.as_bytes().to_vec()),
            ("Cargo.lock", cargo_lock),
            ("tool.md", std::fs::read(entry.path().join("tool.md"))?),
            ("src/lib.rs", source.into_bytes()),
            ("typed-guest/src/lib.rs", guest.as_bytes().to_vec()),
            ("tool.wit", tool_wit),
            ("deps/resource/world.wit", dependency_wit),
        ] {
            let path = package.join(relative);
            if !path.exists() {
                std::fs::write(path, bytes)?;
            }
        }
    }
    Ok(())
}

pub fn address(target: &str) -> Result<ResourceAddress> {
    if target.is_empty() {
        bail!("resource target cannot be empty");
    }
    ResourceUri::parse(target)
        .map(ResourceAddress::uri)
        .with_context(|| format!("parse resource URI or path {target:?}"))
}

#[derive(Debug, Serialize)]
pub struct CliResult {
    pub target: ResourceAddress,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<artist_kernel::KernelError>,
}

impl CliResult {
    fn success(target: ResourceAddress, value: Value) -> Self {
        Self {
            target,
            ok: true,
            value: Some(value),
            error: None,
        }
    }

    fn failure(target: ResourceAddress, error: artist_kernel::KernelError) -> Self {
        Self {
            target,
            ok: false,
            value: None,
            error: Some(error),
        }
    }
}

pub async fn dispatch(kernel: &Kernel, verb: Verb, target: &str, args: &str) -> Result<CliResult> {
    let args = serde_json::from_str::<Value>(args)
        .with_context(|| format!("parse resource arguments as JSON: {args:?}"))?;
    let uri = address(target)?
        .as_uri()
        .cloned()
        .ok_or_else(|| anyhow!("typed resource operations require a URI target: {target:?}"))?;
    let mut input = dynamic_input(verb, &args)?;
    let identity = if matches!(verb, Verb::Run) && uri.scheme() == "file" {
        let executable = uri
            .as_ref()
            .to_file_path()
            .map_err(|_| anyhow!("run target is not a local executable path"))?;
        if let DynamicValue::Record(fields) = &mut input {
            fields.insert(
                "executable".to_owned(),
                DynamicValue::String(executable.to_string_lossy().into_owned()),
            );
            fields.insert("target".to_owned(), DynamicValue::ResourceUri(uri.clone()));
            fields.insert("cwd".to_owned(), DynamicValue::Option(None));
            fields.insert("environment".to_owned(), DynamicValue::List(Vec::new()));
        }
        native_verb("process", "run")
    } else {
        native_verb(dynamic_namespace(&uri), dynamic_function(verb))
    };
    let result = kernel
        .invoke_dynamic_resource(identity, uri.clone(), input)
        .await;
    Ok(dynamic_cli_result(ResourceAddress::uri(uri), verb, result))
}

fn dynamic_namespace(uri: &ResourceUri) -> &'static str {
    match uri.scheme() {
        "session" => "session",
        "process" => "process",
        "repo" => "repository",
        "resources" => "resources",
        _ => "filesystem",
    }
}

fn dynamic_function(verb: Verb) -> &'static str {
    match verb {
        Verb::Read => "read",
        Verb::Write => "write",
        Verb::Edit => "edit",
        Verb::Insert => "insert",
        Verb::Run => "run",
        Verb::Poll => "poll",
        Verb::Abort => "abort",
        Verb::Delete => "delete",
        Verb::Find => "find",
        Verb::Grep => "grep",
    }
}

fn dynamic_input(verb: Verb, args: &Value) -> Result<DynamicValue> {
    let object = args.as_object().cloned().unwrap_or_default();
    let string = |name: &str| -> Result<DynamicValue> {
        Ok(DynamicValue::String(
            object
                .get(name)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ))
    };
    let option_u32 = |name: &str| -> Result<DynamicValue> {
        Ok(DynamicValue::Option(
            object
                .get(name)
                .and_then(Value::as_u64)
                .map(|value| Box::new(DynamicValue::U32(value as u32))),
        ))
    };
    let option_value = |name: &str| -> Result<DynamicValue> {
        match object.get(name) {
            None | Some(Value::Null) => Ok(DynamicValue::Option(None)),
            Some(value) => Ok(DynamicValue::Option(Some(Box::new(json_dynamic(value)?)))),
        }
    };
    match verb {
        Verb::Read => Ok(DynamicValue::Record(BTreeMap::from([
            ("at".to_owned(), option_value("at")?),
            ("before".to_owned(), option_u32("before")?),
            ("after".to_owned(), option_u32("after")?),
        ]))),
        Verb::Write => Ok(DynamicValue::Record(BTreeMap::from([(
            "content".to_owned(),
            object
                .get("content")
                .or_else(|| object.get("value"))
                .and_then(Value::as_str)
                .map(|value| DynamicValue::String(value.to_owned()))
                .unwrap_or(DynamicValue::String(String::new())),
        )]))),
        Verb::Find => Ok(DynamicValue::Record(BTreeMap::from([(
            "query".to_owned(),
            string("query")?,
        )]))),
        Verb::Grep => Ok(DynamicValue::Record(BTreeMap::from([(
            "pattern".to_owned(),
            string("pattern")?,
        )]))),
        Verb::Edit => Ok(DynamicValue::Record(BTreeMap::from([
            ("start".to_owned(), string("start")?),
            ("end".to_owned(), option_value("end")?),
            ("content".to_owned(), string("content")?),
        ]))),
        Verb::Insert => Ok(DynamicValue::Record(BTreeMap::from([
            (
                "at".to_owned(),
                match object.get("at").and_then(Value::as_str) {
                    Some("top") => DynamicValue::Variant("top".to_owned(), None),
                    Some("bottom") => DynamicValue::Variant("bottom".to_owned(), None),
                    Some(anchor) => DynamicValue::Variant(
                        "at".to_owned(),
                        Some(Box::new(DynamicValue::String(anchor.to_owned()))),
                    ),
                    None => {
                        return Err(anyhow!(
                            "insert requires an at position (top, bottom, or anchor)"
                        ));
                    }
                },
            ),
            ("content".to_owned(), string("content")?),
        ]))),
        Verb::Run => Ok(DynamicValue::Record(BTreeMap::from([(
            "args".to_owned(),
            DynamicValue::List(
                object
                    .get("args")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(|value| DynamicValue::String(value.to_owned()))
                    .collect(),
            ),
        )]))),
        Verb::Poll | Verb::Abort | Verb::Delete => Ok(DynamicValue::Record(BTreeMap::new())),
    }
}

fn json_dynamic(value: &Value) -> Result<DynamicValue> {
    Ok(match value {
        Value::Null => DynamicValue::Option(None),
        Value::Bool(value) => DynamicValue::Bool(*value),
        Value::Number(value) => DynamicValue::U64(
            value
                .as_u64()
                .ok_or_else(|| anyhow!("dynamic number must be a non-negative integer"))?,
        ),
        Value::String(value) => DynamicValue::String(value.clone()),
        Value::Array(values) => DynamicValue::List(
            values
                .iter()
                .map(json_dynamic)
                .collect::<Result<Vec<_>>>()?,
        ),
        Value::Object(fields) => DynamicValue::Record(
            fields
                .iter()
                .map(|(name, value)| Ok((name.clone(), json_dynamic(value)?)))
                .collect::<Result<BTreeMap<_, _>>>()?,
        ),
    })
}

fn dynamic_cli_result(
    target: ResourceAddress,
    verb: Verb,
    result: std::result::Result<artist_kernel::DynamicVerbResult, artist_kernel::KernelError>,
) -> CliResult {
    let output = match result {
        Ok(result) => result.output,
        Err(error) => return CliResult::failure(target, error),
    };
    if verb == Verb::Write && target.to_string().starts_with("session:") {
        return CliResult::success(
            target.clone(),
            serde_json::json!({"created": true, "uri": target.to_string(), "status": "running"}),
        );
    }
    if verb == Verb::Read {
        if let DynamicValue::Record(fields) = &output {
            if let Some(DynamicValue::List(lines)) = fields.get("lines") {
                let content = lines
                    .iter()
                    .filter_map(|line| match line {
                        DynamicValue::Record(fields) => {
                            let DynamicValue::String(text) = fields.get("text")? else {
                                return None;
                            };
                            let ending = match fields.get("ending") {
                                Some(DynamicValue::Enum(value)) if value == "crlf" => "\r\n",
                                Some(DynamicValue::Enum(value)) if value == "cr" => "\r",
                                Some(DynamicValue::Enum(value)) if value == "none" => "",
                                _ => "\n",
                            };
                            Some(format!("{text}{ending}"))
                        }
                        _ => None,
                    })
                    .collect::<String>();
                return CliResult::success(
                    target.clone(),
                    serde_json::json!({
                        "path": fields.get("uri").map(dynamic_json).unwrap_or_else(|| Value::String(target.to_string())),
                        "value": {"type": "text", "content": content}
                    }),
                );
            }
            if let Some(DynamicValue::List(entries)) = fields.get("entries") {
                return CliResult::success(
                    target.clone(),
                    serde_json::json!({
                        "path": fields.get("uri").map(dynamic_json).unwrap_or_else(|| Value::String(target.to_string())),
                        "value": {"type": "directory", "entries": entries.iter().map(dynamic_json).collect::<Vec<_>>()}
                    }),
                );
            }
        }
    }
    CliResult::success(target, dynamic_json(&output))
}

fn dynamic_json(value: &DynamicValue) -> Value {
    match value {
        DynamicValue::Bool(value) => Value::Bool(*value),
        DynamicValue::S8(value) => serde_json::json!(*value),
        DynamicValue::S16(value) => serde_json::json!(*value),
        DynamicValue::S32(value) => serde_json::json!(*value),
        DynamicValue::S64(value) => serde_json::json!(*value),
        DynamicValue::U8(value) => serde_json::json!(*value),
        DynamicValue::U16(value) => serde_json::json!(*value),
        DynamicValue::U32(value) => serde_json::json!(*value),
        DynamicValue::U64(value) => serde_json::json!(*value),
        DynamicValue::F32(value) => serde_json::json!(*value),
        DynamicValue::F64(value) => serde_json::json!(*value),
        DynamicValue::Char(value) => Value::String(value.to_string()),
        DynamicValue::String(value) | DynamicValue::Enum(value) => Value::String(value.clone()),
        DynamicValue::ResourceUri(value) => Value::String(value.to_string()),
        DynamicValue::List(values) | DynamicValue::Tuple(values) => {
            Value::Array(values.iter().map(dynamic_json).collect())
        }
        DynamicValue::Record(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), dynamic_json(value)))
                .collect(),
        ),
        DynamicValue::Option(None) => Value::Null,
        DynamicValue::Option(Some(value)) => dynamic_json(value),
        DynamicValue::Result(Ok(value)) => dynamic_json(value),
        DynamicValue::Result(Err(value)) => dynamic_json(value),
        DynamicValue::Variant(name, value) => {
            let mut object = serde_json::Map::new();
            object.insert(
                name.clone(),
                value.as_deref().map(dynamic_json).unwrap_or(Value::Null),
            );
            Value::Object(object)
        }
        DynamicValue::Flags(values) => {
            Value::Array(values.iter().cloned().map(Value::String).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn clean_project_seeds_and_activates_named_read_tool() {
        let root = tempdir().unwrap();
        let source = root.path().join("hello.txt");
        std::fs::write(&source, "hello\n").unwrap();
        let ast_source = root.path().join("main.rs");
        std::fs::write(&ast_source, "fn caller() { main(); }\nfn main() {}\n").unwrap();
        let kernel = build(root.path()).await.unwrap();
        assert_eq!(kernel.active_verbs().unwrap().len(), 20);
        let seeded = ToolsHandler::new(root.path().join("tools"), Vec::<String>::new())
            .unwrap()
            .registrations()
            .unwrap();
        assert!(seeded.iter().any(|tool| tool.tool_name() == "read"));
        assert!(root.path().join("tools/read/tool.wit").is_file());
        assert!(
            root.path()
                .join("tools/read/deps/resource/world.wit")
                .is_file()
        );
        let output = kernel
            .execute_tool(
                "read",
                json_dynamic(&serde_json::json!({
                    "requests": [{"uri": source.display().to_string(), "at": null, "before": null, "after": null}]
                })).unwrap(),
            )
            .await
            .unwrap();
        assert!(format!("{output:?}").contains("hello"));

        // A framework migration must not overwrite source that the user has
        // customized. The old seed text is intentionally still present in
        // this source so path-based repair would destroy it.
        let read_source = root.path().join("tools/read/src/lib.rs");
        let mut customized = std::fs::read_to_string(&read_source).unwrap();
        customized.push_str("\n// user customization: preserve me\n");
        std::fs::write(&read_source, customized).unwrap();
        let _ = build(root.path()).await.unwrap();
        assert!(
            std::fs::read_to_string(&read_source)
                .unwrap()
                .contains("user customization: preserve me")
        );

        let docs = kernel
            .execute_tool(
                "read",
                json_dynamic(&serde_json::json!({
                    "requests": [{"uri": "resources://ast/resource.md", "at": null, "before": null, "after": null}]
                })).unwrap(),
            )
            .await
            .unwrap();
        assert!(format!("{docs:?}").contains("artist-ast"));
        assert!(root.path().join("resources/ast/resource.wit").is_file());

        // Native repository projections retain ownership of production AST
        // semantics; the seeded extension remains available as a package.
        let projection = format!("file://{}/symbols/", ast_source.display());
        let symbols = kernel
            .execute_tool(
                "read",
                json_dynamic(&serde_json::json!({
                    "requests": [{"uri": projection, "at": null, "before": null, "after": null}]
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        assert!(format!("{symbols:?}").contains("main"));
        let callers = kernel
            .execute_tool(
                "read",
                json_dynamic(&serde_json::json!({
                    "requests": [{"uri": format!("file://{}/symbols/main/callers", ast_source.display()), "at": null, "before": null, "after": null}]
                })).unwrap(),
            )
            .await
            .unwrap();
        assert!(format!("{callers:?}").contains("caller"));
    }

    #[test]
    fn canonicalizes_native_paths_before_kernel_dispatch() {
        assert_eq!(
            address("src/main.rs").unwrap().as_uri().unwrap().scheme(),
            "file"
        );
        assert_eq!(
            address("session://local/one")
                .unwrap()
                .as_uri()
                .unwrap()
                .scheme(),
            "session"
        );
    }

    #[tokio::test]
    async fn dispatches_filesystem_requests_through_the_registry() {
        let root = tempdir().unwrap();
        std::fs::write(root.path().join("note.txt"), "kernel path\n").unwrap();
        let kernel = build(root.path()).await.unwrap();
        let result = dispatch(
            &kernel,
            Verb::Read,
            &root.path().join("note.txt").display().to_string(),
            "null",
        )
        .await
        .unwrap();
        assert!(result.ok, "filesystem dispatch failed: {result:?}");
        assert_eq!(result.value.unwrap()["value"]["content"], "kernel path\n");
    }

    #[tokio::test]
    async fn dispatches_virtual_session_creation_through_the_same_registry() {
        let root = tempdir().unwrap();
        let kernel = build(root.path()).await.unwrap();
        let result = dispatch(&kernel, Verb::Write, "session://local/one", "null")
            .await
            .unwrap();
        assert!(result.ok);
        assert_eq!(result.value.unwrap()["status"], "running");
    }
}
