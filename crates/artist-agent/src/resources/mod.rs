mod agents;
mod instructions_tool;
mod skill_io;
mod skill_tool;
mod skills;
#[cfg(test)]
mod tests;

pub use instructions_tool::InstructionsTool;
pub use skill_tool::SkillTool;
use std::{collections::BTreeMap, path::Path, sync::Arc};

#[derive(Clone)]
pub struct Resources(Arc<ResourceData>);

struct ResourceData {
    workspace: std::path::PathBuf,
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
            workspace: workspace.to_owned(),
            agents,
            nested_agents,
            skills,
            activated: std::sync::Mutex::new(std::collections::HashSet::new()),
            diagnostics,
        }))
    }

    pub fn instructions_tool(&self) -> InstructionsTool {
        InstructionsTool(self.clone())
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
            output.push_str("\n\nNested AGENTS.md files exist. Before working beneath one of these directories, call the instructions tool for the target path:\n<nested_instruction_scopes>\n");
            for file in &self.0.nested_agents {
                output.push_str(&format!(
                    "  <file>{}</file>\n",
                    xml(&file.display().to_string())
                ));
            }
            output.push_str("</nested_instruction_scopes>");
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

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
