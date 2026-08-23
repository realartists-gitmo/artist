use std::path::PathBuf;

use artist_core::{ContextFragment as CoreContextFragment, InitialContext};
use artist_plugin::{ContextFragment, HookEvent, Message, ModelConfig, PluginHost};
use artist_resource::{ResourceReply, ResourceRequest, ResourceUri};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = std::env::args_os().skip(1);
    let path = paths.next().map(PathBuf::from).ok_or(
        "usage: load <default.wasm> <builtin-tools.wasm> [tool-fixture.wasm] [ast-fixture.wasm]",
    )?;
    let builtin_tools = paths
        .next()
        .map(PathBuf::from)
        .ok_or("missing builtin-tools component")?;
    let mut host = PluginHost::new().await?;
    let untouched = host
        .compose_prompt(vec![ContextFragment {
            source: "empty".into(),
            content: "  ".into(),
        }])
        .await?;
    assert_eq!(
        untouched.len(),
        1,
        "the host must not compose prompts itself"
    );
    let descriptor = host.load(path).await?;
    assert_eq!(descriptor.id.as_str(), "artist.default");

    let prompt = host
        .compose_prompt(vec![
            ContextFragment {
                source: "empty".into(),
                content: "  ".into(),
            },
            ContextFragment {
                source: "SYSTEM.md".into(),
                content: "system".into(),
            },
        ])
        .await?;
    assert_eq!(prompt.len(), 1);
    let composed = host
        .compose_initial_context(InitialContext {
            fragments: vec![
                CoreContextFragment {
                    source: "empty".into(),
                    content: " ".into(),
                },
                CoreContextFragment {
                    source: "SYSTEM.md".into(),
                    content: "system".into(),
                },
            ],
        })
        .await?;
    assert_eq!(composed.fragments.len(), 1);
    assert_eq!(composed.fragments[0].source, "SYSTEM.md");
    assert!(host.tools().await?.is_empty());
    let tools_descriptor = host.load(builtin_tools).await?;
    assert_eq!(tools_descriptor.id.as_str(), "artist.builtin.tools");
    assert_eq!(host.tools().await?.len(), 6);
    for name in ["read", "find", "grep", "write", "move", "poll"] {
        assert_eq!(
            host.registry().owner(name).as_deref(),
            Some("artist.builtin.tools")
        );
    }
    let messages = vec![Message {
        role: "user".into(),
        content: "hello".into(),
    }];
    let transformed = host.transform_context(messages).await?;
    assert_eq!(transformed.len(), 1);
    assert_eq!(transformed[0].role, "user");
    assert_eq!(transformed[0].content, "hello");
    assert!(
        host.call_tool("read", r#"{"uri":"arch.md"}"#)
            .await?
            .unwrap()
            .contains("General purpose agentic harness")
    );
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    let moved = temp.path().join("moved.txt");
    host.call_tool(
        "write",
        &serde_json::json!({"uri": source, "text": "component smoke\n"}).to_string(),
    )
    .await?
    .expect("write tool");
    host.call_tool(
        "move",
        &serde_json::json!({"from": source, "to": moved}).to_string(),
    )
    .await?
    .expect("move tool");
    let root = ResourceUri::resolve(&temp.path().to_string_lossy(), std::path::Path::new("/"))?;
    assert!(matches!(
        host.handle_resource(ResourceRequest::Children { uri: root }).await?,
        ResourceReply::Children { children } if children.len() == 1
    ));
    host.call_tool(
        "move",
        &serde_json::json!({"from": moved, "to": null}).to_string(),
    )
    .await?
    .expect("remove tool");
    assert_eq!(
        host.observe_hook(&HookEvent {
            kind: "run.started".into(),
            payload: "{}".into()
        })
        .await?
        .len(),
        1
    );
    let config = ModelConfig {
        provider: "example".into(),
        model: "streaming-model".into(),
        parameters: "{}".into(),
    };
    let configured = host.configure_model(config).await?;
    assert_eq!(configured.provider, "example");
    assert_eq!(configured.model, "streaming-model");
    assert_eq!(configured.parameters, "{}");
    host.observe_event(r#"{"event":"run.completed"}"#).await?;
    if let Some(tool_fixture) = paths.next() {
        host.load(tool_fixture).await?;
        assert_eq!(
            host.call_tool("fixture-cross", r#"{"value":42}"#).await?,
            None,
            "cross-caller is registered by the AST fixture loaded next"
        );
    }
    if let Some(ast_fixture) = paths.next() {
        host.load(ast_fixture).await?;
        // The two narrow fixtures deliberately fail every lifecycle socket
        // they do not advertise. These calls prove capability discovery keeps
        // them out of the corresponding component chains.
        assert_eq!(
            host.compose_prompt(vec![ContextFragment {
                source: "SYSTEM.md".into(),
                content: "still component-composed".into(),
            }])
            .await?
            .len(),
            1
        );
        assert_eq!(
            host.transform_context(vec![Message {
                role: "user".into(),
                content: "still default-owned".into(),
            }])
            .await?
            .len(),
            1
        );
        assert_eq!(
            host.call_tool("fixture-cross", r#"{"value":42}"#).await?,
            Some(r#"{"value":42}"#.into())
        );
        let listed: usize = serde_json::from_str(
            &host
                .call_tool("fixture-list-tools", "{}")
                .await?
                .expect("fixture list-tools bridge"),
        )?;
        assert_eq!(listed, host.tools().await?.len());
        for cyclic in ["fixture-direct-cycle", "fixture-same-a", "fixture-cycle-a"] {
            let error = host.call_tool(cyclic, "{}").await.unwrap_err().to_string();
            assert!(error.contains("recursive tool invocation"), "{error}");
        }
        let symbol = host
            .call_tool(
                "read",
                r#"{"uri":"crates/artist-resource/src/uri.rs?symbols/has_scheme"}"#,
            )
            .await?
            .expect("universal read tool");
        assert!(symbol.contains("has_scheme"));
        let callers = host
            .call_tool(
                "read",
                r#"{"uri":"crates/artist-resource/src/uri.rs?symbols/has_scheme/callers"}"#,
            )
            .await?
            .expect("projected callers resource");
        assert!(callers.contains("has_scheme"));

        let poll_error = host
            .call_tool("poll", r#"{"uri":"arch.md","timeout_ms":1}"#)
            .await
            .unwrap_err()
            .to_string();
        assert!(poll_error.contains("unsupported"), "{poll_error}");
    }
    Ok(())
}
