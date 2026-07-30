use super::{
    Resources,
    skill_io::{cap, message, read_bounded, resources},
    skills,
};
use rig_core::tool::Tool;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Component, Path};

#[derive(Clone)]
pub struct SkillTool(pub(super) Resources);

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillArgs {
    mode: Option<String>,
    name: Option<String>,
    path: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill: {0}")]
    Message(String),
}

impl Tool for SkillTool {
    const NAME: &'static str = "skill";
    type Error = SkillError;
    type Args = SkillArgs;
    type Output = String;

    fn description(&self) -> String {
        "List or activate an Agent Skill, or read a resource from an activated skill directory."
            .into()
    }
    fn parameters(&self) -> Value {
        let names = self.0.0.skills.keys().cloned().collect::<Vec<_>>();
        json!({"type":"object","properties":{
            "mode":{"enum":["list","activate","readResource"],"default":"list"},
            "name":{"type":"string","enum":names},
            "path":{"type":"string","description":"Skill-root-relative resource path"}
        },"additionalProperties":false})
    }

    async fn call(&self, args: SkillArgs) -> Result<String, SkillError> {
        match args.mode.as_deref().unwrap_or("list") {
            "list" => Ok(self.list()),
            "activate" => self.activate(required(args.name, "name")?),
            "readResource" => {
                self.read_resource(required(args.name, "name")?, required(args.path, "path")?)
            }
            mode => Err(SkillError::Message(format!("invalid mode: {mode}"))),
        }
    }
}

impl SkillTool {
    fn list(&self) -> String {
        let mut lines = self
            .0
            .0
            .skills
            .values()
            .map(|skill| format!("{} — {}", skill.name, skill.description))
            .collect::<Vec<_>>();
        lines.extend(
            self.0
                .0
                .diagnostics
                .iter()
                .map(|diagnostic| format!("[warning] {diagnostic}")),
        );
        cap(lines.join("\n"))
    }

    pub(super) fn activate(&self, name: String) -> Result<String, SkillError> {
        let skill = self.skill(&name)?;
        let text = read_bounded(&skill.file, &skill.base)?;
        let (_, body) = skills::frontmatter(&text).map_err(message)?;
        let resources = resources(&skill.base).join("\n");
        self.0
            .0
            .activated
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(name);
        Ok(cap(format!(
            "<skill_content name=\"{}\">\n{}\n\nSkill directory: {}\nRelative paths are relative to this directory.\n<skill_resources>\n{}\n</skill_resources>\n</skill_content>",
            skill.name,
            body,
            skill.base.display(),
            resources
        )))
    }

    fn read_resource(&self, name: String, path: String) -> Result<String, SkillError> {
        let skill = self.skill(&name)?;
        if !self
            .0
            .0
            .activated
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(&name)
        {
            return Err(SkillError::Message(format!(
                "skill `{name}` is not activated"
            )));
        }
        let relative = Path::new(&path);
        if relative.is_absolute()
            || relative.components().any(|part| {
                matches!(
                    part,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(SkillError::Message(
                "resource path escapes the skill".into(),
            ));
        }
        let target = skill.base.join(relative).canonicalize().map_err(message)?;
        if !target.starts_with(&skill.base) {
            return Err(SkillError::Message(
                "resource path escapes the skill".into(),
            ));
        }
        read_bounded(&target, &skill.base)
    }

    fn skill(&self, name: &str) -> Result<&skills::Skill, SkillError> {
        self.0
            .0
            .skills
            .get(name)
            .ok_or_else(|| SkillError::Message(format!("unknown skill: {name}")))
    }
}

fn required(value: Option<String>, name: &str) -> Result<String, SkillError> {
    value.ok_or_else(|| SkillError::Message(format!("{name} is required")))
}
