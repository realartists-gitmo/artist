use artist_plugin_sdk::artist::plugin::types::ModelContext;

struct Context;

fn transform(mut context: ModelContext) -> Result<ModelContext, String> {
    let from = context.history.len().saturating_sub(200);
    context.history = context.history.into_iter().skip(from).collect();
    if !context.context.is_empty() {
        context.context.push_str("\n\n");
    }
    context.context.push_str("[context-plugin]");
    Ok(context)
}

artist_plugin_sdk::context_component!(Context, "artist.context", 0, transform);

#[cfg(test)]
mod tests {
    use artist_plugin_sdk::artist::plugin::types::{ContentPart, ModelHistoryItem};

    use super::*;

    #[test]
    fn retains_only_the_last_two_hundred_typed_history_items() {
        let context = ModelContext {
            context: "system".into(),
            prompt: vec![ContentPart::Text("current".into())],
            history: (0..205)
                .map(|index| ModelHistoryItem {
                    sequence: index,
                    message: index.to_string(),
                })
                .collect(),
        };
        let transformed = transform(context).unwrap();
        assert_eq!(transformed.history.len(), 200);
        assert_eq!(transformed.history[0].message, "5");
        assert_eq!(transformed.prompt.len(), 1);
        match &transformed.prompt[0] {
            ContentPart::Text(text) => assert_eq!(text, "current"),
            other => panic!("unexpected prompt part: {other:?}"),
        }
        assert_eq!(transformed.context, "system\n\n[context-plugin]");
    }
}
