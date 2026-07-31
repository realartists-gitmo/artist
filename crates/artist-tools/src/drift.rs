//! Telling the model that files moved underneath it.
//!
//! Anchors are content-addressed and reconciled on every operation, so an edit
//! against a file someone else changed cannot misapply — it fails as a stale
//! anchor. That makes this module a latency optimisation rather than a safety
//! mechanism: without it the model learns from the failure, two round trips
//! later, having already spent a turn on an edit that could not land.
//!
//! Which is also the licence to be aggressive about trimming. Anything dropped
//! here degrades to today's behaviour, never to a wrong edit.

use hashline_tools::{AgentIdentity, Drift, DriftKind, FileCoordinator};
use similar::TextDiff;

use crate::output::{DiffStyle, anchored_diff_styled};

/// Asks, on behalf of one session, what moved underneath it.
///
/// Exists so the agent crate can hook this into tool dispatch without knowing
/// anything about coordinators, actors or anchor tables — the whole contract is
/// "call `report`, splice the string on if you get one".
#[derive(Clone)]
pub struct DriftWatch {
    files: FileCoordinator,
    actor: AgentIdentity,
    budget: usize,
}

impl DriftWatch {
    pub fn new(files: FileCoordinator, actor: AgentIdentity) -> Self {
        DriftWatch {
            files,
            actor,
            budget: DRIFT_BUDGET,
        }
    }

    /// What changed since the model last looked, or `None`.
    ///
    /// Errors are swallowed deliberately. This runs after every tool call and
    /// reports on something the model can already recover from unaided — a
    /// drift check that failed must never turn a successful tool call into a
    /// failed one.
    pub async fn report(&self) -> Option<String> {
        let drifts = self.files.drifted(&self.actor).await.ok()?;
        report(drifts, self.budget)
    }
}

/// How much of a tool result a drift report may take.
///
/// `output::OUTPUT_CAP` is 50 KiB and truncates from the end, so an unbounded
/// report does not stay unbounded — it gets cut at an arbitrary point, very
/// possibly mid-diff, and takes the actual tool output with it. The choice is
/// not whether to truncate but whether to choose where. A sixth of the cap
/// leaves the tool's own output the room it came for.
pub const DRIFT_BUDGET: usize = 8 * 1024;

/// Render drift for the model, or `None` when nothing moved.
///
/// Files are ordered by how recently the model touched them, so the budget is
/// spent on what it is most likely working on and the overflow is what it had
/// forgotten about.
pub fn report(mut drifts: Vec<Drift>, budget: usize) -> Option<String> {
    if drifts.is_empty() {
        return None;
    }
    drifts.sort_by(|left, right| right.touched.cmp(&left.touched));

    let mut sections: Vec<String> = Vec::new();
    let mut overflow: Vec<String> = Vec::new();
    let mut spent = 0usize;

    for drift in &drifts {
        // Once the budget is gone every remaining file degrades, rather than
        // the next small one sneaking in ahead of a larger earlier one. Keeping
        // the order honest matters more than packing the budget tightly.
        if spent >= budget {
            overflow.push(summary_line(drift));
            continue;
        }
        let section = section_for(drift, budget.saturating_sub(spent));
        spent += section.len();
        sections.push(section);
    }

    let mut out = String::from(
        "\n\n--- files changed since you read them ---\n\
         Anchors below are current. Unlisted files are unaffected.\n",
    );
    for section in sections {
        out.push('\n');
        out.push_str(&section);
    }
    if !overflow.is_empty() {
        out.push_str(&format!(
            "\n{} more file(s) changed; re-read before editing:\n",
            overflow.len()
        ));
        for line in overflow {
            out.push_str(&format!("  {line}\n"));
        }
    }
    Some(out)
}

/// One file's entry, in full.
fn section_for(drift: &Drift, remaining: usize) -> String {
    let attribution = match &drift.writer {
        // Worth distinguishing: another session is a coordination signal and
        // may warrant leaving the file alone, whereas an unattributed change is
        // usually the user's editor and warrants nothing but a re-read.
        Some(writer) => format!(" (written by {writer})"),
        None => String::new(),
    };

    match &drift.kind {
        DriftKind::Deleted => format!(
            "{} — deleted{attribution}. Anchors you hold for it are gone.\n",
            drift.path
        ),
        DriftKind::Unreadable(why) => {
            format!("{} — could not be read{attribution}: {why}\n", drift.path)
        }
        DriftKind::Modified {
            before,
            after,
            before_text,
            after_text,
        } => {
            let raw = TextDiff::from_lines(before_text, after_text)
                .unified_diff()
                .context_radius(2)
                .to_string();
            let body = anchored_diff_styled(&raw, before, after, DiffStyle::Explicit);
            let header = format!("{} — changed{attribution}:\n", drift.path);

            // A single file larger than what is left gets a window rather than
            // nothing: the first lines of a diff are usually the informative
            // ones, and silently dropping the file entirely would leave the
            // model believing it still knows the contents.
            if header.len() + body.len() <= remaining {
                format!("{header}{body}\n")
            } else {
                let room = remaining.saturating_sub(header.len() + 64);
                let (kept, elided) = window(&body, room);
                format!("{header}{kept}\n  … {elided} more diff line(s); re-read for the rest\n")
            }
        }
    }
}

/// Trim a rendered diff to `room` bytes on a line boundary, reporting how many
/// lines were dropped.
fn window(body: &str, room: usize) -> (String, usize) {
    let mut kept = String::new();
    let mut lines = body.lines();
    for line in lines.by_ref() {
        if kept.len() + line.len() + 1 > room {
            // Put this line back in the count: it was not kept.
            return (kept, lines.count() + 1);
        }
        kept.push_str(line);
        kept.push('\n');
    }
    (kept, 0)
}

