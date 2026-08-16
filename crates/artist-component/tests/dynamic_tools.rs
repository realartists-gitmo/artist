use artist_component::tools::ToolsHandler;
use std::fs;

#[test]
fn unbuildable_tool_package_is_not_published() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("echo");
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(
        package.join("tool.md"),
        "---\nname: echo\ndescription: Echo text\nversion: 1.0.0\ncontract: example:tool:echo@1\ncapabilities: []\n---\nEchoes text.\n",
    )
    .unwrap();
    fs::write(
        package.join("tool.wit"),
        "package example:tool@1.0.0; interface echo { type error = string; echo: func(requests: list<string>) -> list<result<string, error>>; observe: func(response: result<string, error>) -> string; }",
    )
    .unwrap();
    fs::write(package.join("src/lib.rs"), "").unwrap();

    let handler = ToolsHandler::new(root.path(), Vec::<String>::new()).unwrap();
    let definitions = handler.dynamic_verb_definitions().unwrap();
    assert!(definitions.is_empty());
}
