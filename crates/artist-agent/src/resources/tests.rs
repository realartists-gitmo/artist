use super::*;

fn profile(root: &std::path::Path) -> crate::profiles::Profile {
    crate::profiles::Profiles::discover_from(root, None)
        .get("default")
        .expect("default profile")
}

#[test]
fn project_and_nested_instructions_are_rendered() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("AGENTS.md"), "root instructions").unwrap();
    let nested = root.path().join("src");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("AGENTS.md"), "nested instructions").unwrap();
    let resources = Resources::discover(root.path());
    let prompt = resources.prompt_section(&profile(root.path()));
    assert!(prompt.contains("root instructions"));
    assert!(prompt.contains("nested instructions"));
}

#[test]
fn prompt_lists_only_profile_permitted_skills_by_canonical_id() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".artist/skills/demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: demo skill\n---\nbody",
    )
    .unwrap();
    let resources = Resources::discover(root.path());
    let prompt = resources.prompt_section(&profile(root.path()));
    assert!(prompt.contains("<id>skill:demo</id>"));
    assert!(!prompt.contains("mode=activate"));
}

#[test]
fn available_skills_exposes_registry_metadata_without_loading_bodies() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join(".artist/skills/demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: demo skill\n---\nSECRET BODY",
    )
    .unwrap();
    let resources = Resources::discover(root.path());
    let available = resources.available_skills();
    let demo = available
        .iter()
        .find(|skill| skill.name == "demo")
        .expect("demo skill");
    assert_eq!(demo.description, "demo skill");
}
