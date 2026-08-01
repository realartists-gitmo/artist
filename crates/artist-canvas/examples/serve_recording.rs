//! Serve a project with a host that says what the page asked it for.
//!
//! `cargo run -p artist-canvas --example serve_recording -- <project-dir> <slug>`
//!
//! The other harnesses run against `DetachedHost`, which refuses everything —
//! so a page can be rendered but never observed *using* the bridge. This one
//! answers, and prints each call on a `BRIDGE ` line, which is what lets a
//! shell check that `artist.send` and `artist.call` in the browser reach the
//! agent with what the page meant.

use std::sync::Arc;

use artist_canvas::bridge::{CanvasHost, Denied, HostFuture, SendMode, SendOutcome};
use artist_session::ask::{Answer, Question, QuestionOption};

struct Recording;

impl CanvasHost for Recording {
    fn send(&self, text: String, mode: SendMode) -> HostFuture<'_, SendOutcome> {
        println!("BRIDGE send mode={mode:?} text={text}");
        Box::pin(async move {
            match mode {
                SendMode::Steer => SendOutcome::Steered,
                SendMode::Queue => SendOutcome::Queued,
            }
        })
    }

    fn call_tool(
        &self,
        tool: String,
        arguments: serde_json::Value,
        allowed: Vec<String>,
    ) -> HostFuture<'_, Result<String, Denied>> {
        println!("BRIDGE call tool={tool} arguments={arguments} allowed={allowed:?}");
        Box::pin(async move {
            if allowed.iter().any(|name| *name == tool) {
                Ok(format!("ran {tool}"))
            } else {
                Err(Denied::NotDeclared { tool })
            }
        })
    }

    fn state_changed(&self, slug: &str, keys: Vec<String>) {
        println!("BRIDGE notify slug={slug} keys={keys:?}");
    }

    fn pending_questions(&self) -> Vec<Question> {
        vec![Question {
            id: "q-probe".to_owned(),
            header: "Probe".to_owned(),
            question: "Does the bridge carry an answer?".to_owned(),
            multi_select: false,
            options: vec![QuestionOption {
                label: "yes".to_owned(),
                description: String::new(),
                preview: None,
            }],
        }]
    }

    fn answer_question(&self, answer: Answer, surface: &str) -> bool {
        println!(
            "BRIDGE answer id={} selected={:?} surface={surface}",
            answer.question_id, answer.selected
        );
        true
    }

    fn context(&self) -> serde_json::Value {
        serde_json::json!({"attached": true, "model": "probe", "busy": false})
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let project = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("cwd"));
    let slug = args.next().unwrap_or_else(|| "demo".to_owned());

    let server =
        artist_canvas::server::Server::start_with_host(project, Arc::new(Recording)).await?;
    println!("{}", server.url(&slug));

    loop {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        for report in server.take_reports(None) {
            println!("[{}] {}: {}", report.slug, report.level, report.message);
        }
    }
}
