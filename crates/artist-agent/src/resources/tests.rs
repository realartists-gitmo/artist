use super::*;
use rig_core::tool::Tool;
use serde_json::json;

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

fn skill(path: &Path, name: &str, description: &str, body: &str) {
    write(
        &path.join("SKILL.md"),
        &format!("---\nname: {name}\ndescription: {description}\n---\n{body}"),
    );
}

#[test]
fn agents_load_global_then_broad_to_specific() {
    let config = tempfile::tempdir().unwrap();
    let repository = tempfile::tempdir().unwrap();
    std::fs::create_dir(repository.path().join(".git")).unwrap();
    let workspace = repository.path().join("packages/app");
    std::fs::create_dir_all(&workspace).unwrap();
    write(&config.path().join("AGENTS.md"), "global");
    write(&repository.path().join("AGENTS.md"), "root");
    write(&repository.path().join("packages/AGENTS.md"), "package");
    write(&workspace.join("src/AGENTS.md"), "nested");

    let mut diagnostics = Vec::new();
    let files = agents::discover_from(&workspace, Some(config.path()), &mut diagnostics);
    assert!(diagnostics.is_empty());
    assert_eq!(
        files
            .iter()
            .map(|file| file.content.as_str())
            .collect::<Vec<_>>(),
        ["global", "root", "package"]
    );
}

#[test]
fn nested_agents_are_injected_into_the_scoped_prompt() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join("frontend/AGENTS.md"), "frontend only");
    write(&root.path().join("backend/AGENTS.md"), "backend only");

    let resources = Resources::discover(root.path());
    let prompt = resources.prompt_section();

    assert!(prompt.contains("<scoped_project_instructions>"));
    assert!(prompt.contains("frontend only"));
    assert!(prompt.contains("backend only"));
    assert!(!prompt.contains("call the instructions tool"));
}

#[test]
fn embedded_skill_mentions_use_exact_boundaries() {
    assert!(mentions_skill("please use $linear for this", "linear"));
    assert!(mentions_skill("$linear, then continue", "linear"));
    assert!(!mentions_skill("use $linear-extra", "linear"));
}

#[test]
fn project_skills_override_user_and_malformed_skills_are_skipped() {
    let user = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    skill(
        &user.path().join("review"),
        "review",
        "user version",
        "user",
    );
    skill(
        &project.path().join("review"),
        "review",
        "project version",
        "project",
    );
    write(
        &project.path().join("broken/SKILL.md"),
        "---\nname: broken\n---\nmissing description",
    );
    let mut diagnostics = Vec::new();
    let skills = skills::discover_roots(
        vec![user.path().to_owned(), project.path().to_owned()],
        &mut diagnostics,
    );
    assert_eq!(skills["review"].description, "project version");
    assert!(!skills.contains_key("broken"));
    assert!(
        diagnostics
            .iter()
            .any(|line| line.contains("missing description"))
    );
}

#[tokio::test]
async fn skill_tool_activates_and_rejects_resource_traversal() {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().join("demo");
    skill(&base, "demo", "demo skill", "# Instructions\nDo the thing.");
    write(&base.join("references/info.md"), "reference");
    let mut diagnostics = Vec::new();
    let skills = skills::discover_roots(vec![root.path().to_owned()], &mut diagnostics);
    let resources = Resources(Arc::new(ResourceData {
        agents: Vec::new(),
        nested_agents: Vec::new(),
        skills,
        activated: std::sync::Mutex::new(std::collections::HashSet::new()),
        diagnostics,
    }));
    let tool = resources.skill_tool();
    let before = serde_json::from_value(
        json!({"mode":"readResource","name":"demo","path":"references/info.md"}),
    )
    .unwrap();
    assert!(tool.call(before).await.is_err());
    let activate = serde_json::from_value(json!({"mode":"activate","name":"demo"})).unwrap();
    let output = tool.call(activate).await.unwrap();
    assert!(output.contains("# Instructions"));
    let read = serde_json::from_value(
        json!({"mode":"readResource","name":"demo","path":"references/info.md"}),
    )
    .unwrap();
    assert_eq!(tool.call(read).await.unwrap(), "reference");
    let escape =
        serde_json::from_value(json!({"mode":"readResource","name":"demo","path":"../outside"}))
            .unwrap();
    assert!(tool.call(escape).await.is_err());
}
