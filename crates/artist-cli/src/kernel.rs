use anyhow::{Context, Result, bail};
use artist_component::resources::ResourcesHandler;
use artist_component::tools::ToolsHandler;
use artist_component::watcher::SharedWatcher;
use artist_kernel::{
    FileHandler, Kernel, RepositoryHandler, Request, ResourceAddress, ResourceUri, SessionHandler,
};
use serde_json::Value;
use std::path::Path;

pub async fn build(root: &Path) -> Result<Kernel> {
    let kernel = Kernel::new();
    let file_handler = FileHandler::new(root)
        .with_context(|| format!("initialize filesystem handler at {}", root.display()))?;
    kernel.register_typed(FileHandler::new(root)?).await;
    kernel.register(file_handler).await;
    let session_handler = SessionHandler::new();
    kernel.register_typed(session_handler.clone()).await;
    kernel.register(session_handler).await;
    kernel
        .register_typed(
            RepositoryHandler::new(root)
                .with_context(|| format!("initialize repository handler at {}", root.display()))?,
        )
        .await;
    kernel
        .register(
            RepositoryHandler::new(root)
                .with_context(|| format!("initialize repository handler at {}", root.display()))?,
        )
        .await;
    let tools_root = root.join("tools");
    std::fs::create_dir_all(&tools_root)
        .with_context(|| format!("initialize tools root at {}", tools_root.display()))?;
    seed_universal_tools(&tools_root)?;
    let resources_root = root.join("resources");
    std::fs::create_dir_all(&resources_root)
        .with_context(|| format!("initialize resources root at {}", resources_root.display()))?;
    seed_ast_resource(&resources_root)?;
    let shared_watcher = SharedWatcher::new();
    let resources = ResourcesHandler::new_with_watcher(&resources_root, Some(&shared_watcher))?
        .without_file_package("artist-ast");
    kernel.register_typed_resource_handler(resources).await;
    let tool_capabilities = artist_kernel::Verb::ALL
        .iter()
        .map(|verb| format!("resource.{verb}"))
        .collect::<Vec<_>>();
    let tools =
        ToolsHandler::new_with_watcher(&tools_root, tool_capabilities, Some(&shared_watcher))?;
    let package_watcher = shared_watcher.start([&tools_root, &resources_root])?;
    kernel.retain_background(package_watcher);
    kernel.register_typed_tool_handler(tools).await;
    Ok(kernel)
}

fn seed_ast_resource(root: &Path) -> Result<()> {
    let package = root.join("ast");
    std::fs::create_dir_all(package.join("src"))?;
    std::fs::create_dir_all(root.join("wit/resource-surface"))?;
    let manifest = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/Cargo.toml"
    ));
    let guest = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/src/lib.rs"
    ))
    .replace(
        "../../../wit/resource-surface",
        "../../wit/resource-surface",
    );
    let resource_md = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/resource.md"
    ));
    let resource_wit = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/conformance/resources/ast/resource.wit"
    ));
    let shared_wit = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../artist-component/wit/resource-surface/world.wit"
    ));
    for (relative, bytes) in [
        ("Cargo.toml", manifest.as_bytes()),
        ("resource.md", resource_md.as_slice()),
        ("resource.wit", resource_wit.as_slice()),
        ("src/lib.rs", guest.as_bytes()),
        ("../wit/resource-surface/world.wit", shared_wit.as_slice()),
    ] {
        let path = package.join(relative);
        if !path.exists() {
            std::fs::write(path, bytes)?;
        }
    }
    Ok(())
}

