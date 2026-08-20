use std::fs;

use artist_cli::ToolRunner;
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

    let registered = ["edit", "find", "grep", "move", "read", "write"];
    for verb in registered {
        let (listed, _) = runner
            .call_toon(
                verb,
                match verb {
                    "read" => "uri: src/main.rs\nrange: -1..+0\n",
                    "find" | "grep" => "uri: src\nquery: main\nlimit: 1\n",
                    "write" => "uri: registered.txt\ncontent: registered\n",
                    "edit" => {
                        "uri: src/main.rs\nchanges[1]{anchor,content}:\n  line:0,fn main() {\n"
                    }
                    "move" => "source: missing\ndestination: missing2\n",
                    _ => unreachable!(),
                },
            )
            .await
            .unwrap();
        assert!(!listed.is_empty());
    }

    let (read, ok) = runner
        .call_toon("read", "uri: src/main.rs\nrange: -1..+2\n")
        .await
        .unwrap();
    assert!(ok);
    let read = decode(&read);
    assert_eq!(read["ok"], true);
    assert_eq!(read["response"]["uri"], "file:///src/main.rs");
    assert_eq!(read["response"]["lines"][0]["anchor"], "line:0");

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
    assert_eq!(matches[0]["uri"], "file:///src/main.rs#2");
    assert!(matches[0]["content"].as_str().unwrap().contains("println"));

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
    assert!(first_page["response"].get("next_cursor").is_some());

    let (written, ok) = runner
        .call_toon("write", "uri: created.txt\ncontent: \"first\"\n")
        .await
        .unwrap();
    assert!(ok);
    assert_eq!(decode(&written)["ok"], true);

    let (edited, ok) = runner
        .call_toon(
            "edit",
            "uri: src/main.rs\nchanges[1]{anchor,content}:\n  line:1,    println!(\"changed\");\n",
        )
        .await
        .unwrap();
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
    assert_eq!(decode(&rejected)["error"], "invalid_argument");

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
