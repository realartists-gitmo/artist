use super::{
    Resources,
    skill_io::{cap, message, read_bounded, resources},
    skills,
};
use crate::profiles::Profile;
use fff_search::{FuzzyQuery, QueryParser};
use rig_core::tool::{PortableTool, ToolExecutionError};
use serde::Deserialize;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct SkillTool {
    resources: Resources,
    states: artist_registry::ArtistStates,
    artist: String,
    profile: Profile,
}

impl SkillTool {
    pub fn new(
        resources: Resources,
        states: artist_registry::ArtistStates,
        artist: impl Into<String>,
        profile: Profile,
    ) -> Self {
        Self {
            resources,
            states,
            artist: artist.into(),
            profile,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillArgs {
    query: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("skill: {0}")]
    Message(String),
}

impl From<SkillError> for ToolExecutionError {
    fn from(value: SkillError) -> Self {
        ToolExecutionError::other(value.to_string()).with_code("skill_error")
    }
}

impl PortableTool for SkillTool {
    const NAME: &'static str = "skill";
    type Error = SkillError;
    type Args = SkillArgs;
    type Output = String;

    fn description(&self) -> String {
        "Search the profile-scoped skill registry. An exact skill name loads only after that name has appeared in a prior skill search result for this artist session; repeat the same exact call to load it.".into()
    }

    fn parameters(&self) -> Value {
        json!({
            "type":"object",
            "properties":{"query":{"type":"string"}},
            "required":["query"],
            "additionalProperties":false
        })
    }

    async fn call(&self, args: SkillArgs) -> Result<String, SkillError> {
        let query = args.query.trim();
        let exact = self
            .resources
            .0
            .skills
            .get(query)
            .filter(|skill| self.profile.permits_skill(&skill.name));
        let state = self
            .states
            .get(&self.artist)
            .map_err(|error| SkillError::Message(error.to_string()))?;

        if let Some(skill) = exact
            && state
                .seen_skill_names
                .iter()
                .any(|name| name == &skill.name)
        {
            if state
                .loaded_skill_names
                .iter()
                .any(|name| name == &skill.name)
            {
                return Ok(format!("skill:{} already loaded", skill.name));
            }
            let output = self.load(skill)?;
            self.states
                .update(&self.artist, |state| {
                    if !state
                        .loaded_skill_names
                        .iter()
                        .any(|name| name == &skill.name)
                    {
                        state.loaded_skill_names.push(skill.name.clone());
                    }
                })
                .map_err(|error| SkillError::Message(error.to_string()))?;
            return Ok(output);
        }

        let results = self.search(query);
        self.states
            .update(&self.artist, |state| {
                for skill in &results {
                    if !state
                        .seen_skill_names
                        .iter()
                        .any(|name| name == &skill.name)
                    {
                        state.seen_skill_names.push(skill.name.clone());
                    }
                }
            })
            .map_err(|error| SkillError::Message(error.to_string()))?;

        let mut lines = results
            .iter()
            .map(|skill| format!("skill:{} — {}", skill.name, skill.description))
            .collect::<Vec<_>>();
        lines.extend(
            self.resources
                .0
                .diagnostics
                .iter()
                .map(|diagnostic| format!("[warning] {diagnostic}")),
        );
        if query.is_empty() {
            return Ok(cap(if lines.is_empty() {
                "No skills available for this profile.".into()
            } else {
                lines.join("\n")
            }));
        }
        if exact.is_some() {
            lines.push(format!(
                "Call skill {{\"query\":{}}} again with the exact same query to load it.",
                serde_json::to_string(query).unwrap_or_else(|_| "\"\"".into())
            ));
        }
        Ok(cap(if lines.is_empty() {
            "No matching skills.".into()
        } else {
            lines.join("\n")
        }))
    }
}

impl SkillTool {
    fn load(&self, skill: &skills::Skill) -> Result<String, SkillError> {
        let text = read_bounded(&skill.file, &skill.base)?;
        let (_, body) = skills::frontmatter(&text).map_err(message)?;
        let resources = resources(&skill.base).join("\n");
        Ok(cap(format!(
            "<skill_content id=\"skill:{}\" name=\"{}\">\n{}\n\nSkill directory: {}\nUse the ordinary read tool for files below this directory.\n<skill_resources>\n{}\n</skill_resources>\n</skill_content>",
            skill.name,
            skill.name,
            body,
            skill.base.display(),
            resources
        )))
    }

    fn search(&self, query: &str) -> Vec<&skills::Skill> {
        let allowed = self
            .resources
            .0
            .skills
            .values()
            .filter(|skill| self.profile.permits_skill(&skill.name))
            .collect::<Vec<_>>();
        if query.trim().is_empty() {
            return allowed;
        }

        // Use the same FFF query parser and neo-frizbee matcher used by the find
        // subsystem. Skill search operates over the registry's name+description
        // projection rather than filesystem paths, but retains FFF fuzzy semantics.
        let parsed = QueryParser::default().parse(query);
        let needle = match parsed.fuzzy_query {
            FuzzyQuery::Text(text) => text.to_owned(),
            FuzzyQuery::Parts(parts) => parts.iter().copied().collect::<Vec<_>>().join(" "),
            FuzzyQuery::Empty => query.trim().to_owned(),
        };
        if needle.is_empty() {
            return allowed;
        }
        let haystacks = allowed
            .iter()
            .map(|skill| format!("{} {}", skill.name, skill.description))
            .collect::<Vec<_>>();
        let config = neo_frizbee::Config {
            max_typos: Some((needle.chars().count() / 3).max(1).min(u16::MAX as usize) as u16),
            casing: neo_frizbee::CaseMatching::Smart,
            ..Default::default()
        };
        let matches = neo_frizbee::match_list(&needle, &haystacks, &config);
        let mut result = matches
            .into_iter()
            .take(40)
            .filter_map(|matched| allowed.get(matched.index as usize).copied())
            .collect::<Vec<_>>();

        // Exact names are contractual referents. Ensure an exact match is visible
        // in its own first search even if description scoring would otherwise rank it out.
        if let Some(exact) = allowed.iter().copied().find(|skill| skill.name == query)
            && !result.iter().any(|skill| skill.name == exact.name)
        {
            result.insert(0, exact);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiles::Profiles;

    fn write_skill(root: &std::path::Path, name: &str, description: &str) {
        let dir = root.join(".artist/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\nbody for {name}"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn exact_name_is_search_then_load_and_state_is_durable() {
        let root = tempfile::tempdir().unwrap();
        write_skill(root.path(), "rust-review", "Review Rust code");
        let resources = Resources::discover(root.path());
        let states = artist_registry::Registry::for_project(root.path()).artist_states();
        let profile = Profiles::discover_from(root.path(), None)
            .get("default")
            .unwrap();
        let first = SkillTool::new(resources.clone(), states.clone(), "Goethe", profile.clone());
        let searched = first
            .call(SkillArgs {
                query: "rust-review".into(),
            })
            .await
            .unwrap();
        assert!(searched.contains("skill:rust-review"));
        assert!(searched.contains("exact same query"));

        // A distinct tool instance/process view uses the durable seen state.
        let second = SkillTool::new(resources, states, "Goethe", profile);
        let loaded = second
            .call(SkillArgs {
                query: "rust-review".into(),
            })
            .await
            .unwrap();
        assert!(loaded.contains("body for rust-review"));
        let again = second
            .call(SkillArgs {
                query: "rust-review".into(),
            })
            .await
            .unwrap();
        assert_eq!(again, "skill:rust-review already loaded");
    }

    #[test]
    fn schema_is_one_argument_only() {
        let root = tempfile::tempdir().unwrap();
        let resources = Resources::discover(root.path());
        let states = artist_registry::Registry::for_project(root.path()).artist_states();
        let profile = Profiles::discover_from(root.path(), None)
            .get("default")
            .unwrap();
        let tool = SkillTool::new(resources, states, "Goethe", profile);
        let schema = tool.parameters();
        assert_eq!(schema["required"], json!(["query"]));
        assert_eq!(schema["properties"].as_object().unwrap().len(), 1);
    }
}
