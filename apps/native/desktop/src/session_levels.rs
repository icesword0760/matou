use serde_json::Value;

pub(super) fn nodes(graph: &Value) -> &[Value] {
    graph["nodes"].as_array().map_or(&[], Vec::as_slice)
}

pub(super) fn node<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
    nodes
        .iter()
        .find(|node| node["sessionId"].as_str() == Some(id))
}

pub(super) fn parent_id(node: &Value) -> Option<&str> {
    node["parentSessionId"].as_str()
}

pub(super) fn children_of<'a>(
    nodes: &'a [Value],
    parent: Option<&str>,
) -> impl Iterator<Item = &'a Value> {
    nodes.iter().filter(move |node| parent_id(node) == parent)
}

pub(super) fn level_parent<'a>(
    nodes: &'a [Value],
    explicit_parent: Option<Option<&'a str>>,
    focused_id: Option<&str>,
) -> Option<&'a str> {
    explicit_parent
        .unwrap_or_else(|| {
            focused_id
                .and_then(|id| node(nodes, id))
                .or_else(|| nodes.first())
                .and_then(parent_id)
        })
        .filter(|id| node(nodes, id).is_some())
}

pub(super) fn preferred_child<'a>(children: impl Iterator<Item = &'a Value>) -> Option<&'a str> {
    children
        .filter(|node| node["archivedAt"].is_null())
        .filter_map(|node| Some((node["sessionId"].as_str()?, node["workStatus"].as_str())))
        .min_by_key(|&(_, status)| match status {
            Some("error") => 0,
            Some("needs-input") => 1,
            Some("running" | "starting") => 2,
            Some("interrupted") => 3,
            Some("idle") => 4,
            Some("exited") => 5,
            _ => 6,
        })
        .map(|(id, _)| id)
}

pub(super) fn owner_key(scene_id: &str, parent: Option<&str>) -> String {
    format!("session-group:{scene_id}:{}", parent.unwrap_or("root"))
}

pub(super) struct ChildBadge {
    pub(super) summary: String,
    pub(super) detail: String,
    pub(super) dot_color: u32,
    pub(super) is_idle: bool,
}

