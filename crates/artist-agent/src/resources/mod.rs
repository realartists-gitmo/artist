mod agents;

mod skill_io;
mod skill_tool;
mod skills;
#[cfg(test)]
mod tests;

pub use skill_tool::SkillTool;
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
    activated: std::sync::Mutex<std::collections::HashSet<String>>,
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
            activated: std::sync::Mutex::new(std::collections::HashSet::new()),
            diagnostics,
        }))
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

    pub fn explicit_skill_section(&self, input: &str) -> String {
        let mut output = String::new();
        for skill in self.0.skills.values() {
            if mentions_skill(input, &skill.name)
                && let Ok(content) = self.skill_tool().activate(skill.name.clone())
            {
                output.push_str("\n\n");
                output.push_str(&content);
            }
        }
        output
    }

    pub fn skill_tool(&self) -> SkillTool {
        SkillTool(self.clone())
    }

    pub fn prompt_section(&self) -> String {
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

        if !self.0.skills.is_empty() {
            output.push_str("\n\nThe following skills provide specialized instructions. When a task matches a description, call the skill tool with mode=activate before proceeding.\n<available_skills>\n");
            for skill in self.0.skills.values() {
                output.push_str(&format!(
                    "  <skill><name>{}</name><description>{}</description></skill>\n",
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

fn mentions_skill(input: &str, name: &str) -> bool {
    let needle = format!("${name}");
    input.match_indices(&needle).any(|(index, _)| {
        input[index + needle.len()..]
            .chars()
            .next()
            .is_none_or(|character| {
                !(character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
            })
    })
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
