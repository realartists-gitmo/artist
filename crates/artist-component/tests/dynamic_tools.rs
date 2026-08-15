use artist_component::tools::ToolsHandler;
use artist_kernel::DynamicType;
use std::fs;

#[test]
fn discovered_tool_package_projects_to_a_wit_typed_verb_definition() {
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
    assert_eq!(definitions.len(), 1);
    assert_eq!(
        definitions[0].identity.to_string(),
        "example:tool/echo@1.0.0"
    );
    assert_eq!(definitions[0].input_type, Some(DynamicType::String));
    assert_eq!(
        definitions[0].output_type,
        Some(DynamicType::Result {
            ok: Some(Box::new(DynamicType::String)),
            err: Some(Box::new(DynamicType::String)),
        })
    );
    assert_eq!(definitions[0].extractor.as_deref(), Some("tool-wit"));
    assert_eq!(definitions[0].schema.as_deref(), Some("tool-frontmatter"));
}
