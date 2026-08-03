//! Architectural guardrails for the native frontend.
//!
//! These checks intentionally inspect the view source. GPUI's platform
//! accessibility adapters cannot be instantiated reliably in a headless test
//! process, but losing the component controls or semantic roles is still a
//! regression we can catch on every platform.

const APP: &str = include_str!("../src/app.rs");

#[test]
fn interactive_controls_come_from_gpui_component() {
    for required in [
        "gpui_component::init(cx)",
        "Root::new(app, window, cx)",
        "InputState::new(window, cx)",
        "Input::new(&self.composer)",
        "Button::new(\"send\")",
        "Button::new(\"new-conversation\")",
        "Button::new(\"open-project\")",
        "TextView::markdown",
    ] {
        assert!(
            APP.contains(required),
            "missing component foundation: {required}"
        );
    }

    for hand_rolled in ["on_key_down", "KeyDownEvent", "on_mouse_up"] {
        assert!(
            !APP.contains(hand_rolled),
            "interactive behavior must not regress to {hand_rolled}"
        );
    }
}

#[test]
fn conversation_exposes_a_semantic_accessibility_tree() {
    for required in [
        "Role::Application",
        "Role::Heading",
        "Role::Status",
        "Role::Log",
        "Role::Article",
        "Role::Alert",
        "Role::Navigation",
        "Role::Complementary",
        "aria_label(\"Conversation transcript\")",
        "aria_label(\"Message composer\")",
        "\"Artist context {}\"",
        "aria_label(\"Message\")",
        "aria_level(1)",
    ] {
        assert!(
            APP.contains(required),
            "missing accessibility semantic: {required}"
        );
    }
}

#[test]
fn repeated_transcript_text_has_stable_unique_ids() {
    assert!(APP.contains("block-{block_index}-line-{line_index}"));
    assert!(APP.contains("block-{block_index}-line-{line_index}-run-{run_index}"));
}

#[test]
fn transcript_runs_wrap_within_the_message_width() {
    let line_view = APP
        .split("fn line_view(")
        .nth(1)
        .expect("line_view must exist")
        .split("impl Render for ArtistApp")
        .next()
        .expect("line_view must precede Render");

    assert!(line_view.contains(".min_w_0()"));
    assert!(line_view.contains(".max_w_full()"));
    assert!(line_view.matches(".whitespace_normal()").count() >= 2);
}

#[test]
fn rich_navigation_rows_are_not_forced_into_component_button_height() {
    for row in [
        "Button::new((\"project\", index))",
        "Button::new((\"session\", index))",
    ] {
        let body = APP
            .split(row)
            .nth(1)
            .unwrap_or_else(|| panic!("missing navigation row: {row}"));
        let before_click = body
            .split(".on_click")
            .next()
            .expect("row must have a click handler");
        assert!(
            before_click.contains(".h_auto()") && before_click.contains(".min_h(px("),
            "rich row must override gpui-component's fixed button height: {row}"
        );
    }
}

#[test]
fn shell_uses_component_theme_tokens() {
    assert!(APP.contains("cx.theme().background"));
    assert!(APP.contains("cx.theme().foreground"));
    assert!(APP.contains("cx.theme().border"));
    assert!(!APP.contains("const BG:"));
}

#[test]
fn harness_completion_always_restores_the_composer() {
    for required in [
        "ARTIST_EVENT_STREAM_DONE=1",
        "Ready · compatibility mode",
        "harness bridge disconnected",
        "child.try_wait()",
    ] {
        assert!(
            APP.contains(required),
            "missing completion guard: {required}"
        );
    }
}

#[test]
fn conversation_has_streaming_and_recovery_affordances() {
    for required in [
        "ScrollHandle::new()",
        ".track_scroll(&self.transcript_scroll)",
        "self.transcript_scroll.scroll_to_bottom()",
        "if self.running { \"Steer\" } else { \"Send\" }",
        "Button::new(\"send\")",
        "Button::new(\"stop\")",
        "cancellation.store(true, Ordering::Release)",
        "cancellation.load(Ordering::Acquire)",
        "window_min_size: Some(size(px(680.), px(480.)))",
        "viewport.width >= px(760.)",
        "fn new_conversation(",
    ] {
        assert!(APP.contains(required), "missing GUI affordance: {required}");
    }
}

#[test]
fn workspace_supports_project_session_and_inspector_flows() {
    for required in [
        "prompt_for_paths",
        "fn select_project(",
        "fn select_session(",
        "fn delete_session(",
        "fn changes_view(",
        "fn activity_view(",
        "fn agents_view(",
        "git_diff(&self.project",
        "command.current_dir(project)",
        "fn rename_session(",
        "fn toggle_session_archived(",
        "fn toggle_session_pinned(",
        "fn fork_session(",
        "fn command_palette(",
        "fn session_group_rank(",
        "searchable_text",
        "DismissOverlay",
        "ARTIST_CONTROL_STREAM",
        "HarnessMessage::Question",
        "fn question_view(",
        "Input::new(&self.session_search)",
        "Input::new(&self.session_name)",
        "Input::new(&self.model_override)",
        "--provider",
        "--model",
        "--profile",
    ] {
        assert!(
            APP.contains(required),
            "missing workspace affordance: {required}"
        );
    }
}

#[test]
fn files_tab_and_permanent_inspector_are_not_rendered() {
    assert!(!APP.contains("SidebarTab::Files"));
    assert!(!APP.contains("Button::new((\"file\", index))"));
    let shell = APP
        .split("impl Render for ArtistApp")
        .nth(1)
        .expect("shell render must exist");
    assert!(!shell.contains("self.inspector(cx)"));
}
