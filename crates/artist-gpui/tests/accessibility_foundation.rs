//! Architectural guardrails for the native frontend.
//!
//! These checks intentionally inspect the view source. GPUI's platform
//! accessibility adapters cannot be instantiated reliably in a headless test
//! process, but losing the component controls or semantic roles is still a
//! regression we can catch on every platform.

const APP: &str = include_str!("../src/app.rs");
const HOST_CONTROLLER: &str = include_str!("../src/host_controller.rs");
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
fn icon_controls_register_and_use_component_assets() {
    for required in [
        "use gpui_component_assets::Assets;",
        "application().with_assets(Assets).run",
        ".icon(IconName::GalleryVerticalEnd)",
        ".icon(IconName::Plus)",
        ".icon(IconName::FolderOpen)",
        ".icon(IconName::ArrowLeft)",
        ".icon(IconName::Copy)",
    ] {
        assert!(
            APP.contains(required),
            "missing icon asset or icon control setup: {required}"
        );
    }

    for obsolete_label in [
        ".label(tree_toggle_label)",
        ".label(\"New\")",
        ".label(\"Open\")",
        ".label(\"Back\")",
        ".label(\"Copy\")",
    ] {
        assert!(
            !APP.contains(obsolete_label),
            "icon action regressed to visible text: {obsolete_label}"
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
        "TreeState",
        "ListItem::new(ix)",
        "aria_label(\"Conversation transcript\")",
        "aria_label(\"Message composer\")",
        "\"Current context: {contextual_title}\"",
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
fn rich_navigation_rows_use_component_tree_and_list_items() {
    for required in [
        "workspace_tree: Entity<TreeState>",
        "let tree_view = tree(",
        "&self.workspace_tree",
        "TreeItem::new(",
        "ListItem::new(ix)",
        ".h(px(48.))",
        ".min_h(px(48.))",
        "cx.theme().sidebar_accent",
        "cx.theme().list_active_border",
        "tree_parenthood_guides(depth, cx)",
        "Spinner::new()",
    ] {
        assert!(
            APP.contains(required),
            "missing component tree behavior: {required}"
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
fn every_prompt_uses_the_resident_session_host() {
    for required in [
        "create_snapshot(&self.project, None)",
        "fn start_host_prompt(",
        "HostCommand::Message",
        "HostCommand::QueueNextTurn",
        "HostCommand::Stop",
        "Session host unavailable",
    ] {
        assert!(
            APP.contains(required),
            "missing resident-host guard: {required}"
        );
    }
    for removed in [
        "fn run_harness(",
        "ARTIST_EVENT_STREAM_DONE=1",
        "Ready · compatibility mode",
        "child.try_wait()",
        "ARTIST_CONTROL_STREAM",
    ] {
        assert!(
            !APP.contains(removed),
            "obsolete subprocess path remains: {removed}"
        );
    }
}

#[test]
fn resident_host_event_pump_never_consumes_an_unhandled_message() {
    assert!(APP.contains("match receive.try_recv()"));
    assert!(APP.contains("Err(mpsc::TryRecvError::Empty) => break"));
    assert!(APP.contains("Err(mpsc::TryRecvError::Disconnected) => return"));
    assert_eq!(
        APP.matches("receive.try_recv()").count(),
        1,
        "a second probe can consume and discard a host event"
    );
}

#[test]
fn dropping_a_root_controller_stops_its_worker() {
    assert!(HOST_CONTROLLER.contains("match commands.try_recv()"));
    assert!(HOST_CONTROLLER.contains("Err(mpsc::TryRecvError::Disconnected) => return"));
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
        "HostCommand::Steer",
        "HostCommand::Answer",
        "window_min_size: Some(size(px(680.), px(480.)))",
        "viewport.width >= px(980.)",
        "fn new_conversation(",
    ] {
        assert!(APP.contains(required), "missing GUI affordance: {required}");
    }
}
#[test]
fn workspace_supports_project_session_and_focus_stack_flows() {
    for required in [
        "prompt_for_paths",
        "fn select_project(",
        "fn select_session(",
        "fn delete_session(",
        "fn rename_session(",
        "fn toggle_session_pinned(",
        "fn fork_session(",
        "searchable_text",
        "DismissOverlay",
        "HarnessMessage::Question",
        "fn question_view(",
        "fn promote_object(",
        "fn pop_focus(",
        "fn focused_object_view(",
        "fn ensure_canvas_view(",
        "Input::new(&self.session_search)",
        "HostEvent::AccessibilityPatch",
        "stage_accessibility",
        "fn activate_stage_accessibility(",
        "fn stage_accessibility_role(",
    ] {
        assert!(
            APP.contains(required),
            "missing workspace affordance: {required}"
        );
    }
    for removed in [
        "fn inspector(",
        "fn changes_view(",
        "fn activity_view(",
        "fn agents_view(",
        "InspectorTab",
    ] {
        assert!(
            !APP.contains(removed),
            "removed inspector path remains: {removed}"
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

#[test]
fn accepted_gui_architecture_is_preserved() {
    for required in [
        "aria_label(\"Project and object tree\")",
        "workspace_tree: Entity<TreeState>",
        "let tree_view = tree(",
        "&self.workspace_tree",
        "ListItem::new(ix)",
        "\"needs_input\"",
        "preview =",
        ".session_summaries",
        "id(\"workspace-switcher\")",
        "TabBar::new(\"workspace-tabs\")",
        ".underline()",
        "Breadcrumb::new()",
        "when(!showing_focus, |view| view.max_w(px(820.)).mx_auto())",
        "when(showing_focus, |view| view.h_full())",
        "Button::new((\"tool-portal\", index))",
        "Button::new((\"subagent-portal\", index))",
        "inline-object-preview-",
        "this.promote_object(object.clone(), window, cx)",
        "Button::new(\"queue-next\")",
        "Button::new(\"jump-latest\")",
        "h_resizable(\"artist-workspace-layout\")",
        "Alert::warning(",
        "Collapsible::new()",
        "ButtonGroup::new(\"composer-actions\")",
    ] {
        assert!(
            APP.contains(required),
            "missing accepted GUI architecture: {required}"
        );
    }

    for removed in [
        "Type a command, project, or session",
        "Button::new(\"show-commands\")",
        "Button::new(\"composer-routing\")",
        "fn command_palette(",
        "WORKSPACE",
        "Ready · resumed",
        "Archived",
        "Toggle archived",
    ] {
        assert!(!APP.contains(removed), "obsolete GUI remains: {removed}");
    }

    let message = APP
        .split("Block::Message")
        .nth(1)
        .expect("message view must exist")
        .split("Block::Reasoning")
        .next()
        .expect("message view must precede reasoning");
    assert!(message.contains("when(user"));
    assert!(!message.contains("else {\n                        cx.theme().group_box"));

    let summary = APP
        .split("fn summarize_session")
        .nth(1)
        .expect("session summary must exist");
    assert!(summary.contains("ReplayItem::User(text) | ReplayItem::Assistant(text)"));
    assert!(
        !summary
            .split("summary.preview =")
            .nth(1)
            .unwrap()
            .split("summary\n}")
            .next()
            .unwrap()
            .contains("ReplayItem::Tool")
    );
}
