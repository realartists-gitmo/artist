use artist_tools::{BashResult, BashStatus, BashTool, ToolBundle, Workspace};
use rig_core::tool::{IntoToolOutput, PortableTool};
use serde_json::json;

fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path(), state.path(), "test").unwrap();
    (root, state, workspace)
}

/// Call a tool and render its output as text.
///
/// Generic over the output type rather than pinned to `String`: `read` returns
/// `ToolOutput` so it can carry real image blocks, and every other built-in
/// still returns a plain string.
async fn call<T: PortableTool>(tool: &T, value: serde_json::Value) -> String
where
    T::Error: std::fmt::Debug,
{
    let args = serde_json::from_value(value).unwrap();
    let output = tool.call(args).await.unwrap().into_tool_output().unwrap();
    output.render().trim_matches('"').replace("\\n", "\n")
}

/// Poll a managed session until it leaves the running state, so tests read
/// output the same way the session host does instead of racing the child.
async fn wait_for(bash: &BashTool, id: &str) -> BashResult {
    for _ in 0..100 {
        let snapshot = bash.managed_snapshot(id).unwrap();
        if snapshot.status != BashStatus::Running {
            return snapshot;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("session {id} did not finish in time");
}

#[test]
fn edit_schema_uses_nullable_end_for_strict_tools() {
    let (_root, _state, workspace) = workspace(&[]);
    let tools = ToolBundle::new(workspace);
    let schema = tools.edit.parameters();
    let replacement = &schema["properties"]["replacements"]["items"];
    assert_eq!(replacement["required"], json!(["start", "end", "content"]));
    assert_eq!(
        replacement["properties"]["end"]["anyOf"],
        json!([{"type":"string"}, {"type":"null"}])
    );
}

#[tokio::test]
async fn read_returns_a_real_image_block() {
    use rig_core::completion::message::ToolResultContent;

    let (root, _state, workspace) = workspace(&[]);
    // A 1x1 PNG.
    let png: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0a, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];
    std::fs::write(root.path().join("shot.png"), png).unwrap();

    let tools = ToolBundle::new(workspace);
    let args = serde_json::from_value(json!({"path":"shot.png"})).unwrap();
    let output = tools.read.call(args).await.unwrap();

    let blocks = output.as_content();
    assert_eq!(blocks.len(), 1);
    let ToolResultContent::Image(image) = blocks.first_ref() else {
        panic!("read must return a real image block, not a description: {blocks:?}");
    };
    assert_eq!(
        image.media_type,
        Some(rig_core::completion::message::ImageMediaType::PNG)
    );
}

#[tokio::test]
async fn read_declines_to_inline_a_format_no_model_accepts() {
    let (root, _state, workspace) = workspace(&[]);
    std::fs::write(root.path().join("old.bmp"), b"BM not really a bitmap").unwrap();
    let tools = ToolBundle::new(workspace);
    let output = call(&tools.read, json!({"path":"old.bmp"})).await;
    assert!(
        output.contains("cannot be sent to a model"),
        "unsupported formats must say so plainly: {output}"
    );
}

#[tokio::test]
async fn reads_then_edits_with_semantic_anchor() {
    let (_root, _state, workspace) = workspace(&[("src/lib.rs", "fn alpha() {}\nfn beta() {}\n")]);
    let tools = ToolBundle::new(workspace);
    let read = call(&tools.read, json!({"path":"src/lib.rs"})).await;
    let anchor = read.lines().nth(1).unwrap().split_once(": ").unwrap().0;
    let edited = call(
        &tools.edit,
        json!({"path":"src/lib.rs","replacements":[{"start":anchor,"content":"fn renamed() {}"}]}),
    )
    .await;
    assert!(edited.contains("fn renamed"));
    let reread = call(&tools.read, json!({"path":"src/lib.rs"})).await;
    assert!(reread.contains("fn renamed() {}"));
}

#[tokio::test]
async fn writes_finds_and_greps_project_files() {
    let (_root, _state, workspace) = workspace(&[
        ("src/lib.rs", "pub fn needle() {}\n"),
        ("README.md", "hello\n"),
    ]);
    let tools = ToolBundle::new(workspace);
    assert!(
        call(&tools.find, json!({"query":"lib rs","glob":"**/*.rs"}))
            .await
            .contains("src/lib.rs")
    );
    assert!(call(
        &tools.grep,
        json!({"query":"needle","glob":"**/*.rs","context":1})
    )
    .await
    .contains("src/lib.rs:1"));
    assert!(call(
        &tools.find,
        json!({"query":"lib rs","path":".","glob":"**/*.rs"})
    )
    .await
    .contains("src/lib.rs"));
    assert!(call(
        &tools.grep,
        json!({"query":"needle","path":".","glob":"**/*.rs"})
    )
    .await
    .contains("src/lib.rs:1"));
    assert_eq!(
        call(
            &tools.grep,
            json!({"query":"needle","path":".","glob":"**/*.md"})
        )
        .await,
        "No matches found."
    );
    assert!(
        call(&tools.grep, json!({"query":"NEEDLE","case":"insensitive"}))
            .await
            .contains("src/lib.rs:1")
    );
    assert!(call(
        &tools.write,
        json!({"path":"src/new.rs","content":"new file\n"})
    )
    .await
    .contains("created"));
    assert_eq!(
        std::fs::read_to_string(_root.path().join("src/new.rs")).unwrap(),
        "new file\n"
    );
    assert!(call(&tools.find, json!({"query":"new rs"}))
        .await
        .contains("src/new.rs"));
    assert!(call(&tools.grep, json!({"query":"new file"}))
        .await
        .contains("src/new.rs"));
}

#[tokio::test]
async fn all_file_tools_accept_external_absolute_paths() {
    let (_root, _state, workspace) = workspace(&[]);
    let outside = tempfile::tempdir().unwrap();
    let existing = outside.path().join("existing.rs");
    std::fs::write(&existing, "fn external_needle() {}\n").unwrap();
    let existing = existing.to_string_lossy().into_owned();
    let tools = ToolBundle::new(workspace.clone());

    let read = call(&tools.read, json!({"path":existing})).await;
    let anchor = read.lines().next().unwrap().split_once(": ").unwrap().0;
    call(
        &tools.edit,
        json!({"path":existing,"replacements":[{"start":anchor,"content":"fn edited_external_needle() {}"}]}),
    )
    .await;
    assert!(std::fs::read_to_string(&existing)
        .unwrap()
        .contains("edited_external_needle"));

    let created = outside.path().join("created.txt");
    call(
        &tools.write,
        json!({"path":created,"content":"absolute write needle\n"}),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(&created).unwrap(),
        "absolute write needle\n"
    );

    let scope = outside.path().to_string_lossy();
    let found = call(
        &tools.find,
        json!({"query":"existing rs","path":scope,"glob":"**/*.rs"}),
    )
    .await;
    assert!(found.contains(&existing), "unexpected find output: {found}");
    let grepped = call(
        &tools.grep,
        json!({"query":"edited_external_needle","path":scope,"glob":"**/*.rs"}),
    )
    .await;
    assert!(
        grepped.contains(&existing),
        "unexpected grep output: {grepped}"
    );

    let bash = BashTool::new(workspace);
    let id = bash
        .managed_start("pwd; cat created.txt".into(), Some(scope.clone().into_owned()), None)
        .await
        .unwrap();
    let finished = wait_for(&bash, &id).await;
    assert!(finished.output.contains(&*scope), "{finished:?}");
    assert!(finished.output.contains("absolute write needle"), "{finished:?}");
}

#[tokio::test]
async fn stale_anchor_requires_a_fresh_read() {
    let (root, _state, workspace) = workspace(&[("file.rs", "one\ntwo\n")]);
    let tools = ToolBundle::new(workspace);
    let read = call(&tools.read, json!({"path":"file.rs"})).await;
    let anchor = read.lines().nth(1).unwrap().split_once(": ").unwrap().0;
    std::fs::write(root.path().join("file.rs"), "one\ntwo changed externally\n").unwrap();
    let args = serde_json::from_value(
        json!({"path":"file.rs","replacements":[{"start":anchor,"content":"changed"}]}),
    )
    .unwrap();
    assert!(tools.edit.call(args).await.is_err());
    let retry = serde_json::from_value(
        json!({"path":"file.rs","replacements":[{"start":anchor,"content":"changed"}]}),
    )
    .unwrap();
    assert!(tools.edit.call(retry).await.is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn edit_temp_symlink_cannot_escape_workspace() {
    use std::os::unix::fs::symlink;
    let (root, _state, workspace) = workspace(&[("file.rs", "one\n")]);
    let outside = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(outside.path(), "safe").unwrap();
    symlink(outside.path(), root.path().join("file.rs.tmp")).unwrap();
    let tools = ToolBundle::new(workspace);
    let read = call(&tools.read, json!({"path":"file.rs"})).await;
    let anchor = read.lines().next().unwrap().split_once(": ").unwrap().0;
    call(
        &tools.edit,
        json!({"path":"file.rs","replacements":[{"start":anchor,"content":"changed"}]}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "safe");
}

#[tokio::test]
async fn managed_sessions_run_from_root_and_read_input() {
    let (_root, _state, workspace) = workspace(&[("marker.txt", "ok")]);
    let bash = BashTool::new(workspace);

    let id = bash
        .managed_start("pwd; cat marker.txt".into(), None, None)
        .await
        .unwrap();
    let finished = wait_for(&bash, &id).await;
    assert!(finished.output.contains("ok"), "{finished:?}");
    assert!(finished.output.contains("status: "), "{finished:?}");
    let _ = bash.managed_abort(&id);

    let interactive = bash
        .managed_start("read line; echo got:$line".into(), None, None)
        .await
        .unwrap();
    bash.managed_send(&interactive, "hello\n").unwrap();
    let finished = wait_for(&bash, &interactive).await;
    assert_eq!(finished.status, BashStatus::Completed, "{finished:?}");
    assert!(finished.output.contains("got:hello"), "{finished:?}");
    let _ = bash.managed_abort(&interactive);

    let failed = bash
        .managed_start("exit 7".into(), None, None)
        .await
        .unwrap();
    let finished = wait_for(&bash, &failed).await;
    assert_eq!(finished.status, BashStatus::Failed, "{finished:?}");
    assert_eq!(finished.exit_code, Some(7), "{finished:?}");
    let _ = bash.managed_abort(&failed);

    bash.run_input("cd /tmp").await.unwrap();
    let direct = bash.run_input("pwd").await.unwrap();
    assert!(direct.contains("/tmp"));
    assert!(!direct.contains("sessionId:"));
    assert!(!direct.contains("status:"));
    assert!(!direct.lines().any(|line| line.trim() == "pwd"));

    let typo = "artist_command_that_does_not_exist";
    let _ = bash.run_input(typo).await.unwrap();
    let following = bash.run_input("whoami").await.unwrap();
    assert!(!following.contains(typo));
    assert!(!following.contains("read>"));
}
