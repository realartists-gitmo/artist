use artist_plugin_sdk::artist::plugin::types::ContextFragment;

struct Prompt;

fn compose(fragments: Vec<ContextFragment>) -> Result<Vec<ContextFragment>, String> {
    Ok(fragments
        .into_iter()
        .filter(|fragment| !fragment.content.trim().is_empty())
        .collect())
}

artist_plugin_sdk::prompt_component!(Prompt, "artist.prompt", compose);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_empty_fragments() {
        let fragments = vec![
            ContextFragment {
                source: "empty".into(),
                content: "  ".into(),
                role: artist_plugin_sdk::artist::plugin::types::ContextRole::Other,
            },
            ContextFragment {
                source: "system".into(),
                content: "kept".into(),
                role: artist_plugin_sdk::artist::plugin::types::ContextRole::System,
            },
        ];
        assert_eq!(compose(fragments).unwrap().len(), 1);
    }
}
