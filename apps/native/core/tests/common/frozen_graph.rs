use serde_json::{Value, json};

/// The frozen fixtures predate permission projection, including graphs in their receipts.
pub fn apply_permission_default(value: &mut Value) {
    let graph = if value.get("graph").is_some() {
        &mut value["graph"]
    } else {
        value
    };
    if let Some(nodes) = graph.get_mut("nodes").and_then(Value::as_array_mut) {
        for node in nodes {
            node.as_object_mut()
                .expect("session graph node")
                .entry("permissionMode")
                .or_insert_with(|| json!("default"));
        }
    }
}
