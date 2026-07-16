//! Minimal multi-agent demo. Run from this crate:
//!
//! ```bash
//! cargo run --example basic
//! ```

use std::path::PathBuf;

use hashline_tools::{
    AgentIdentity, EditOperation, EditRequest, FileCoordinator, FileToolConfig, ReadFileRequest,
    WriteCondition,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace)?;
    let db = root.path().join("hashline.db");
    let locks = root.path().join("locks");

    let config = FileToolConfig {
        workspace_root: Some(workspace.clone()),
        allow_outside_workspace: false,
        follow_symlinks: false,
    };
    let coord = FileCoordinator::open(config, &db, &locks)?;

    let agent_a = AgentIdentity::from_id("agent-a").unwrap();
    let agent_b = AgentIdentity::from_id("agent-b").unwrap();

    let path = "demo.rs".to_string();
    let written = coord
        .write_file(
            &agent_a,
            path.clone(),
            "fn alpha() {}\nfn beta() {}\nfn gamma() {}\n".into(),
            WriteCondition::Absent,
        )
        .await?;

    println!("content_hash = {}", written.content_hash);
    println!("view:\n{}", written.result.content);

    // Agent B can read and gets its own independent anchor namespace.
    let view_b = coord
        .read_file(
            &agent_b,
            ReadFileRequest {
                path: path.clone(),
                start_line: 1,
                max_lines: None,
            },
        )
        .await?;
    println!("agent-b anchors:\n{}", view_b.result.content);

    // Agent A edits using the bare token from its own view.
    let beta_anchor = written
        .result
        .lines
        .iter()
        .find(|l| l.text.contains("beta"))
        .map(|l| l.anchor.clone())
        .expect("beta line");

    let edited = coord
        .edit_file(
            &agent_a,
            EditRequest {
                path: path.clone(),
                operations: vec![EditOperation::Replace {
                    hash: beta_anchor,
                    end_hash: None,
                    content: "fn beta_renamed() {}".into(),
                }],
            },
        )
        .await?;

    println!("after edit:\n{}", edited.result.content);
    println!("new content_hash = {}", edited.content_hash);

    // Show that the on-disk workspace path was updated.
    let on_disk = std::fs::read_to_string(workspace.join("demo.rs"))?;
    println!("on disk:\n{on_disk}");

    let _ = PathBuf::from("."); // silence unused in some editions
    Ok(())
}