/// The degraded form: enough to know whether to care, not what changed.
fn summary_line(drift: &Drift) -> String {
    match &drift.kind {
        DriftKind::Deleted => format!("{} (deleted)", drift.path),
        DriftKind::Unreadable(_) => format!("{} (unreadable)", drift.path),
        DriftKind::Modified {
            before_text,
            after_text,
            ..
        } => {
            let (added, removed) = magnitude(before_text, after_text);
            format!("{} (+{added} −{removed})", drift.path)
        }
    }
}

/// Line counts either way, so the summary says how much moved rather than only
/// that something did.
fn magnitude(before: &str, after: &str) -> (usize, usize) {
    let diff = TextDiff::from_lines(before, after);
    let mut added = 0;
    let mut removed = 0;
    for change in diff.iter_all_changes() {
        match change.tag() {
            similar::ChangeTag::Insert => added += 1,
            similar::ChangeTag::Delete => removed += 1,
            similar::ChangeTag::Equal => {}
        }
    }
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hashline_tools::AnchoredLine;

    fn line(number: usize, anchor: &str, text: &str) -> AnchoredLine {
        AnchoredLine {
            line_number: number,
            anchor: anchor.to_owned(),
            text: text.to_owned(),
        }
    }

    fn modified(path: &str, touched: u64, before: &str, after: &str) -> Drift {
        Drift {
            path: path.to_owned(),
            touched,
            writer: None,
            kind: DriftKind::Modified {
                before: vec![line(1, "campfire", before)],
                after: vec![line(1, "lantern", after)],
                before_text: format!("{before}\n"),
                after_text: format!("{after}\n"),
            },
        }
    }

    /// Nothing moved must cost nothing. This runs after every tool call, so a
    /// report that always appends something would tax every result in the
    /// session for the sake of the rare one that matters.
    #[test]
    fn no_drift_renders_nothing() {
        assert!(report(Vec::new(), DRIFT_BUDGET).is_none());
    }

    /// The removal row is the point: it carries the anchor the model is still
    /// holding, so it is what connects the stale handle to its fate.
    #[test]
    fn a_modification_shows_what_went_as_well_as_what_came() {
        let rendered = report(
            vec![modified(
                "a.rs",
                1,
                "define Gortnite {}",
                "define Fortnite {}",
            )],
            DRIFT_BUDGET,
        )
        .expect("drift renders");

        assert!(rendered.contains("a.rs"), "{rendered}");
        assert!(rendered.contains("-define Gortnite {}"), "{rendered}");
        assert!(rendered.contains("+define Fortnite {}"), "{rendered}");
        // The collapsed style would have shown only `~define Fortnite {}`.
        assert!(!rendered.contains("~define"), "{rendered}");
    }

    /// Budget is spent on what the model touched last, because that is what it
    /// is most likely about to edit.
    #[test]
    fn the_most_recently_touched_file_is_reported_first() {
        let rendered = report(
            vec![
                modified("old.rs", 1, "a", "b"),
                modified("recent.rs", 9, "c", "d"),
            ],
            DRIFT_BUDGET,
        )
        .expect("drift renders");

        let recent = rendered.find("recent.rs").expect("recent listed");
        let old = rendered.find("old.rs").expect("old listed");
        assert!(recent < old, "{rendered}");
    }

    /// Overflow degrades to a magnitude, not a bare name: "+40 −12" is enough
    /// to decide whether to look, which a filename alone is not.
    #[test]
    fn overflow_degrades_to_a_magnitude_rather_than_vanishing() {
        let big = "x\n".repeat(400);
        let drifts = vec![
            modified("first.rs", 3, &big, &big.replace('x', "y")),
            modified("second.rs", 2, "a", "b"),
            modified("third.rs", 1, "c", "d"),
        ];

        let rendered = report(drifts, 256).expect("drift renders");
        assert!(rendered.contains("more file(s) changed"), "{rendered}");
        assert!(rendered.contains("third.rs (+"), "{rendered}");
        assert!(rendered.contains("−"), "{rendered}");
    }

    /// A file bigger than the whole budget is windowed, not dropped. Dropping
    /// it silently would leave the model believing it still knows the file.
    #[test]
    fn an_oversized_single_file_is_windowed_and_says_so() {
        let big = "x\n".repeat(500);
        let rendered =
            report(vec![modified("huge.rs", 1, &big, "y\n")], 512).expect("drift renders");

        assert!(rendered.contains("huge.rs"), "{rendered}");
        assert!(rendered.contains("more diff line(s)"), "{rendered}");
        assert!(rendered.len() < 2048, "budget overrun: {}", rendered.len());
    }

    /// Deleted is not modified. "Re-read it" is the wrong advice for a file
    /// that is not there.
    #[test]
    fn deletion_is_reported_as_deletion() {
        let rendered = report(
            vec![Drift {
                path: "gone.rs".into(),
                touched: 1,
                writer: None,
                kind: DriftKind::Deleted,
            }],
            DRIFT_BUDGET,
        )
        .expect("drift renders");

        assert!(rendered.contains("gone.rs — deleted"), "{rendered}");
        assert!(rendered.contains("Anchors you hold"), "{rendered}");
    }

    /// Another session's write reads differently from an unattributed one: the
    /// first is a coordination signal, the second is usually the user.
    #[test]
    fn another_sessions_write_is_attributed() {
        let mut drift = modified("shared.rs", 1, "a", "b");
        drift.writer = Some("agent-two".into());
        let rendered = report(vec![drift], DRIFT_BUDGET).expect("drift renders");
        assert!(rendered.contains("written by agent-two"), "{rendered}");
    }
}
