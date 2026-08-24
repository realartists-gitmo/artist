use artist_plugin_sdk::artist::plugin::types::Message;

struct Context;

fn transform(messages: Vec<Message>) -> Result<Vec<Message>, String> {
    let from = messages.len().saturating_sub(200);
    Ok(messages.into_iter().skip(from).collect())
}

artist_plugin_sdk::context_component!(Context, "artist.context", 0, transform);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_only_the_last_two_hundred_messages() {
        let messages = (0..205)
            .map(|index| Message {
                role: "user".into(),
                content: index.to_string(),
            })
            .collect();
        let transformed = transform(messages).unwrap();
        assert_eq!(transformed.len(), 200);
        assert_eq!(transformed[0].content, "5");
    }
}
