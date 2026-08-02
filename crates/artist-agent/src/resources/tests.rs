use super::*;
use rig_core::tool::PortableTool;
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

/// The scan used to walk build output. A Rust `target/` holds tens of thousands
/// of entries, which consumed the whole traversal budget before the source tree
/// was reached — so a real instruction file went missing, and *which* files
/// survived depended on `read_dir` ordering rather than on the tree.
#[test]
fn build_output_does_not_crowd_out_real_instructions() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".gitignore"), "target\n");
    write(&root.path().join("crates/app/AGENTS.md"), "the real one");
    // Enough to have exhausted the old 5,000-entry budget on its own.
    for index in 0..6_000 {
        write(
            &root.path().join(format!("target/debug/deps/artifact{index}.rlib")),
            "",
        );
    }

    let mut diagnostics = Vec::new();
    let found = agents::nested(root.path(), &mut diagnostics);

    assert_eq!(found.len(), 1, "found: {found:?}");
    assert!(found[0].ends_with("crates/app/AGENTS.md"), "{found:?}");
}

/// A gitignored `AGENTS.md` is not a project instruction — it is scratch, or
/// vendored, or a fixture. Including it silently changed the system prompt.
#[test]
fn gitignored_instructions_are_skipped() {
    let root = tempfile::tempdir().unwrap();
    write(&root.path().join(".gitignore"), "vendor\n");
    write(&root.path().join("vendor/dep/AGENTS.md"), "not ours");
    write(&root.path().join("src/AGENTS.md"), "ours");

    let mut diagnostics = Vec::new();
    let found = agents::nested(root.path(), &mut diagnostics);

    assert_eq!(found.len(), 1, "found: {found:?}");
    assert!(found[0].ends_with("src/AGENTS.md"), "{found:?}");
}

/// Order feeds the system prompt, so it has to come from the tree rather than
/// from whatever order the filesystem returned entries in this time.
#[test]
fn the_order_is_the_trees_order() {
    let root = tempfile::tempdir().unwrap();
    for name in ["zebra", "alpha", "middle"] {
        write(&root.path().join(name).join("AGENTS.md"), name);
    }

    let mut diagnostics = Vec::new();
    let first = agents::nested(root.path(), &mut diagnostics);
    let second = agents::nested(root.path(), &mut diagnostics);

    assert_eq!(first, second);
    let names: Vec<_> = first
        .iter()
        .map(|path| path.parent().unwrap().file_name().unwrap().to_str().unwrap())
        .collect();
    assert_eq!(names, ["alpha", "middle", "zebra"]);
}

/// An instruction file too large to load used to fail in silence: the
/// diagnostic was collected and then never read by anything.
#[test]
fn a_failure_to_load_instructions_is_surfaced() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    write(&root.path().join("AGENTS.md"), &"x".repeat(200 * 1024));

    let resources = Resources::discover(root.path());
    assert!(
        resources
            .diagnostics()
            .iter()
            .any(|d| d.contains("exceeds")),
        "diagnostics: {:?}",
        resources.diagnostics()
    );
}
