use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct CanvasAccessibilityTree {
    pub nodes: BTreeMap<String, CanvasAccessibilityNode>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct CanvasAccessibilityNode {
    pub id: String,
    pub role: String,
    pub name: String,
    pub value: Option<String>,
    pub children: Vec<String>,
}

impl CanvasAccessibilityTree {
    /// Apply either a full CEF AX update or a location-only patch. Chromium
    /// keeps node ids stable for the browser lifetime, so incremental updates
    /// replace nodes without invalidating the surrounding GPUI tree.
    pub fn apply(&mut self, update: &Value) {
        visit(update, &mut |value| {
            let Some(object) = value.as_object() else {
                return;
            };
            let Some(id) = scalar(object.get("id")) else {
                return;
            };
            let role = property(object.get("role"));
            if role.is_empty() && !self.nodes.contains_key(&id) {
                return;
            }
            let previous = self.nodes.get(&id).cloned();
            self.nodes.insert(
                id.clone(),
                CanvasAccessibilityNode {
                    id,
                    role: nonempty(role, previous.as_ref().map(|node| node.role.clone())),
                    name: nonempty(
                        property(object.get("name")),
                        previous.as_ref().map(|node| node.name.clone()),
                    ),
                    value: optional_property(object.get("value"))
                        .or_else(|| previous.as_ref().and_then(|node| node.value.clone())),
                    children: object
                        .get("childIds")
                        .or_else(|| object.get("child_ids"))
                        .and_then(Value::as_array)
                        .map(|children| {
                            children
                                .iter()
                                .filter_map(|value| scalar(Some(value)))
                                .collect()
                        })
                        .or_else(|| previous.map(|node| node.children))
                        .unwrap_or_default(),
                },
            );
        });
    }
}

fn visit(value: &Value, visitor: &mut impl FnMut(&Value)) {
    visitor(value);
    match value {
        Value::Array(values) => values.iter().for_each(|value| visit(value, visitor)),
        Value::Object(values) => values.values().for_each(|value| visit(value, visitor)),
        _ => {}
    }
}

fn scalar(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn property(value: Option<&Value>) -> String {
    optional_property(value).unwrap_or_default()
}

fn optional_property(value: Option<&Value>) -> Option<String> {
    let value = value?;
    scalar(Some(value)).or_else(|| {
        value
            .as_object()
            .and_then(|object| object.get("value"))
            .and_then(|value| scalar(Some(value)))
    })
}

fn nonempty(value: String, fallback: Option<String>) -> String {
    if value.is_empty() {
        fallback.unwrap_or_default()
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_and_incremental_updates_preserve_stable_nodes() {
        let mut tree = CanvasAccessibilityTree::default();
        tree.apply(&serde_json::json!({"updates":[{"nodes":[
            {"id":1,"role":{"value":"button"},"name":{"value":"Save"},"childIds":[2]},
            {"id":2,"role":{"value":"StaticText"},"name":{"value":"now"}}
        ]}]}));
        tree.apply(&serde_json::json!({"nodes":[
            {"id":1,"name":{"value":"Save changes"}}
        ]}));
        assert_eq!(tree.nodes["1"].role, "button");
        assert_eq!(tree.nodes["1"].name, "Save changes");
        assert_eq!(tree.nodes["1"].children, ["2"]);
    }
}
