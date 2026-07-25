use dashmap::DashMap;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tokio::sync::Semaphore;

#[derive(Clone, Debug)]
pub struct Role {
    pub name: String,
    pub description: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub instructions: Option<String>,
    pub allow: Option<Vec<String>>,
    pub deny: Vec<String>,
}

#[derive(Clone)]
pub struct Subagents {
    roles: Arc<BTreeMap<String, Role>>,
    pub semaphore: Arc<Semaphore>,
    diagnostics: Arc<Vec<String>>,
}

#[derive(Default, Deserialize)]
struct FileConfig {
    settings: Option<Settings>,
    agents: Option<BTreeMap<String, AgentConfig>>,
}
#[derive(Default, Deserialize)]
struct Settings {
    max_concurrent: Option<usize>,
}
#[derive(Deserialize)]
struct AgentConfig {
    description: String,
    model: Option<String>,
    reasoning_effort: Option<String>,
    instructions: Option<String>,
    tools: Option<Tools>,
}
#[derive(Default, Deserialize)]
struct Tools {
    allow: Option<Vec<String>>,
    #[serde(default)]
    deny: Vec<String>,
}

const KNOWN_TOOLS: &[&str] = &[
    "bash", "read", "find", "grep", "edit", "write", "skill", "subagent",
];

