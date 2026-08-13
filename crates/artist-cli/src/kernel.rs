use anyhow::{Context, Result, bail};
use artist_component::tools::ToolsHandler;
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
    let tool_capabilities = artist_kernel::Verb::ALL
        .iter()
        .map(|verb| format!("resource.{verb}"))
        .collect::<Vec<_>>();
    kernel
        .register_typed(ToolsHandler::new(&tools_root, tool_capabilities.clone())?)
        .await;
    kernel
        .register_tool_handler(ToolsHandler::new(&tools_root, tool_capabilities)?)
        .await;
    Ok(kernel)
}

fn seed_universal_tools(root: &Path) -> Result<()> {
    macro_rules! seed {
        ($verb:literal) => {{
            let package = root.join($verb);
            std::fs::create_dir_all(package.join("src"))?;
            std::fs::create_dir_all(root.join("wit/tool-surface"))?;
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
            .replace("../../../wit/tool-surface", "../wit/tool-surface");
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
                    "../wit/tool-surface/world.wit",
                    include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/../artist-component/wit/tool-surface/world.wit"
                    ))
                    .as_slice(),
                ),
            ] {
                let path = package.join(relative);
                let needs_repair = match relative {
                    "Cargo.toml" => path
                        .exists()
                        .then(|| std::fs::read_to_string(&path).ok())
                        .flatten()
                        .is_some_and(|contents| contents.contains("../../typed-guest/src/lib.rs")),
                    "src/lib.rs" => path
                        .exists()
                        .then(|| std::fs::read_to_string(&path).ok())
                        .flatten()
                        .is_some_and(|contents| {
                            contents.contains("../../../wit/tool-surface")
                                || contents.contains("../../wit/tool-surface")
                        }),
                    _ => false,
                };
                if !path.exists() || needs_repair {
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
        let kernel = build(root.path()).await.unwrap();
        let seeded = ToolsHandler::new(root.path().join("tools"), Vec::<String>::new())
            .unwrap()
            .registrations()
            .unwrap();
        assert!(seeded.iter().any(|tool| tool.tool_name() == "read"));
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
