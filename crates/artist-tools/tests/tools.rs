use artist_tools::{BashTool, ToolBundle, Workspace};
use rig_core::tool::Tool;
use serde_json::json;

fn workspace(files: &[(&str, &str)]) -> (tempfile::TempDir, tempfile::TempDir, Workspace) {
    let root = tempfile::tempdir().unwrap();
    for (path, content) in files {
        let target = root.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, content).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let workspace = Workspace::open(root.path(), state.path()).unwrap();
    (root, state, workspace)
}

async fn call<T: Tool<Output = String>>(tool: &T, value: serde_json::Value) -> String
where
    T::Error: std::fmt::Debug,
{
    let args = serde_json::from_value(value).unwrap();
    tool.call(args)
        .await
        .unwrap()
        .to_string()
        .trim_matches('"')
        .replace("\\n", "\n")
}

#[tokio::test]
async fn reads_then_edits_with_mnemonic_anchor() {
    let (_root, _state, workspace) = workspace(&[("src/lib.rs", "fn alpha() {}\nfn beta() {}\n")]);
    let tools = ToolBundle::new(workspace);
    let read = call(&tools.read, json!({"path":"src/lib.rs"})).await;
    let anchor = read
        .lines()
        .nth(1)
        .unwrap()
        .split('|')
        .next()
        .unwrap()
        .trim();
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
    assert!(
        call(
            &tools.grep,
            json!({"query":"needle","glob":"**/*.rs","context":1})
        )
        .await
        .contains("src/lib.rs:1")
    );
    assert!(
        call(&tools.grep, json!({"query":"NEEDLE","case":"insensitive"}))
            .await
            .contains("src/lib.rs:1")
    );
    assert!(
        call(
            &tools.write,
            json!({"path":"src/new.rs","content":"new file\n"})
        )
        .await
        .contains("created")
    );
    assert_eq!(
        std::fs::read_to_string(_root.path().join("src/new.rs")).unwrap(),
        "new file\n"
    );
    assert!(
        call(&tools.find, json!({"query":"new rs"}))
            .await
            .contains("src/new.rs")
    );
    assert!(
        call(&tools.grep, json!({"query":"new file"}))
            .await
            .contains("src/new.rs")
    );
}

#[tokio::test]
async fn stale_anchor_requires_a_fresh_read() {
    let (root, _state, workspace) = workspace(&[("file.rs", "one\ntwo\n")]);
    let tools = ToolBundle::new(workspace);
    let read = call(&tools.read, json!({"path":"file.rs"})).await;
    let anchor = read
        .lines()
        .nth(1)
        .unwrap()
        .split('|')
        .next()
        .unwrap()
        .trim();
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
    let anchor = read
        .lines()
        .next()
        .unwrap()
        .split('|')
        .next()
        .unwrap()
        .trim();
    call(
        &tools.edit,
        json!({"path":"file.rs","replacements":[{"start":anchor,"content":"changed"}]}),
    )
    .await;
    assert_eq!(std::fs::read_to_string(outside.path()).unwrap(), "safe");
}

#[tokio::test]
async fn bash_exec_and_persistent_session_work_from_root() {
    let (_root, _state, workspace) = workspace(&[("marker.txt", "ok")]);
    let bash = BashTool::new(workspace);
    let output = call(
        &bash,
        json!({"mode":"exec","command":"pwd; cat marker.txt"}),
    )
    .await;
    assert!(output.contains("ok"));
    let bounded = call(
        &bash,
        json!({"mode":"exec","command":"yes x | head -c 100000","maxBytes":128}),
    )
    .await;
    assert!(bounded.len() < 300);
    assert!(bounded.contains("truncated: true"));
    let started = call(&bash, json!({"mode":"start","command":"read line; echo got:$line","sessionId":"shell","waitMs":10})).await;
    assert!(started.contains("sessionId: shell"));
    let sent = call(
        &bash,
        json!({"mode":"send","sessionId":"shell","input":"hello\n","waitMs":500}),
    )
    .await;
    assert!(sent.contains("got:hello"));
}
