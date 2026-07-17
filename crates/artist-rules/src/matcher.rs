//! The streaming matcher: compiled rule sets and per-run window buffers.
//!
//! Hot-path budget: one `RegexSet` prescreen over a bounded tail window,
//! evaluated only when ≥64 new bytes accumulated or a newline arrives.
//! Individual regexes run only after a set hit, to attribute the match.

use std::collections::HashMap;
use std::sync::Arc;

use regex::{Regex, RegexSet};

use crate::types::{DeclarativeRule, Firing, MatchTarget, RuleId};

const COALESCE_BYTES: usize = 64;
const EXCERPT_CAP: usize = 200;

/// One rule compiled for matching.
pub struct CompiledRule {
    pub rule: DeclarativeRule,
    regexes: Vec<Regex>,
}

/// All enabled rules compiled per target. Immutable once built; a run holds
/// an `Arc<RuleSet>` snapshot so hot-reload never swaps rules mid-run.
pub struct RuleSet {
    pub rules: Vec<Arc<CompiledRule>>,
    text: TargetMatcher,
    reasoning: TargetMatcher,
    tool_args: TargetMatcher,
}

/// RegexSet prescreen for one match target: pattern index → (rule, regex).
struct TargetMatcher {
    set: RegexSet,
    /// Parallel to the set's patterns.
    origins: Vec<(usize, usize)>,
    /// Max window across participating rules (bytes of tail retained).
    window: usize,
}

impl TargetMatcher {
    fn build(rules: &[Arc<CompiledRule>], target: MatchTarget) -> Self {
        let mut patterns = Vec::new();
        let mut origins = Vec::new();
        let mut window = 0;
        for (rule_index, compiled) in rules.iter().enumerate() {
            if !compiled.rule.targets.contains(&target) {
                continue;
            }
            window = window.max(compiled.rule.window);
            for (regex_index, pattern) in compiled.rule.patterns.iter().enumerate() {
                patterns.push(pattern.clone());
                origins.push((rule_index, regex_index));
            }
        }
        Self {
            // Patterns were validated at parse time; an empty set is fine.
            set: RegexSet::new(&patterns).unwrap_or_else(|_| RegexSet::empty()),
            origins,
            window: window.max(1),
        }
    }

    fn is_empty(&self) -> bool {
        self.origins.is_empty()
    }