fn seed_universal_tools(root: &Path) -> Result<()> {
    macro_rules! seed {
        ($verb:literal) => {{
            let package = root.join($verb);
            std::fs::create_dir_all(package.join("src"))?;
            std::fs::create_dir_all(root.join("wit/tool-surface-v1"))?;
            std::fs::create_dir_all(root.join("wit/tool-surface-v1/deps/resource"))?;
            std::fs::create_dir_all(root.join("wit/resource-surface"))?;
            let manifest = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../artist-component/conformance/verbs/",
                $verb,
                "/Cargo.toml"
            ))
            .replace(
                "path = \"../../typed-guest/src/lib.rs\"",
                "path = \"src/lib.rs\"",
            );
            let guest = include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../artist-component/conformance/typed-guest/src/lib.rs"
            ))
            .replace("../../../wit/tool-surface-v1", "../wit/tool-surface-v1")
            .replace("../../wit/tool-surface-v1", "../wit/tool-surface-v1")
            .replace("../../../wit/tool-surface", "../wit/tool-surface-v1")
            .replace("../../wit/tool-surface", "../wit/tool-surface-v1");
            let resource_wit = include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../artist-component/wit/resource-surface/world.wit"
            ));
            for (relative, bytes) in [
                ("Cargo.toml", manifest.as_bytes()),
                (
                    "Cargo.lock",
                    include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../artist-component/conformance/verbs/",
                        $verb,
                        "/Cargo.lock"
                    ))
                    .as_slice(),
                ),
                (
                    "tool.md",
                    include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../artist-component/conformance/verbs/",
                        $verb,
                        "/tool.md"
                    ))
                    .as_slice(),
                ),
                ("src/lib.rs", guest.as_bytes()),
                (
                    "../wit/tool-surface-v1/world.wit",
                    include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../artist-component/wit/tool-surface-v1/world.wit"
                    ))
                    .as_slice(),
                ),
                (
                    "../wit/tool-surface-v1/deps/resource/world.wit",
                    resource_wit.as_slice(),
                ),
                ("../wit/resource-surface/world.wit", resource_wit.as_slice()),
            ] {
                let path = package.join(relative);
                if !path.exists() {
                    std::fs::write(path, bytes)?;
                }
            }
        }};
    }
    seed!("read");
    seed!("write");
    seed!("edit");
    seed!("find");
    seed!("grep");
    seed!("run");
    seed!("send");
    seed!("abort");
    seed!("delete");
    seed!("poll");
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

pub async fn dispatch(
    kernel: &Kernel,
    verb: artist_kernel::Verb,
    target: &str,
    args: &str,
) -> Result<artist_kernel::ItemResult> {
    let args = serde_json::from_str::<Value>(args)
        .with_context(|| format!("parse resource arguments as JSON: {args:?}"))?;
    Ok(kernel
        .execute(Request::new(verb, address(target)?, args))
        .await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use artist_kernel::Verb;
    use tempfile::tempdir;

    #[tokio::test]
    async fn clean_project_seeds_and_activates_named_read_tool() {
        let root = tempdir().unwrap();
        let source = root.path().join("hello.txt");
        std::fs::write(&source, "hello\n").unwrap();
        let ast_source = root.path().join("main.rs");
        std::fs::write(&ast_source, "fn caller() { main(); }\nfn main() {}\n").unwrap();
        let kernel = build(root.path()).await.unwrap();
        let seeded = ToolsHandler::new(root.path().join("tools"), Vec::<String>::new())
            .unwrap()
            .registrations()
            .unwrap();
        assert!(seeded.iter().any(|tool| tool.tool_name() == "read"));
        assert!(
            root.path()
                .join("tools/wit/tool-surface-v1/deps/resource/world.wit")
                .is_file()
        );
        assert!(
            root.path()
                .join("tools/wit/resource-surface/world.wit")
                .is_file()
        );
        let output = kernel
            .execute_tool(
                "read",
                serde_json::json!({
                    "requests": [{"uri": source.display().to_string(), "at": null, "before": null, "after": null}]
                }),
            )
            .await
            .unwrap();
        assert!(output.to_string().contains("hello"));

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
                serde_json::json!({
                    "requests": [{"uri": "resources://ast/resource.md", "at": null, "before": null, "after": null}]
                }),
            )
            .await
            .unwrap();
        assert!(docs.to_string().contains("artist-ast"));
        assert!(root.path().join("resources/ast/resource.wit").is_file());

        // Native repository projections retain ownership of production AST
        // semantics; the seeded extension remains available as a package.
        let projection = format!("file://{}/symbols/", ast_source.display());
        let symbols = kernel
            .execute_tool(
                "read",
                serde_json::json!({
                    "requests": [{"uri": projection, "at": null, "before": null, "after": null}]
                }),
            )
            .await
            .unwrap();
        assert!(symbols.to_string().contains("main"));
        let callers = kernel
            .execute_tool(
                "read",
                serde_json::json!({
                    "requests": [{"uri": format!("file://{}/symbols/main/callers", ast_source.display()), "at": null, "before": null, "after": null}]
                }),
            )
            .await
            .unwrap();
        assert!(callers.to_string().contains("caller"));
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