impl Subagents {
    pub fn discover(project: &Path) -> Self {
        let global = std::env::var_os("ARTIST_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| dirs::config_dir().map(|p| p.join("artist")))
            .map(|p| p.join("subagents.toml"));
        Self::discover_from(project, global.as_deref())
    }

    fn discover_from(project: &Path, global: Option<&Path>) -> Self {
        let mut roles = builtins();
        let mut max = None;
        let mut diagnostics = Vec::new();
        let project_config = project.join(".artist/subagents.toml");
        for path in global
            .into_iter()
            .chain(std::iter::once(project_config.as_path()))
        {
            if !path.exists() {
                continue;
            }
            match std::fs::read_to_string(path)
                .ok()
                .and_then(|s| toml::from_str::<FileConfig>(&s).ok())
            {
                Some(file) => {
                    if let Some(value) = file.settings.and_then(|s| s.max_concurrent) {
                        max = Some(value.max(1));
                    }
                    for (name, cfg) in file.agents.unwrap_or_default() {
                        let tools = cfg.tools.unwrap_or_default();
                        let unknown = tools
                            .allow
                            .iter()
                            .flatten()
                            .chain(&tools.deny)
                            .filter(|n| !KNOWN_TOOLS.contains(&n.as_str()))
                            .cloned()
                            .collect::<Vec<_>>();
                        if !unknown.is_empty() {
                            diagnostics.push(format!(
                                "{}: agent {name} has unknown tools: {}",
                                path.display(),
                                unknown.join(", ")
                            ));
                            continue;
                        }
                        roles.insert(
                            name.clone(),
                            Role {
                                name,
                                description: cfg.description,
                                model: cfg.model,
                                reasoning_effort: cfg.reasoning_effort,
                                instructions: cfg.instructions,
                                allow: tools.allow,
                                deny: tools.deny,
                            },
                        );
                    }
                }
                None => diagnostics.push(format!(
                    "{}: invalid subagent configuration",
                    path.display()
                )),
            }
        }
        Self {
            roles: Arc::new(roles),
            semaphore: project_semaphore(project, max.unwrap_or(4)),
            diagnostics: Arc::new(diagnostics),
        }
    }

    pub fn role(&self, name: &str) -> Result<Role, String> {
        self.roles.get(name).cloned().ok_or_else(|| {
            format!(
                "unknown subagent role: {name}; available roles: {}",
                self.roles.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })
    }
    pub fn catalog(&self) -> String {
        let roles: String = self
            .roles
            .values()
            .map(|r| {
                format!(
                    "<subagent><name>{}</name><description>{}</description></subagent>",
                    escape(&r.name),
                    escape(&r.description)
                )
            })
            .collect();
        let diagnostics: String = self
            .diagnostics
            .iter()
            .map(|d| format!("<diagnostic>{}</diagnostic>", escape(d)))
            .collect();
        format!("{roles}{diagnostics}")
    }
    pub fn names(&self) -> Vec<String> {
        self.roles.keys().cloned().collect()
    }
}

impl Role {
    pub fn permits(&self, tool: &str) -> bool {
        let allowed = self
            .allow
            .as_ref()
            .is_none_or(|a| a.iter().any(|v| v == tool));
        allowed && !self.deny.iter().any(|v| v == tool) && tool != "subagent"
    }
}

fn project_semaphore(project: &Path, permits: usize) -> Arc<Semaphore> {
    static SEMAPHORES: OnceLock<DashMap<PathBuf, Arc<Semaphore>>> = OnceLock::new();
    SEMAPHORES
        .get_or_init(DashMap::new)
        .entry(project.to_owned())
        .or_insert_with(|| Arc::new(Semaphore::new(permits)))
        .clone()
}

fn builtins() -> BTreeMap<String, Role> {
    [
        (
            "default",
            "General-purpose agent inheriting the parent configuration",
            None,
            "Complete the delegated task and return concise findings with evidence.",
        ),
        (
            "worker",
            "Implementation-focused agent for bounded changes and verification",
            None,
            "Implement the requested change, verify it, and report modified files and residual risks.",
        ),
        (
            "explorer",
            "Read-heavy agent for tracing code and gathering evidence",
            Some(vec!["read", "find", "grep", "skill"]),
            "Inspect without editing. Return concise findings with file and symbol references.",
        ),
        (
            "planner",
            "Planning agent that turns requirements and code evidence into an executable plan",
            Some(vec!["read", "find", "grep", "skill"]),
            "Analyze requirements and the current code before planning. Return an ordered, implementation-ready plan with exact files, dependencies, verification steps, and risks. Do not edit files.",
        ),
        (
            "reviewer",
            "Review agent focused on correctness, regressions, security, and missing tests",
            Some(vec!["read", "find", "grep", "skill"]),
            "Review like a code owner. Lead with concrete findings ordered by severity, cite files and symbols, explain impact and reproduction, and avoid style-only feedback. Do not edit files.",
        ),
    ]
    .into_iter()
    .map(|(name, description, allow, instructions)| {
        (
            name.into(),
            Role {
                name: name.into(),
                description: description.into(),
                model: None,
                reasoning_effort: None,
                instructions: Some(instructions.into()),
                allow: allow.map(|v| v.into_iter().map(str::to_owned).collect()),
                deny: vec![],
            },
        )
    })
    .collect()
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layers_and_validates_tools() {
        let d = tempfile::tempdir().unwrap();
        let g = d.path().join("global.toml");
        std::fs::write(&g, "[agents.worker]\ndescription='global'").unwrap();
        std::fs::create_dir(d.path().join(".artist")).unwrap();
        std::fs::write(d.path().join(".artist/subagents.toml"),"[settings]\nmax_concurrent=2\n[agents.worker]\ndescription='project'\n[agents.bad]\ndescription='bad'\n[agents.bad.tools]\nallow=['wat']").unwrap();
        let s = Subagents::discover_from(d.path(), Some(&g));
        assert_eq!(s.role("worker").unwrap().description, "project");
        for name in ["default", "explorer", "planner", "reviewer"] {
            assert!(s.role(name).is_ok(), "missing built-in role {name}");
        }
        assert!(!s.role("planner").unwrap().permits("write"));
        assert!(!s.role("reviewer").unwrap().permits("bash"));
        assert!(s.role("bad").is_err());
        assert_eq!(s.semaphore.available_permits(), 2);
        assert!(s.catalog().contains("unknown tools"));
    }
}
