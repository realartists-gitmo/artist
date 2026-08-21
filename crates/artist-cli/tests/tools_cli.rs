use std::fs;

use artist_cli::ToolRunner;
use artist_component::CompositionInput;
use serde_json::Value;
use tempfile::tempdir;

fn decode(payload: &str) -> Value {
    toon_format::decode_default(payload).unwrap()
}

#[tokio::test]
async fn cli_runner_executes_read_find_and_move_as_toon() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    fs::write(
        dir.path().join("src/main.rs"),
        "fn main() {\n    println!(\"hello\");\n}\n",
    )
    .unwrap();
    fs::write(dir.path().join("src/nested/lib.rs"), "pub fn helper() {}\n").unwrap();
    fs::write(dir.path().join("old.txt"), "move me\n").unwrap();

    let runner = ToolRunner::new(dir.path()).await.unwrap();

    let planner = runner.load_profile("planner").unwrap();
    assert_eq!(planner.name, "planner");
    assert!(planner.prompt.contains("broad requests"));

    let profile_uri: artist_kernel::ResourceUri = "profile://planner/PROFILE.md".parse().unwrap();
    let profile = runner
        .component_host()
        .kernel()
        .read_uri(&profile_uri, 0, u32::MAX)
        .await
        .unwrap();
    assert!(
        String::from_utf8(profile)
            .unwrap()
            .contains("Planner profile")
    );

    let packaged = runner
        .component_host()
        .compose_initial(CompositionInput {
            identity: "tester".into(),
            profile: None,
            profile_resource: None,
            system: None,
            agent_instructions: None,
            profile_content: None,
            visible_resources: Vec::new(),
            tool_events: Vec::new(),
        })
        .await
        .unwrap();
    assert!(
        packaged
            .contributions
            .iter()
            .find(|contribution| contribution.id == "system")
            .is_some_and(|contribution| contribution.content.contains("agentic coding harness"))
    );

    let snapshot = runner
        .component_host()
        .compose_initial(CompositionInput {
            identity: "tester".into(),
            profile: None,
            profile_resource: None,
            system: Some("system".into()),
            agent_instructions: Some("instructions".into()),
            profile_content: None,
            visible_resources: Vec::new(),
            tool_events: vec!["read: read a file".into(), "find: search resources".into()],
        })
        .await
        .unwrap();
    assert_eq!(
        snapshot.contributions.last().unwrap().content,
        "You are tester."
    );
    let tools = snapshot
        .contributions
        .iter()
        .find(|contribution| contribution.id == "tools")
        .expect("default prompt composition includes tool summary");
    assert!(tools.content.contains("read: read a file"));

    let (initial_read, ok) = runner
        .call_toon("read", "uri: src/main.rs\nrange: -1..+2\n")
        .await
        .unwrap();
    assert!(ok);
    let initial_read = decode(&initial_read);
    let first_anchor = initial_read["response"]["lines"][0]["anchor"]
        .as_str()
        .unwrap()
        .to_owned();

    let registered = ["find", "grep", "move", "read", "write"];
    for verb in registered {
        let (listed, _) = runner
            .call_toon(
                verb,
                match verb {
                    "read" => "uri: src/main.rs\nrange: -1..+0\n",
                    "find" | "grep" => "uri: src\nquery: main\nlimit: 1\n",
                    "write" => "uri: registered.txt\ncontent: registered\n",
                    "move" => "source: missing\ndestination: missing2\n",
                    _ => unreachable!(),
                },
            )
            .await
            .unwrap();
        assert!(!listed.is_empty());
    }

    assert_eq!(initial_read["ok"], true);
    assert_eq!(initial_read["response"]["uri"], "file:///src/main.rs");
    assert!(
        initial_read["response"]["lines"][0]["anchor"]
            .as_str()
            .unwrap()
            .starts_with("teca:v1:")
    );

    let anchored_uri = format!("uri: \"src/main.rs#{first_anchor}\"\nrange: -1..+1\n");
    let (anchored, ok) = runner.call_toon("read", &anchored_uri).await.unwrap();
    assert!(ok);
    let anchored = decode(&anchored);
    assert_eq!(anchored["response"]["position"], first_anchor);
    assert!(
        anchored["response"]["lines"][0]["anchor"]
            .as_str()
            .unwrap()
            .starts_with("teca:v1:")
    );

    let (find, ok) = runner
        .call_toon("find", "uri: src\nquery: \"**/*.rs\"\nlimit: 20\n")
        .await
        .unwrap();
    assert!(ok);
    let find = decode(&find);
    let results = find["response"]["results"].as_array().unwrap();
    assert!(results.len() <= 20);
    assert!(
        results
            .iter()
            .any(|result| { result["uri"] == "file:///src/main.rs" && result["kind"] == "file" })
    );
    assert!(results.iter().any(|result| {
        result["uri"] == "file:///src/nested/lib.rs" && result["kind"] == "file"
    }));

    let (grep, ok) = runner
        .call_toon("grep", "uri: src\nquery: println\nlimit: 10\n")
        .await
        .unwrap();
    assert!(ok);
    let grep = decode(&grep);
    let matches = grep["response"]["matches"].as_array().unwrap();
    assert_eq!(matches.len(), 1);
    assert!(
        matches[0]["uri"]
            .as_str()
            .unwrap()
            .starts_with("file:///src/main.rs#teca:v1:")
    );
    assert!(
        matches[0]["anchor"]
            .as_str()
            .unwrap()
            .starts_with("teca:v1:")
    );
    assert!(matches[0]["content"].as_str().unwrap().contains("println"));

    let (grep_page, ok) = runner
        .call_toon("grep", "uri: src\nquery: fn\nlimit: 1\n")
        .await
        .unwrap();
    assert!(ok);
    let grep_page = decode(&grep_page);
    assert_eq!(
        grep_page["response"]["matches"].as_array().unwrap().len(),
        1
    );
    assert_eq!(grep_page["response"]["truncated"], true);

    let (first_page, ok) = runner
        .call_toon("find", "uri: src\nquery: \"**/*.rs\"\nlimit: 1\n")
        .await
        .unwrap();
    assert!(ok);
    let first_page = decode(&first_page);
    assert_eq!(
        first_page["response"]["results"].as_array().unwrap().len(),
        1
    );
    assert_eq!(first_page["response"]["truncated"], true);

    let (written, ok) = runner
        .call_toon("write", "uri: created.txt\ncontent: \"first\"\n")
        .await
        .unwrap();
    assert!(ok);
    assert_eq!(decode(&written)["ok"], true);

    let edit_payload = toon_format::encode_default(&serde_json::json!({
        "uri": "src/main.rs",
        "changes": [{"anchor": first_anchor, "content": "fn changed() {"}]
    }))
    .unwrap();
    let (edited, ok) = runner.call_toon("edit", &edit_payload).await.unwrap();
    assert!(ok);
    assert_eq!(decode(&edited)["ok"], true);
    assert!(
        fs::read_to_string(dir.path().join("src/main.rs"))
            .unwrap()
            .contains("changed")
    );
    let (written, ok) = runner
        .call_toon("write", "uri: created.txt\ncontent: \"replacement\"\n")
        .await
        .unwrap();
    assert!(ok);
    assert_eq!(decode(&written)["ok"], true);
    assert_eq!(
        fs::read_to_string(dir.path().join("created.txt")).unwrap(),
        "replacement"
    );
    let (rejected, ok) = runner
        .call_toon("write", "uri: created.txt?kind\ncontent: nope\n")
        .await
        .unwrap();
    assert!(!ok);
    let rejected = decode(&rejected);
    let error = rejected["error"].as_str().expect("diagnostic error string");
    assert!(error.starts_with("invalid_argument: "));
    assert!(error.contains("invalid tool request"));

    let (moved, ok) = runner
        .call_toon("move", "source: old.txt\ndestination: renamed.txt\n")
        .await
        .unwrap();
    assert!(ok);
    assert_eq!(decode(&moved)["ok"], true);
    assert!(!dir.path().join("old.txt").exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("renamed.txt")).unwrap(),
        "move me\n"
    );
}
