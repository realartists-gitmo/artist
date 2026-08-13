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
    let tool_capabilities = artist_kernel::Verb::ALL
        .iter()
        .map(|verb| format!("resource.{verb}"))
        .collect::<Vec<_>>();
    kernel
        .register_tool_handler(ToolsHandler::new(&tools_root, tool_capabilities)?)
        .await;
    Ok(kernel)
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
        let result = dispatch(&kernel, Verb::Read, "note.txt", "null")
            .await
            .unwrap();
        assert!(result.ok);
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