pub(super) fn child_badge<'a>(children: impl Iterator<Item = &'a Value>) -> Option<ChildBadge> {
    // Mode order follows the launchable agent manifest. Status entries are
    // ordered by summary priority, with a separate display order for detail.
    let mut modes = [
        ("claude-code", "Claude", 0),
        ("codex", "Codex", 0),
        ("antigravity", "Antigravity", 0),
        ("shell", "Shell", 0),
    ];
    let mut statuses = [
        ("starting", "准备中", "准备中", 0x9298a2, 0),
        ("running", "运行中", "运行中", 0x43b66e, 0),
        ("needs-input", "待输入", "待输入", 0xe7a535, 0),
        ("error", "错误", "异常", 0xe45d55, 0),
    ];
    let mut count = 0;
    for child in children {
        count += 1;
        if let Some((_, _, count)) = modes
            .iter_mut()
            .find(|(id, _, _)| child["currentMode"].as_str() == Some(id))
        {
            *count += 1;
        }
        if let Some((_, _, _, _, count)) = statuses
            .iter_mut()
            .find(|(id, _, _, _, _)| child["workStatus"].as_str() == Some(id))
        {
            *count += 1;
        }
    }
    if count == 0 {
        return None;
    }

    let highest = statuses.iter().rfind(|(_, _, _, _, count)| *count != 0);
    let mut summary = format!("{count} 分支");
    if let Some((_, _, label, _, count)) = highest {
        summary.push_str(&format!(" · {count} {label}"));
    }
    let mut detail = modes
        .iter()
        .filter(|(id, _, count)| *count != 0 || matches!(*id, "claude-code" | "shell"))
        .map(|(_, name, count)| format!("{name} {count}"))
        .collect::<Vec<_>>()
        .join(" · ");
    let status_detail = [1, 0, 2, 3]
        .into_iter()
        .filter_map(|index| {
            let (_, label, _, _, count) = statuses[index];
            (count != 0).then(|| format!("{label} {count}"))
        })
        .collect::<Vec<_>>()
        .join(" · ");
    if !status_detail.is_empty() {
        detail.push('；');
        detail.push_str(&status_detail);
    }
    Some(ChildBadge {
        summary,
        detail,
        dot_color: highest.map_or(0x9298a2, |(_, _, _, color, _)| *color),
        is_idle: highest.is_none(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn levels_keep_graph_order_and_archived_children() {
        let graph = json!({"nodes": [
            {"sessionId": "second", "parentSessionId": "parent", "archivedAt": 1},
            {"sessionId": "root-b"},
            {"sessionId": "first", "parentSessionId": "parent"},
            {"sessionId": "parent"},
            {"sessionId": "grandchild", "parentSessionId": "first"},
            {"sessionId": "orphan", "parentSessionId": "missing"},
        ]});
        let nodes = nodes(&graph);
        let order = |parent| {
            children_of(nodes, parent)
                .map(|node| node["sessionId"].as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(order(None), ["root-b", "parent"]);
        assert_eq!(order(Some("parent")), ["second", "first"]);
        assert_eq!(order(Some("first")), ["grandchild"]);
        assert!(order(Some("grandchild")).is_empty());
        assert_eq!(level_parent(nodes, None, Some("first")), Some("parent"));
        assert_eq!(level_parent(nodes, None, None), Some("parent"));
        assert_eq!(level_parent(nodes, Some(None), Some("first")), None);
        assert_eq!(
            level_parent(nodes, Some(Some("first")), Some("root-b")),
            Some("first")
        );
        assert_eq!(level_parent(nodes, Some(Some("missing")), None), None);
        assert_eq!(level_parent(nodes, None, Some("orphan")), None);
        assert_eq!(owner_key("scene", None), "session-group:scene:root");
        assert_eq!(
            owner_key("scene", Some("parent")),
            "session-group:scene:parent"
        );
    }

    #[test]
    fn badge_counts_direct_modes_and_statuses_in_frozen_order() {
        let graph = json!({"nodes": [
            {"sessionId": "parent"},
            {"parentSessionId": "parent", "currentMode": "claude-code", "workStatus": "needs-input"},
            {"parentSessionId": "parent", "currentMode": "antigravity", "workStatus": "running"},
            {"parentSessionId": "parent", "currentMode": "codex", "workStatus": "starting"},
            {"parentSessionId": "parent", "currentMode": "shell", "workStatus": "error", "archivedAt": 1},
            {"parentSessionId": "parent", "currentMode": "agent-team-member", "workStatus": "error"},
            {"parentSessionId": "grandparent", "currentMode": "shell", "workStatus": "running"},
        ]});
        let nodes = nodes(&graph);
        let badge = child_badge(children_of(nodes, Some("parent"))).unwrap();
        assert_eq!(badge.summary, "5 分支 · 2 异常");
        assert_eq!(
            badge.detail,
            "Claude 1 · Codex 1 · Antigravity 1 · Shell 1；运行中 1 · 准备中 1 · 待输入 1 · 错误 2"
        );
        assert_eq!(badge.dot_color, 0xe45d55);
        assert!(!badge.is_idle);
        let statuses = ["interrupted", "starting", "running", "needs-input", "error"];
        for (index, (summary, color)) in [
            ("1 分支", 0x9298a2),
            ("2 分支 · 1 准备中", 0x9298a2),
            ("3 分支 · 1 运行中", 0x43b66e),
            ("4 分支 · 1 待输入", 0xe7a535),
            ("5 分支 · 1 异常", 0xe45d55),
        ]
        .into_iter()
        .enumerate()
        {
            let children = statuses[..=index]
                .iter()
                .map(|status| json!({"currentMode": "shell", "workStatus": status}))
                .collect::<Vec<_>>();
            let badge = child_badge(children.iter()).unwrap();
            assert_eq!((badge.summary.as_str(), badge.dot_color), (summary, color));
            assert_eq!(badge.is_idle, index == 0);
            if index == 0 {
                assert_eq!(badge.detail, "Claude 0 · Shell 1");
            }
        }
        assert!(child_badge(children_of(nodes, Some("empty"))).is_none());
    }

    #[test]
    fn preferred_child_is_stable_and_skips_archived_nodes() {
        let children = [
            json!({"sessionId": "archived", "workStatus": "error", "archivedAt": 1}),
            json!({"sessionId": "exited", "workStatus": "exited"}),
            json!({"sessionId": "idle", "workStatus": "idle"}),
            json!({"sessionId": "interrupted", "workStatus": "interrupted"}),
            json!({"sessionId": "starting", "workStatus": "starting"}),
            json!({"sessionId": "running", "workStatus": "running"}),
            json!({"sessionId": "waiting", "workStatus": "needs-input"}),
            json!({"sessionId": "first-error", "workStatus": "error"}),
            json!({"sessionId": "second-error", "workStatus": "error"}),
        ];
        let expected = [
            None,
            Some("exited"),
            Some("idle"),
            Some("interrupted"),
            Some("starting"),
            Some("starting"),
            Some("waiting"),
            Some("first-error"),
            Some("first-error"),
        ];
        for (index, expected) in expected.into_iter().enumerate() {
            assert_eq!(preferred_child(children[..=index].iter()), expected);
        }
    }
}
