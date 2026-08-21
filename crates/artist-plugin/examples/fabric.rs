use std::{path::PathBuf, process::Command};

use artist_plugin::PluginHost;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paths = std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    if paths.len() != 2 {
        return Err("usage: fabric <tool-fixture.wasm> <ast-fixture.wasm>".into());
    }
    let mut paths = paths.into_iter();
    let mut host = PluginHost::new()?;
    host.load(paths.next().unwrap())?;
    assert!(
        host.call_tool("read", r#"{"uri":"fixture://workspace/rust.rs"}"#)?
            .expect("read tool")
            .contains("fixture_symbol")
    );
    let fabric = host.mount_fabric()?;
    host.load(paths.next().unwrap())?;
    let projected = fabric.root().join("fixture/workspace/rust.rs");
    assert_eq!(
        std::fs::read_to_string(projected)?,
        "pub fn fixture_symbol() {}\nfn caller() { fixture_symbol(); }\n"
    );
    let unix_tree = Command::new("find").arg(fabric.root()).output()?;
    assert!(
        unix_tree.status.success(),
        "{}",
        String::from_utf8_lossy(&unix_tree.stderr)
    );
    let unix_tree = String::from_utf8(unix_tree.stdout)?;
    assert!(
        unix_tree.contains("rust.rs?symbols/fixture_symbol"),
        "{unix_tree}"
    );

    let base = "fixture://workspace/rust.rs";
    let all = host
        .call_tool("find", r#"{"uri":"fixture:///","limit":50}"#)?
        .expect("find tool");
    let indexed = fabric.search().indexed_resources()?;
    assert!(
        all.contains("fixture://workspace/rust.rs"),
        "FFF: {all}\nIndexed: {indexed:?}\nUnix: {unix_tree}"
    );
    let ordinary = host
        .call_tool(
            "grep",
            &serde_json::json!({"uri": base, "regex": "fixture_symbol", "limit": 5}).to_string(),
        )?
        .expect("grep tool");
    assert!(
        ordinary.contains("fixture://workspace/rust.rs"),
        "{ordinary}; indexed: {indexed:?}"
    );

    let projection = format!("{base}?symbols");
    let found = host
        .call_tool(
            "find",
            &serde_json::json!({"uri": projection, "max_depth": 1}).to_string(),
        )?
        .expect("find tool");
    assert!(found.contains("?symbols/"), "{found}");
    let projected = host
        .call_tool(
            "grep",
            &serde_json::json!({"uri": projection, "regex": "symbol", "limit": 5}).to_string(),
        )?
        .expect("grep tool");
    assert!(projected.contains("?symbols/"), "{projected}");

    host.call_tool(
        "write",
        r#"{"uri":"fixture://workspace/notes.txt","text":"refreshed fixture txt\n"}"#,
    )?
    .expect("write tool");
    let refreshed = host
        .call_tool(
            "grep",
            r#"{"uri":"fixture://workspace/","regex":"refreshed","limit":5}"#,
        )?
        .expect("grep tool");
    assert!(
        refreshed.contains("fixture://workspace/notes.txt"),
        "{refreshed}"
    );
    Ok(())
}
