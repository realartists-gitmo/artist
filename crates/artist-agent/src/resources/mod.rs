mod agents;

mod skill_io;
mod skill_tool;
mod skills;
#[cfg(test)]
mod tests;

pub use skill_tool::SkillTool;
pub(crate) use skills::Skill;
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableSkill {
    pub name: String,
    pub description: String,
}

#[derive(Clone)]
pub struct Resources(Arc<ResourceData>);

struct ResourceData {
    agents: Vec<agents::AgentsFile>,
    nested_agents: Vec<std::path::PathBuf>,
    skills: BTreeMap<String, skills::Skill>,

    diagnostics: Vec<String>,
}

impl Resources {
    pub fn discover(workspace: &Path) -> Self {
        let mut diagnostics = Vec::new();
        let agents = agents::discover(workspace, &mut diagnostics);
        let nested_agents = agents::nested(workspace, &mut diagnostics);
        let skills = skills::discover(workspace, &mut diagnostics);
        Self(Arc::new(ResourceData {
            agents,
            nested_agents,
            skills,

            diagnostics,
        }))
    }

    /// Problems encountered while loading instructions and skills.
    ///
    /// These were collected and then dropped on the floor: an `AGENTS.md` over
    /// the size cap, or a skill with unparseable frontmatter, failed silently
    /// and the user was left believing their instructions were in effect.
    pub fn diagnostics(&self) -> &[String] {
        &self.0.diagnostics
    }

    pub fn available_skills(&self) -> Vec<AvailableSkill> {
        self.0
            .skills
            .values()
            .map(|skill| AvailableSkill {
                name: skill.name.clone(),
                description: skill.description.clone(),
            })
            .collect()
    }

    /// The canonical skill resources visible to a particular profile. Reading
    /// one of these files is itself activation; this catalog carries no
    /// separate activation state.
    pub(crate) fn skills_for(&self, profile: &crate::profiles::Profile) -> Vec<Skill> {
        self.0
            .skills
            .values()
            .filter(|skill| profile.permits_skill(&skill.name))
            .cloned()
            .collect()
    }

    pub(crate) fn read_skill(
        &self,
        profile: &crate::profiles::Profile,
        name: &str,
    ) -> Result<String, String> {
        let skill = self
            .0
            .skills
            .get(name)
            .filter(|skill| profile.permits_skill(&skill.name))
            .ok_or_else(|| format!("unknown or disallowed skill `{name}`"))?;
        skill_io::read_bounded(&skill.file, &skill.base).map_err(|error| error.to_string())
    }

    pub fn prompt_section(&self, profile: &crate::profiles::Profile) -> String {
        let mut output = String::new();
        if !self.0.agents.is_empty() {
            output.push_str(
                "\n\n<project_context>\nProject-specific instructions and guidelines. Global instructions apply everywhere. Each project instruction applies only beneath its containing directory; when instructions conflict, the closest file to the target path wins.\n\n",
            );
            for file in &self.0.agents {
                let tag = if file.global {
                    "global_instructions"
                } else {
                    "project_instructions"
                };
                output.push_str(&format!(
                    "<{tag} path=\"{}\">\n{}\n</{tag}>\n\n",
                    xml(&file.path.display().to_string()),
                    file.content
                ));
            }
            output.push_str("</project_context>");
        }
        if !self.0.nested_agents.is_empty() {
            output.push_str("\n\n<scoped_project_instructions>\nThe following instructions apply only when working beneath their containing directories. More deeply nested instructions take precedence over broader instructions.\n\n");
            let mut remaining = 128 * 1024;
            for path in &self.0.nested_agents {
                let Ok(content) = std::fs::read_to_string(path) else {
                    continue;
                };
                if remaining == 0 {
                    break;
                }
                let end = floor_char_boundary(&content, remaining.min(content.len()));
                output.push_str(&format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>\n\n",
                    xml(&path.display().to_string()),
                    &content[..end]
                ));
                remaining -= end;
            }
            output.push_str("</scoped_project_instructions>");
        }

        let visible = self
            .0
            .skills
            .values()
            .filter(|skill| profile.permits_skill(&skill.name))
            .collect::<Vec<_>>();
        if !visible.is_empty() {
            output.push_str("\n\nThe following skills provide specialized instructions. Use the skill tool to search first; an exact name loads only after it appeared in an earlier search result.\n<available_skills>\n");
            for skill in visible {
                output.push_str(&format!(
                    "  <skill><id>skill:{}</id><description>{}</description></skill>\n",
                    xml(&skill.name),
                    xml(&skill.description.chars().take(1024).collect::<String>())
                ));
            }
            output.push_str("</available_skills>");
        }
        output
    }
}

fn floor_char_boundary(value: &str, mut index: usize) -> usize {
    while !value.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
