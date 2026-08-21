use std::path::PathBuf;

use artist_plugin::{ContextFragment, HookEvent, Message, ModelConfig, PluginHost};
use artist_resource::{ResourceReply, ResourceRequest, ResourceUri};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut paths = std::env::args_os().skip(1);
    let path = paths
        .next()
        .map(PathBuf::from)
        .ok_or("usage: load <component.wasm>")?;
    let mut host = PluginHost::new()?;
    let descriptor = host.load(path)?;
    assert_eq!(descriptor.id.as_str(), "artist.default");

    let prompt = host.compose_prompt(vec![
        ContextFragment {
            source: "empty".into(),
            content: "  ".into(),
        },
        ContextFragment {
            source: "SYSTEM.md".into(),
            content: "system".into(),
        },
    ])?;
    assert_eq!(prompt.len(), 1);
    assert_eq!(host.tools()?.len(), 6);
    let messages = vec![Message {
        role: "user".into(),
        content: "hello".into(),
    }];
    let transformed = host.transform_context(messages)?;
    assert_eq!(transformed.len(), 1);
    assert_eq!(transformed[0].role, "user");
    assert_eq!(transformed[0].content, "hello");
    assert!(
        host.call_tool("read", r#"{"uri":"arch.md"}"#)?
            .unwrap()
            .contains("General purpose agentic harness")
    );
    let temp = tempfile::tempdir()?;
    let source = temp.path().join("source.txt");
    let moved = temp.path().join("moved.txt");
    host.call_tool(
        "write",
        &serde_json::json!({"uri": source, "text": "component smoke\n"}).to_string(),
    )?
    .expect("write tool");
    host.call_tool(
        "move",
        &serde_json::json!({"from": source, "to": moved}).to_string(),
    )?
    .expect("move tool");
    let root = ResourceUri::resolve(&temp.path().to_string_lossy(), std::path::Path::new("/"))?;
    assert!(matches!(
        host.handle_resource(ResourceRequest::Children { uri: root })?,
        ResourceReply::Children { children } if children.len() == 1
    ));
    host.call_tool(
        "move",
        &serde_json::json!({"from": moved, "to": null}).to_string(),
    )?
    .expect("remove tool");
    assert_eq!(
        host.observe_hook(&HookEvent {
            kind: "run.started".into(),
            payload: "{}".into()
        })?
        .len(),
        1
    );
    let config = ModelConfig {
        provider: "example".into(),
        model: "streaming-model".into(),
        parameters: "{}".into(),
    };
    let configured = host.configure_model(config)?;
    assert_eq!(configured.provider, "example");
    assert_eq!(configured.model, "streaming-model");
    assert_eq!(configured.parameters, "{}");
    host.observe_event(r#"{"event":"run.completed"}"#)?;
    if let Some(tool_fixture) = paths.next() {
        host.load(tool_fixture)?;
        assert_eq!(
            host.call_tool("fixture-cross", r#"{"value":42}"#)?,
            None,
            "cross-caller is registered by the AST fixture loaded next"
        );
    }
    if let Some(ast_fixture) = paths.next() {
        host.load(ast_fixture)?;
        assert_eq!(
            host.call_tool("fixture-cross", r#"{"value":42}"#)?,
            Some(r#"{"value":42}"#.into())
        );
        let listed: usize = serde_json::from_str(
            &host
                .call_tool("fixture-list-tools", "{}")?
                .expect("fixture list-tools bridge"),
        )?;
        assert_eq!(listed, host.tools()?.len());
        for cyclic in ["fixture-direct-cycle", "fixture-same-a", "fixture-cycle-a"] {
            let error = host.call_tool(cyclic, "{}").unwrap_err().to_string();
            assert!(error.contains("recursive tool invocation"), "{error}");
        }
        let symbol = host
            .call_tool(
                "read",
                r#"{"uri":"crates/artist-resource/src/uri.rs?symbols/has_scheme"}"#,
            )?
            .expect("universal read tool");
        assert!(symbol.contains("has_scheme"));
        let callers = host
            .call_tool(
                "read",
                r#"{"uri":"crates/artist-resource/src/uri.rs?symbols/has_scheme/callers"}"#,
            )?
            .expect("projected callers resource");
        assert!(callers.contains("has_scheme"));

        let poll_error = host
            .call_tool("poll", r#"{"uri":"arch.md","timeout_ms":1}"#)
            .unwrap_err()
            .to_string();
        assert!(poll_error.contains("unsupported"), "{poll_error}");
    }
    Ok(())
}
