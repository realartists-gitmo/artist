use std::{
    collections::BTreeSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use artist_core::{
    ContextFragment as CoreContextFragment, ContextRole as CoreContextRole, InitialContext,
    ToolControl,
};
use artist_plugin::artist::plugin::types::{HookPhase, InvocationScope};
use artist_plugin::{
    ContextFragment, ContextRole, HookEvent, ModelConfig, ModelContext, ModelHistoryItem,
    PluginHost,
};
use artist_resource::{
    InvocationContext, ResourceError, ResourceOperation, ResourceProvider, ResourceReply,
    ResourceRequest, ResourceRoute, ResourceUri, TextReplacement, sha256,
};
use async_trait::async_trait;
use tokio::sync::Notify;

const COMPONENT_IDS: [&str; 30] = [
    "artist.prompt",
    "artist.context",
    "artist.hooks",
    "artist.model",
    "artist.events",
    "artist.file.read",
    "artist.file.children",
    "artist.file.write",
    "artist.file.edit",
    "artist.file.move",
    "artist.profiles.read",
    "artist.profiles.children",
    "artist.plugins.read",
    "artist.plugins.children",
    "artist.plugins.write",
    "artist.plugins.edit",
    "artist.plugins.move",
    "artist.plugins.signal",
    "artist.tool.read",
    "artist.tool.find",
    "artist.tool.grep",
    "artist.tool.write",
    "artist.tool.edit",
    "artist.tool.move",
    "artist.tool.poll",
    "artist.tool.run",
    "artist.tool.signal",
    "artist.tool.yield",
    "artist.tool.handoff",
    "artist.notes",
];

struct TwoCallBarrier {
    entered: AtomicUsize,
    release: Notify,
}

#[async_trait]
impl ResourceProvider for TwoCallBarrier {
    async fn handle(&self, request: ResourceRequest) -> Result<ResourceReply, ResourceError> {
        let entered = self.entered.fetch_add(1, Ordering::AcqRel) + 1;
        if entered >= 2 {
            self.release.notify_waiters();
        }
        while self.entered.load(Ordering::Acquire) < 2 {
            let notified = self.release.notified();
            if self.entered.load(Ordering::Acquire) >= 2 {
                break;
            }
            notified.await;
        }
        Ok(ResourceReply::Text {
            text: request.uri().to_string(),
        })
    }
}

async fn verify_concurrent_resource_instances(
    profiles: &std::path::Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let plugins = tempfile::tempdir()?;
    let package = plugins.path().join("concurrency");
    std::fs::create_dir_all(&package)?;
    std::fs::copy(
        "plugins/target/wasm32-wasip2/debug/artist_concurrent_resource_fixture.wasm",
        package.join("fixture.wasm"),
    )?;
    std::fs::write(
        package.join("plugin.json"),
        serde_json::json!({
            "format": 3,
            "id": "artist.test.concurrent-resource",
            "component": "fixture.wasm"
        })
        .to_string(),
    )?;
    let host = PluginHost::new_with_roots(profiles, plugins.path()).await?;
    host.packages().build("concurrency").await?;
    host.packages().activate("concurrency").await?;
    let router = host.router();
    assert_eq!(
        router.route_owner(
            &ResourceUri::resolve("concurrent:///child", std::path::Path::new("/"))?,
            ResourceOperation::Children,
        ),
        Some("artist.test.concurrent-resource".into())
    );
    assert_eq!(
        router.route_owner(
            &ResourceUri::resolve("parallel:///read", std::path::Path::new("/"))?,
            ResourceOperation::Read,
        ),
        Some("artist.test.concurrent-resource".into())
    );
    router
        .register(
            "test.barrier",
            ResourceRoute::new("barrier:///**", None::<String>, [ResourceOperation::Read]),
            Arc::new(TwoCallBarrier {
                entered: AtomicUsize::new(0),
                release: Notify::new(),
            }),
        )
        .await?;
    let request = |name: &str| ResourceRequest::Read {
        uri: ResourceUri::resolve(&format!("concurrent:///{name}"), std::path::Path::new("/"))
            .unwrap(),
        start_line: None,
        line_count: None,
    };
    let (first, second) = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::join!(
            router.handle(request("first")),
            router.handle(request("second"))
        )
    })
    .await?;
    let mut values = [first?, second?]
        .into_iter()
        .map(|reply| match reply {
            ResourceReply::Text { text } => text,
            other => panic!("unexpected concurrent reply: {other:?}"),
        })
        .collect::<Vec<_>>();
    values.sort();
    assert_eq!(values, ["1", "2"]);
    assert_eq!(
        router.handle(request("third")).await?,
        ResourceReply::Text { text: "3".into() }
    );
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let profiles = tempfile::tempdir()?;
    for name in ["planner", "worker"] {
        let directory = profiles.path().join(name);
        std::fs::create_dir_all(&directory)?;
        std::fs::write(
            directory.join("instructions.md"),
            format!("{name} profile instructions"),
        )?;
        std::fs::write(directory.join("profile.json"), "{}")?;
    }
    let empty_plugins = tempfile::tempdir()?;
    let empty_host = PluginHost::new_with_roots(profiles.path(), empty_plugins.path()).await?;
    let untouched = empty_host
        .compose_prompt(vec![ContextFragment {
            source: "empty".into(),
            content: "  ".into(),
            role: ContextRole::Other,
        }])
        .await?;
    assert_eq!(untouched.len(), 1, "the host must not shape prompts");

    verify_concurrent_resource_instances(profiles.path()).await?;

    let host = PluginHost::new_with_profiles(profiles.path()).await?;

    let packages = host.packages();
    for package in packages.package_names()? {
        if packages.active_component(&package).is_err() {
            packages.build(&package).await?;
            packages.activate(&package).await?;
        }
    }
    let mut loaded = BTreeSet::new();
    for descriptor in host.descriptors().await {
        assert_eq!(
            descriptor.capabilities.len(),
            1,
            "{} is not narrow",
            descriptor.id
        );
        assert!(loaded.insert(descriptor.id.to_string()));
    }
    assert_eq!(
        loaded,
        COMPONENT_IDS.into_iter().map(String::from).collect()
    );
    assert!(
        host.slash_commands().is_empty(),
        "no existing component may aggregate slash commands"
    );

    let prompt = host
        .compose_prompt(vec![
            ContextFragment {
                source: "empty".into(),
                content: "  ".into(),
                role: ContextRole::Other,
            },
            ContextFragment {
                source: "SYSTEM.md".into(),
                content: "system".into(),
                role: ContextRole::System,
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
                    role: CoreContextRole::Other,
                },
                CoreContextFragment {
                    source: "SYSTEM.md".into(),
                    content: "system".into(),
                    role: CoreContextRole::System,
                },
            ],
        })
        .await?;
    assert_eq!(composed.fragments.len(), 1);

    let definitions = host.tools().await?;
    assert_eq!(definitions.len(), 12);
    for definition in definitions {
        let schema: serde_json::Value = serde_json::from_str(&definition.input_schema)?;
        assert_eq!(
            schema["additionalProperties"], false,
            "{} accepts unknown fields",
            definition.name
        );
        assert!(
            !definition.effects.is_empty(),
            "{} has no effects",
            definition.name
        );
    }
    for name in [
        "read", "find", "grep", "write", "edit", "move", "poll", "run", "signal", "yield",
        "handoff",
    ] {
        assert_eq!(
            host.registry().owner(name),
            Some(format!("artist.tool.{name}"))
        );
    }
    let routes = host.router().routes();
    assert_eq!(routes.len(), 14);
    for (owner, route) in routes {
        assert!(
            owner.starts_with("artist.file.")
                || owner.starts_with("artist.profiles.")
                || owner.starts_with("artist.plugins.")
                || owner == "artist.host.storage",
            "unexpected route owner: {owner}"
        );
        assert_eq!(
            route.operations.len(),
            if owner == "artist.host.storage" { 4 } else { 1 },
            "{owner} aggregates operations"
        );
        if owner == "artist.plugins.signal" {
            assert_eq!(route.signals.len(), 3);
        } else {
            assert!(route.signals.is_empty());
        }
    }

    let planner = Arc::new(host.load_profile("planner").await?);
    assert_eq!(planner.catalog, ["planner", "worker"]);

    let plugin_source =
        ResourceUri::resolve("plugins:///tool-read/src/lib.rs", std::path::Path::new("/"))?;
    assert!(matches!(
        host.handle_resource(ResourceRequest::Read {
            uri: plugin_source,
            start_line: None,
            line_count: None,
        })
        .await?,
        ResourceReply::Text { text } if text.contains("struct ReadTool")
    ));
    assert!(matches!(
        host.handle_resource(ResourceRequest::Read {
            uri: ResourceUri::resolve(
                "plugins:///_abi/plugin.wit",
                std::path::Path::new("/"),
            )?,
            start_line: None,
            line_count: None,
        })
        .await?,
        ResourceReply::Text { text } if text.contains("package artist:plugin@0.8.0")
    ));
    assert!(matches!(
        host.handle_resource(ResourceRequest::Children {
            uri: ResourceUri::resolve("plugins:///", std::path::Path::new("/"))?,
        })
        .await?,
        ResourceReply::Children { children } if children.contains(&ResourceUri::resolve("plugins:///tool-read", std::path::Path::new("/"))?)
    ));
    let scratch = ResourceUri::resolve(
        "plugins:///tool-read/src/.package-smoke",
        std::path::Path::new("/"),
    )?;
    host.handle_resource(ResourceRequest::Write {
        uri: scratch.clone(),
        text: "before\n".into(),
    })
    .await?;
    host.handle_resource(ResourceRequest::Edit {
        uri: scratch.clone(),
        expected_sha256: sha256(b"before\n"),
        replacements: vec![TextReplacement {
            start_byte: 0,
            end_byte: 6,
            text: "after".into(),
        }],
    })
    .await?;
    let moved_scratch = ResourceUri::resolve(
        "plugins:///tool-read/src/.package-smoke-moved",
        std::path::Path::new("/"),
    )?;
    host.handle_resource(ResourceRequest::Move {
        from: scratch,
        to: Some(moved_scratch.clone()),
    })
    .await?;
    host.handle_resource(ResourceRequest::Move {
        from: moved_scratch,
        to: None,
    })
    .await?;
    host.handle_resource(ResourceRequest::Signal {
        uri: ResourceUri::resolve("plugins:///tool-read", std::path::Path::new("/"))?,
        name: "build-and-activate".into(),
        payload: None,
    })
    .await?;
    assert!(matches!(
        host.handle_resource(ResourceRequest::Read {
            uri: ResourceUri::resolve(
                "plugins:///tool-read/.artist/status.json",
                std::path::Path::new("/"),
            )?,
            start_line: None,
            line_count: None,
        })
        .await?,
        ResourceReply::Text { text } if text.contains("\"state\": \"active\"")
    ));
    let candidate_path = std::path::Path::new("plugins/tool-read/.artist/candidate.wasm");
    let valid_candidate = std::fs::read(candidate_path)?;
    let active_path = packages.active_component("tool-read")?;
    let active_before = sha256(&std::fs::read(&active_path)?);
    std::fs::write(candidate_path, b"not a component")?;
    assert!(packages.activate("tool-read").await.is_err());
    assert_eq!(sha256(&std::fs::read(&active_path)?), active_before);
    assert_eq!(
        host.registry().owner("read").as_deref(),
        Some("artist.tool.read")
    );
    std::fs::write(candidate_path, valid_candidate)?;
    packages.activate("tool-read").await?;
    let profiled_definitions = host.registry().definitions_for(&planner);
    let handoff = profiled_definitions
        .iter()
        .find(|definition| definition.name == "handoff")
        .expect("handoff definition");
    assert_eq!(
        handoff.input_schema["properties"]["profile"]["enum"],
        serde_json::json!(["planner", "worker"])
    );
    let yielded = host
        .registry()
        .call_output_with_context(
            "yield",
            serde_json::json!({"completed": true}),
            InvocationContext::for_profile(planner.clone()),
        )
        .await?;
    assert!(matches!(yielded.control, Some(ToolControl::Yield { .. })));
    let handed_off = host
        .registry()
        .call_output_with_context(
            "handoff",
            serde_json::json!({"profile": "worker", "brief": "continue"}),
            InvocationContext::for_profile(planner),
        )
        .await?;
    assert!(matches!(
        handed_off.control,
        Some(ToolControl::Handoff { ref profile, ref brief })
            if profile == "worker" && brief == "continue"
    ));
    let profile_uri = ResourceUri::resolve(
        "profiles:///planner/instructions.md",
        std::path::Path::new("/"),
    )?;
    assert!(matches!(
        host.handle_resource(ResourceRequest::Read {
            uri: profile_uri,
            start_line: None,
            line_count: None,
        })
        .await?,
        ResourceReply::Text { text } if text == "planner profile instructions"
    ));

    assert!(
        host.call_tool("read", r#"{"uri":"arch.md"}"#)
            .await?
            .expect("read tool")
            .contains("General purpose agentic harness")
    );
    assert!(
        host.call_tool("read", r#"{"uri":"arch.md","extra":true}"#)
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid tool arguments")
    );
    assert!(
        host.call_tool("grep", r#"{"uri":"."}"#)
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid tool arguments")
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
    let read = host
        .call_tool("read", &serde_json::json!({"uri": source}).to_string())
        .await?
        .expect("read tool");
    let read: serde_json::Value = serde_json::from_str(&read)?;
    let anchor = read["text"]
        .as_str()
        .and_then(|text| text.lines().next())
        .and_then(|line| line.split_once(": "))
        .map(|(anchor, _)| anchor)
        .expect("anchored read line");
    let edit = host
        .call_tool(
            "edit",
            &serde_json::json!({
                "uri": source,
                "operations": [{"kind": "replace", "start": anchor, "content": "edited"}]
            })
            .to_string(),
        )
        .await?
        .expect("edit tool");
    let edit: serde_json::Value = serde_json::from_str(&edit)?;
    assert!(
        edit["diff"]
            .as_str()
            .is_some_and(|diff| diff.contains("edited"))
    );
    assert!(
        edit["updated_lines"]
            .as_array()
            .is_some_and(|lines| lines.len() == 1 && lines[0]["anchor"].is_string())
    );
    assert_eq!(std::fs::read_to_string(&source)?, "edited\n");
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

    let context = ModelContext {
        context: "system".into(),
        prompt: vec![artist_plugin::ContentPart::Text("current".into())],
        history: vec![ModelHistoryItem {
            sequence: 0,
            message: serde_json::to_string(&artist_kernel::ModelMessage::User(vec![
                artist_core::ContentPart::text("hello"),
            ]))?,
        }],
    };
    assert_eq!(host.transform_context(context).await?.history.len(), 1);
    assert_eq!(
        host.observe_hook(&HookEvent {
            phase: HookPhase::BeforeModelRequest,
            scope: InvocationScope {
                session_id: "session".into(),
                run_id: Some("run".into()),
                call_id: None,
                correlation_id: "correlation".into(),
                parent_correlation_id: None,
            },
            payload: serde_json::json!({"context": "system", "history_items": 0}).to_string(),
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
    assert_eq!(host.configure_model(config).await?.provider, "example");
    host.observe_event(r#"{"event":"run.completed"}"#).await?;

    let poll_error = host
        .call_tool("poll", r#"{"uri":"arch.md","timeout_ms":1}"#)
        .await
        .unwrap_err()
        .to_string();
    assert!(poll_error.contains("unsupported"), "{poll_error}");
    for (name, arguments) in [
        ("run", r#"{"target":"file:///absent","input":""}"#),
        ("signal", r#"{"uri":"file:///absent","name":"stop"}"#),
    ] {
        let error = host
            .call_tool(name, arguments)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("unsupported"), "{error}");
    }

    // Durable domain state without provider-state: the notes tool persists a
    // document through the `store:///` resource route and emits a generic,
    // durable plugin event per add.
    let _ = host
        .handle_resource(ResourceRequest::Move {
            from: ResourceUri::resolve("store:///global/notes-state", std::path::Path::new("/"))?,
            to: None,
        })
        .await;
    for text in ["first note", "second note"] {
        let output = host
            .call_tool(
                "notes",
                &serde_json::json!({"op": "add", "value": text}).to_string(),
            )
            .await?
            .expect("notes add");
        assert!(output.contains("\"added\""), "{output}");
    }
    let listed = host
        .call_tool("notes", r#"{"op":"list"}"#)
        .await?
        .expect("notes list");
    assert!(listed.contains("first note"), "{listed}");
    assert!(listed.contains("second note"), "{listed}");
    let stored = match host
        .handle_resource(ResourceRequest::Read {
            uri: ResourceUri::resolve("store:///global/notes-state", std::path::Path::new("/"))?,
            start_line: None,
            line_count: None,
        })
        .await?
    {
        ResourceReply::Text { text } => text,
        other => panic!("unexpected notes reply: {other:?}"),
    };
    assert!(stored.contains("first note") && stored.contains("second note"));
    // The durable emissions landed in the legacy outbox keyed by the guest's
    // scope; in real sessions the kernel sink commits them synchronously.
    let facts = artist_kernel::ExecutionExtensions::drain_plugin_facts(
        &host,
        &artist_core::SessionId::from("unknown"),
    )
    .await;
    let emitted = facts.iter().any(|fact| {
        matches!(
            fact,
            artist_core::PluginFact::Event { event }
                if event.event_type == "artist.notes.added"
                    && event.payload.get("text").and_then(|v| v.as_str()) == Some("second note")
        )
    });
    assert!(emitted, "durable notes event missing: {facts:?}");

    let restored = PluginHost::new_with_roots(profiles.path(), "plugins").await?;
    let restored_descriptors = restored.descriptors().await;
    assert_eq!(restored_descriptors.len(), COMPONENT_IDS.len());
    assert_eq!(restored.registry().definitions().len(), 12);
    let survived = match restored
        .handle_resource(ResourceRequest::Read {
            uri: ResourceUri::resolve("store:///global/notes-state", std::path::Path::new("/"))?,
            start_line: None,
            line_count: None,
        })
        .await?
    {
        ResourceReply::Text { text } => text,
        other => panic!("unexpected restart reply: {other:?}"),
    };
    assert!(survived.contains("first note"), "state lost across restart");
    Ok(())
}