    /// Find the first armed rule whose pattern matches `haystack`.
    fn find(
        &self,
        rules: &[Arc<CompiledRule>],
        haystack: &str,
        tool: Option<&str>,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<(Arc<CompiledRule>, String)> {
        if self.is_empty() {
            return None;
        }
        for pattern_index in self.set.matches(haystack) {
            let (rule_index, regex_index) = self.origins[pattern_index];
            let compiled = &rules[rule_index];
            if !armed(&compiled.rule.id) {
                continue;
            }
            if let Some(tool) = tool
                && !compiled.rule.tools.is_empty()
                && !compiled.rule.tools.iter().any(|name| name == tool)
            {
                continue;
            }
            if let Some(found) = compiled.regexes[regex_index].find(haystack) {
                let mut excerpt = found.as_str().to_owned();
                if excerpt.len() > EXCERPT_CAP {
                    excerpt.truncate(
                        (0..=EXCERPT_CAP)
                            .rev()
                            .find(|index| excerpt.is_char_boundary(*index))
                            .unwrap_or(0),
                    );
                }
                return Some((Arc::clone(compiled), excerpt));
            }
        }
        None
    }
}

impl RuleSet {
    pub fn compile(rules: Vec<DeclarativeRule>) -> Self {
        let rules: Vec<Arc<CompiledRule>> = rules
            .into_iter()
            .filter(|rule| rule.enabled)
            .map(|rule| {
                let regexes = rule
                    .patterns
                    .iter()
                    .map(|pattern| {
                        regex::RegexBuilder::new(pattern)
                            .size_limit(crate::declarative::REGEX_SIZE_LIMIT)
                            .build()
                            .expect("patterns validated at parse time")
                    })
                    .collect();
                Arc::new(CompiledRule { rule, regexes })
            })
            .collect();
        let text = TargetMatcher::build(&rules, MatchTarget::AssistantText);
        let reasoning = TargetMatcher::build(&rules, MatchTarget::ReasoningSummary);
        let tool_args = TargetMatcher::build(&rules, MatchTarget::ToolArgs);
        Self {
            rules,
            text,
            reasoning,
            tool_args,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// True when any enabled rule matches this target — used by hooks to
    /// skip observing high-frequency delta events entirely.
    pub fn observes(&self, target: MatchTarget) -> bool {
        match target {
            MatchTarget::AssistantText => !self.text.is_empty(),
            MatchTarget::ReasoningSummary => !self.reasoning.is_empty(),
            MatchTarget::ToolArgs => !self.tool_args.is_empty(),
        }
    }

    pub fn get(&self, id: &RuleId) -> Option<&Arc<CompiledRule>> {
        self.rules.iter().find(|compiled| compiled.rule.id == *id)
    }
}

/// A rolling tail window over one stream.
struct Window {
    buffer: String,
    limit: usize,
    pending: usize,
    saw_newline: bool,
}

impl Window {
    fn new(limit: usize) -> Self {
        Self {
            buffer: String::new(),
            limit,
            pending: 0,
            saw_newline: false,
        }
    }

    fn push(&mut self, delta: &str) {
        self.buffer.push_str(delta);
        self.pending += delta.len();
        self.saw_newline |= delta.contains('\n');
        if self.buffer.len() > self.limit * 2 {
            self.trim();
        }
    }

    fn trim(&mut self) {
        if self.buffer.len() > self.limit {
            let cut = self.buffer.len() - self.limit;
            let cut = (cut..self.buffer.len())
                .find(|index| self.buffer.is_char_boundary(*index))
                .unwrap_or(self.buffer.len());
            self.buffer.drain(..cut);
        }
    }

    /// True when enough accumulated to be worth evaluating.
    fn due(&self) -> bool {
        self.pending >= COALESCE_BYTES || (self.saw_newline && self.pending > 0)
    }

    fn settle(&mut self) {
        self.pending = 0;
        self.saw_newline = false;
        self.trim();
    }

    fn reset(&mut self) {
        self.buffer.clear();
        self.pending = 0;
        self.saw_newline = false;
    }
}

/// Per-run matcher state. One per agent run; reset each model turn.
pub struct StreamMatcher {
    rules: Arc<RuleSet>,
    text: Window,
    reasoning: Window,
    /// internal_call_id → (tool name once known, accumulated args).
    args: HashMap<String, (Option<String>, String)>,
}

impl StreamMatcher {
    pub fn new(rules: Arc<RuleSet>) -> Self {
        let text_window = rules.text.window;
        let reasoning_window = rules.reasoning.window;
        Self {
            rules,
            text: Window::new(text_window),
            reasoning: Window::new(reasoning_window),
            args: HashMap::new(),
        }
    }

    pub fn rules(&self) -> &Arc<RuleSet> {
        &self.rules
    }

    /// Clear per-turn buffers (call at each completion call).
    pub fn reset_turn(&mut self) {
        self.text.reset();
        self.reasoning.reset();
        self.args.clear();
    }

    pub fn push_text(&mut self, delta: &str, armed: &dyn Fn(&RuleId) -> bool) -> Option<Firing> {
        Self::push_windowed(
            &mut self.text,
            &self.rules.text,
            &self.rules.rules,
            MatchTarget::AssistantText,
            delta,
            armed,
        )
    }

    pub fn push_reasoning(
        &mut self,
        delta: &str,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<Firing> {
        Self::push_windowed(
            &mut self.reasoning,
            &self.rules.reasoning,
            &self.rules.rules,
            MatchTarget::ReasoningSummary,
            delta,
            armed,
        )
    }

    fn push_windowed(
        window: &mut Window,
        matcher: &TargetMatcher,
        rules: &[Arc<CompiledRule>],
        target: MatchTarget,
        delta: &str,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<Firing> {
        if matcher.is_empty() {
            return None;
        }
        window.push(delta);
        if !window.due() {
            return None;
        }
        let found = matcher.find(rules, &window.buffer, None, armed);
        window.settle();
        found.map(|(compiled, excerpt)| firing(&compiled, target, excerpt))
    }

    /// Feed a streamed tool-call argument fragment. `tool_name` is present on
    /// the first delta for a call only.
    pub fn push_tool_arg_delta(
        &mut self,
        internal_call_id: &str,
        tool_name: Option<&str>,
        delta: &str,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<Firing> {
        if self.rules.tool_args.is_empty() {
            return None;
        }
        let entry = self
            .args
            .entry(internal_call_id.to_owned())
            .or_insert_with(|| (None, String::new()));
        if let Some(name) = tool_name {
            entry.0 = Some(name.to_owned());
        }
        entry.1.push_str(delta);
        let tool = entry.0.clone();
        let haystack = entry.1.clone();
        self.match_args(&haystack, tool.as_deref(), armed)
    }

    /// Final check when a tool call's arguments are complete; drops the
    /// accumulator either way.
    pub fn tool_call_complete(
        &mut self,
        internal_call_id: &str,
        tool_name: &str,
        arguments: &str,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<Firing> {
        self.args.remove(internal_call_id);
        if self.rules.tool_args.is_empty() {
            return None;
        }
        self.match_args(arguments, Some(tool_name), armed)
    }

    fn match_args(
        &self,
        haystack: &str,
        tool: Option<&str>,
        armed: &dyn Fn(&RuleId) -> bool,
    ) -> Option<Firing> {
        self.rules
            .tool_args
            .find(&self.rules.rules, haystack, tool, armed)
            .map(|(compiled, excerpt)| firing(&compiled, MatchTarget::ToolArgs, excerpt))
    }
}

fn firing(compiled: &CompiledRule, target: MatchTarget, matched: String) -> Firing {
    Firing {
        rule: compiled.rule.id.clone(),
        target,
        matched,
        reminder: compiled.rule.reminder.clone(),
        persistence: compiled.rule.persistence,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::declarative::parse_parts;

    fn rule_set(rules: &[(&str, &str)]) -> Arc<RuleSet> {
        let rules = rules
            .iter()
            .map(|(name, yaml_extra)| {
                parse_parts(
                    &format!("name: {name}\ndescription: d\n{yaml_extra}"),
                    "reminder text",
                    None,
                )
                .unwrap()
            })
            .collect();
        Arc::new(RuleSet::compile(rules))
    }

    fn all_armed(_: &RuleId) -> bool {
        true
    }

    #[test]
    fn match_spanning_two_deltas_fires_on_newline() {
        let rules = rule_set(&[("leak", "patterns: ['Box::leak']")]);
        let mut matcher = StreamMatcher::new(rules);
        assert!(matcher.push_text("let x = Box::le", &all_armed).is_none());
        let firing = matcher.push_text("ak(data);\n", &all_armed).unwrap();
        assert_eq!(firing.rule, RuleId("leak".into()));
        assert_eq!(firing.matched, "Box::leak");
        assert_eq!(firing.target, MatchTarget::AssistantText);
    }

    #[test]
    fn coalescing_defers_until_enough_bytes() {
        let rules = rule_set(&[("leak", "patterns: ['Box::leak']")]);
        let mut matcher = StreamMatcher::new(rules);
        // No newline, under 64 bytes: not evaluated yet.
        assert!(matcher.push_text("Box::leak", &all_armed).is_none());
        // Crossing the byte threshold triggers evaluation.
        let filler = "x".repeat(64);
        assert!(matcher.push_text(&filler, &all_armed).is_some());
    }

    #[test]
    fn disarmed_rules_do_not_fire() {
        let rules = rule_set(&[("leak", "patterns: ['Box::leak']")]);
        let mut matcher = StreamMatcher::new(rules);
        let none = matcher.push_text("Box::leak\n", &|_| false);
        assert!(none.is_none());
    }

    #[test]
    fn tool_filter_applies_to_arg_matches() {
        let rules = rule_set(&[(
            "no-force",
            "targets: [tool-args]\npatterns: ['--force']\ntools: [bash]",
        )]);
        let mut matcher = StreamMatcher::new(Arc::clone(&rules));
        assert!(
            matcher
                .tool_call_complete("ic1", "edit", "push --force", &all_armed)
                .is_none()
        );
        let firing = matcher
            .tool_call_complete("ic2", "bash", "git push --force", &all_armed)
            .unwrap();
        assert_eq!(firing.target, MatchTarget::ToolArgs);
    }

    #[test]
    fn arg_deltas_accumulate_with_late_name() {
        let rules = rule_set(&[(
            "no-force",
            "targets: [tool-args]\npatterns: ['--force']\ntools: [bash]",
        )]);
        let mut matcher = StreamMatcher::new(rules);
        // First delta names the tool; later deltas do not.
        assert!(
            matcher
                .push_tool_arg_delta("ic1", Some("bash"), "{\"cmd\": \"git push --f", &all_armed)
                .is_none()
        );
        let firing = matcher
            .push_tool_arg_delta("ic1", None, "orce\"}", &all_armed)
            .unwrap();
        assert_eq!(firing.rule, RuleId("no-force".into()));
    }

    #[test]
    fn window_trims_but_can_miss_matches_wider_than_window() {
        let rules = rule_set(&[("wide", "patterns: ['START.*END']\nwindow: 256")]);
        let mut matcher = StreamMatcher::new(rules);
        assert!(matcher.push_text("START\n", &all_armed).is_none());
        // Push the START far outside the 256-byte window.
        for _ in 0..20 {
            assert!(matcher.push_text(&"y".repeat(64), &all_armed).is_none());
        }
        // Documented behavior: a match wider than the window is missed.
        assert!(matcher.push_text("END\n", &all_armed).is_none());
    }

    #[test]
    fn reasoning_target_matches_reasoning_stream_only() {
        let rules = rule_set(&[(
            "intent",
            "targets: [reasoning-summary]\npatterns: ['mock the data']",
        )]);
        let mut matcher = StreamMatcher::new(rules);
        assert!(matcher.push_text("mock the data\n", &all_armed).is_none());
        assert!(
            matcher
                .push_reasoning("I will mock the data\n", &all_armed)
                .is_some()
        );
    }

    #[test]
    fn reset_turn_clears_buffers() {
        let rules = rule_set(&[("leak", "patterns: ['Box::leak']")]);
        let mut matcher = StreamMatcher::new(rules);
        assert!(matcher.push_text("Box::le", &all_armed).is_none());
        matcher.reset_turn();
        assert!(matcher.push_text("ak\n", &all_armed).is_none());
    }

    #[test]
    fn observes_reflects_loaded_targets() {
        let rules = rule_set(&[("leak", "patterns: ['x']")]);
        assert!(rules.observes(MatchTarget::AssistantText));
        assert!(!rules.observes(MatchTarget::ToolArgs));
        assert!(!rules.observes(MatchTarget::ReasoningSummary));
    }
}
