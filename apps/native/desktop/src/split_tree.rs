//! The legacy scene tree, independent of the workspace's session graph.

use std::collections::{HashMap, HashSet};

use matou_native_core::geometry::GeometryUpdate;
use serde_json::{Value, json};

const MIN_RATIO: f64 = 0.1;
const MAX_RATIO: f64 = 0.9;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

impl SplitDirection {
    /// The divider measures the pointer against its own split, including its origin.
    pub fn ratio_at(self, bounds: SplitRect, x: f64, y: f64) -> Option<f64> {
        let (offset, extent) = match self {
            Self::Horizontal => (x - bounds.x, bounds.width),
            Self::Vertical => (y - bounds.y, bounds.height),
        };
        if !offset.is_finite() || !extent.is_finite() || extent <= 0.0 {
            return None;
        }
        Some((offset / extent).clamp(MIN_RATIO, MAX_RATIO))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum SplitNode {
    Mount {
        id: String,
        mount_id: String,
        session_id: String,
    },
    Split {
        id: String,
        direction: SplitDirection,
        children: Vec<SplitNode>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SplitRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl SplitRect {
    fn center(self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

#[derive(Debug, PartialEq)]
pub struct SplitPane<'a> {
    pub mount_id: &'a str,
    pub session_id: &'a str,
    pub bounds: SplitRect,
}

#[derive(Debug, PartialEq)]
pub struct SplitDivider<'a> {
    pub node_id: &'a str,
    pub after_child: usize,
    pub direction: SplitDirection,
    pub parent_bounds: SplitRect,
    /// The trailing edge of the preceding child, where the divider is drawn.
    pub position: f64,
}

impl SplitDivider<'_> {
    pub fn drag_to(&self, x: f64, y: f64) -> Option<SplitRatioChange> {
        Some(SplitRatioChange {
            node_id: self.node_id.to_owned(),
            ratio: self.direction.ratio_at(self.parent_bounds, x, y)?,
        })
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct SplitLayout<'a> {
    pub panes: Vec<SplitPane<'a>>,
    pub dividers: Vec<SplitDivider<'a>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitRatioChange {
    pub node_id: String,
    pub ratio: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneDirection {
    Up,
    Down,
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneStep {
    Previous,
    Next,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SplitTree {
    scene_id: String,
    layout_revision: f64,
    root: SplitNode,
    ratios: HashMap<String, f64>,
}

impl SplitTree {
    /// Read the Electron scene snapshot (`scene`, `nodes`, `mounts`, `geometry`).
    /// `None` leaves the caller free to show the existing scene recovery surface.
    pub fn from_snapshot(snapshot: &Value) -> Option<Self> {
        let scene = snapshot.get("scene")?;
        let scene_id = scene.get("id")?.as_str()?;
        let nodes = snapshot.get("nodes")?.as_array()?;
        let mounts = snapshot.get("mounts")?.as_array()?;
        let mut by_id = HashMap::new();
        let mut children = HashMap::<&str, Vec<(&str, i64)>>::new();
        let mut inferred_root = None;
        for node in nodes {
            let id = node.get("id")?.as_str()?;
            let parent = node.get("parentNodeId").and_then(Value::as_str);
            let direction = match node.get("direction").and_then(Value::as_str) {
                Some("vertical") => SplitDirection::Vertical,
                _ => SplitDirection::Horizontal,
            };
            if by_id.insert(id, (parent, direction)).is_some() {
                return None;
            }
            if let Some(parent) = parent {
                let ordinal = node.get("ordinal").and_then(Value::as_i64).unwrap_or(0);
                children.entry(parent).or_default().push((id, ordinal));
            } else if inferred_root.is_none() {
                inferred_root = Some(id);
            }
        }
        // Check every node, including disconnected branches, before following child links.
        let mut visited = HashSet::new();
        let mut path = HashSet::new();
        for &start in by_id.keys() {
            let mut cursor = Some(start);
            while let Some(id) = cursor {
                if visited.contains(id) {
                    break;
                }
                if !path.insert(id) {
                    return None;
                }
                cursor = by_id.get(id).and_then(|(parent, _)| *parent);
            }
            visited.extend(path.drain());
        }
        for siblings in children.values_mut() {
            siblings.sort_by_key(|(_, ordinal)| *ordinal);
        }

        let mut mount_by_node = HashMap::new();
        let mut session_by_mount = HashMap::new();
        for mount in mounts {
            let mount_id = mount.get("id")?.as_str()?;
            let session_id = mount.get("sessionId")?.as_str()?;
            session_by_mount.insert(mount_id, session_id);
            if let Some(node) = mount.get("sceneNodeId").and_then(Value::as_str) {
                mount_by_node.insert(node, mount_id);
            }
        }

        let root_id = scene
            .get("rootNodeId")
            .and_then(Value::as_str)
            .or(inferred_root)?;
        let mut pending = vec![(root_id, false)];
        let mut built = HashMap::new();
        while let Some((id, expanded)) = pending.pop() {
            if let Some(&mount_id) = mount_by_node.get(id) {
                built.insert(
                    id,
                    SplitNode::Mount {
                        id: id.to_owned(),
                        mount_id: mount_id.to_owned(),
                        session_id: session_by_mount[mount_id].to_owned(),
                    },
                );
                continue;
            }
            let Some(siblings) = children.get(id) else {
                continue;
            };
            if !expanded {
                pending.push((id, true));
                pending.extend(siblings.iter().rev().map(|&(id, _)| (id, false)));
                continue;
            }
            let mut branches: Vec<_> = siblings
                .iter()
                .filter_map(|(id, _)| built.remove(id))
                .collect();
            let node = match branches.len() {
                0 => continue,
                1 => branches.pop().unwrap(),
                _ => SplitNode::Split {
                    id: id.to_owned(),
                    direction: by_id
                        .get(id)
                        .map_or(SplitDirection::Horizontal, |(_, direction)| *direction),
                    children: branches,
                },
            };
            built.insert(id, node);
        }

        let mut ratios = HashMap::new();
        for item in snapshot
            .get("geometry")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let node_id = item
                .get("ownerKey")
                .and_then(Value::as_str)
                .and_then(|owner| owner.strip_prefix("node:"));
            let ratio = item.pointer("/geometry/ratio").and_then(Value::as_f64);
            if let (Some(id), Some(ratio)) = (node_id, ratio)
                && (0.0..=1.0).contains(&ratio)
            {
                ratios.insert(id.to_owned(), ratio);
            }
        }
        Some(Self {
            scene_id: scene_id.to_owned(),
            layout_revision: scene
                .get("layoutRevision")
                .and_then(Value::as_f64)
                .unwrap_or(0.0),
            root: built.remove(root_id)?,
            ratios,
        })
    }

    pub fn root(&self) -> &SplitNode {
        &self.root
    }

    pub fn ratio(&self, node_id: &str) -> Option<f64> {
        self.ratios.get(node_id).copied()
    }

    fn child_share(&self, node_id: &str, index: usize, count: usize) -> f64 {
        match self.ratio(node_id) {
            Some(ratio) if index == 0 => ratio,
            Some(ratio) => (1.0 - ratio) / (count - 1) as f64,
            None => 1.0 / count as f64,
        }
    }

    pub fn layout(&self, bounds: SplitRect) -> SplitLayout<'_> {
        self.calculate_layout(bounds, false)
    }

    fn calculate_layout(&self, bounds: SplitRect, for_navigation: bool) -> SplitLayout<'_> {
        let mut result = SplitLayout::default();
        let mut pending = vec![(&self.root, bounds)];
        while let Some((node, bounds)) = pending.pop() {
            match node {
                SplitNode::Mount {
                    mount_id,
                    session_id,
                    ..
                } => {
                    result.panes.push(SplitPane {
                        mount_id,
                        session_id,
                        bounds,
                    });
                }
                SplitNode::Split {
                    id,
                    direction,
                    children,
                } => {
                    let horizontal = *direction == SplitDirection::Horizontal;
                    let start = if horizontal { bounds.x } else { bounds.y };
                    let extent = if horizontal {
                        bounds.width
                    } else {
                        bounds.height
                    };
                    let count = children.len();
                    let mut offset = 0.0;
                    let first_child = pending.len();
                    for (index, child) in children.iter().enumerate() {
                        // Frozen directional navigation uses equal shares for non-binary nodes.
                        let share = if for_navigation && count != 2 {
                            1.0 / count as f64
                        } else {
                            self.child_share(id, index, count)
                        };
                        let size = extent * share;
                        let child_bounds = if horizontal {
                            SplitRect {
                                x: start + offset,
                                width: size,
                                ..bounds
                            }
                        } else {
                            SplitRect {
                                y: start + offset,
                                height: size,
                                ..bounds
                            }
                        };
                        offset += size;
                        pending.push((child, child_bounds));
                        if !for_navigation && index + 1 < count {
                            result.dividers.push(SplitDivider {
                                node_id: id,
                                after_child: index,
                                direction: *direction,
                                parent_bounds: bounds,
                                position: start + offset,
                            });
                        }
                    }
                    pending[first_child..].reverse();
                }
            }
        }
        result
    }

    /// Apply a live divider change and return the existing native geometry wire format.
    /// The caller owns write scheduling and supplies the timestamp.
    pub fn apply_ratio(&mut self, change: SplitRatioChange, now: i64) -> Option<GeometryUpdate> {
        if !(MIN_RATIO..=MAX_RATIO).contains(&change.ratio)
            || self.ratio(&change.node_id) == Some(change.ratio)
        {
            return None;
        }
        let mut pending = vec![&self.root];
        while let Some(node) = pending.pop() {
            if let SplitNode::Split { id, children, .. } = node {
                if *id == change.node_id {
                    let owner_key = format!("node:{id}");
                    self.ratios.insert(change.node_id, change.ratio);
                    return Some(GeometryUpdate {
                        scene_id: self.scene_id.clone(),
                        owner_key,
                        layout_revision: self.layout_revision,
                        geometry: json!({ "ratio": change.ratio }),
                        now,
                    });
                }
                pending.extend(children);
            }
        }
        None
    }

    pub fn ordered_sessions(&self) -> Vec<&str> {
        let mut ordered = Vec::new();
        let mut pending = vec![&self.root];
        while let Some(node) = pending.pop() {
            match node {
                SplitNode::Mount { session_id, .. } => ordered.push(session_id.as_str()),
                SplitNode::Split { children, .. } => pending.extend(children.iter().rev()),
            }
        }
        ordered
    }

    pub fn adjacent_session(&self, current: Option<&str>, step: PaneStep) -> Option<&str> {
        let ordered = self.ordered_sessions();
        let count = ordered.len();
        if count <= 1 {
            return None;
        }
        let index = ordered
            .iter()
            .position(|id| Some(*id) == current)
            .unwrap_or(0);
        let next = match step {
            PaneStep::Previous => (index + count - 1) % count,
            PaneStep::Next => (index + 1) % count,
        };
        Some(ordered[next])
    }

    pub fn directional_session(&self, current: &str, direction: PaneDirection) -> Option<&str> {
        let bounds = SplitRect {
            width: 1.0,
            height: 1.0,
            ..SplitRect::default()
        };
        let panes = self.calculate_layout(bounds, true).panes;
        let (origin_x, origin_y) = panes
            .iter()
            .find(|pane| pane.session_id == current)?
            .bounds
            .center();
        let mut nearest = None;
        let mut best = f64::INFINITY;
        for pane in panes.into_iter().filter(|pane| pane.session_id != current) {
            let (x, y) = pane.bounds.center();
            let (dx, dy) = (x - origin_x, y - origin_y);
            let (primary, cross) = match direction {
                PaneDirection::Up => (-dy, dx),
                PaneDirection::Down => (dy, dx),
                PaneDirection::Left => (-dx, dy),
                PaneDirection::Right => (dx, dy),
            };
            let score = primary * 4.0 + cross.abs();
            if primary > 0.0 && score < best {
                best = score;
                nearest = Some(pane.session_id);
            }
        }
        nearest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VIEW: SplitRect = SplitRect {
        x: 10.0,
        y: 20.0,
        width: 1000.0,
        height: 600.0,
    };

    fn snapshot() -> Value {
        json!({
            "scene": { "id": "scene-a", "rootNodeId": "root", "layoutRevision": 7 },
            "nodes": [
                { "id": "root", "kind": "root" },
                { "id": "right", "parentNodeId": "root", "ordinal": 20, "direction": "vertical" },
                { "id": "lower", "parentNodeId": "right", "ordinal": 3 },
                { "id": "left", "parentNodeId": "root", "ordinal": 10 },
                { "id": "upper", "parentNodeId": "right", "ordinal": 1 }
            ],
            "mounts": [
                { "id": "mount-c", "sceneNodeId": "lower", "sessionId": "c" },
                { "id": "mount-a", "sceneNodeId": "left", "sessionId": "a" },
                { "id": "mount-b", "sceneNodeId": "upper", "sessionId": "b" },
                { "id": "detached", "sceneWindowId": "window-a", "sessionId": "away" }
            ],
            "geometry": [
                { "ownerKey": "node:root", "geometry": { "ratio": 0.6 } },
                { "ownerKey": "node:right", "geometry": { "ratio": 0.25 } },
                { "ownerKey": "mount:mount-c", "geometry": { "ratio": 0.9 } }
            ]
        })
    }

    #[test]
    fn mixed_splits_respect_ordinals_offsets_and_ratios() {
        let tree = SplitTree::from_snapshot(&snapshot()).unwrap();
        let layout = tree.layout(VIEW);
        assert_eq!(
            layout.panes,
            [
                SplitPane {
                    mount_id: "mount-a",
                    session_id: "a",
                    bounds: SplitRect {
                        width: 600.0,
                        ..VIEW
                    },
                },
                SplitPane {
                    mount_id: "mount-b",
                    session_id: "b",
                    bounds: SplitRect {
                        x: 610.0,
                        y: 20.0,
                        width: 400.0,
                        height: 150.0
                    },
                },
                SplitPane {
                    mount_id: "mount-c",
                    session_id: "c",
                    bounds: SplitRect {
                        x: 610.0,
                        y: 170.0,
                        width: 400.0,
                        height: 450.0
                    },
                },
            ]
        );
        assert_eq!(
            layout.dividers,
            [
                SplitDivider {
                    node_id: "root",
                    after_child: 0,
                    direction: SplitDirection::Horizontal,
                    parent_bounds: VIEW,
                    position: 610.0,
                },
                SplitDivider {
                    node_id: "right",
                    after_child: 0,
                    direction: SplitDirection::Vertical,
                    parent_bounds: SplitRect {
                        x: 610.0,
                        width: 400.0,
                        ..VIEW
                    },
                    position: 170.0,
                },
            ]
        );
    }

    #[test]
    fn dragging_clamps_both_axes_and_only_changes_the_dragged_split() {
        let mut tree = SplitTree::from_snapshot(&snapshot()).unwrap();
        let layout = tree.layout(VIEW);
        let middle = &layout.dividers[1];
        assert_eq!(
            layout.dividers[0].drag_to(5_000.0, 10.0).unwrap().ratio,
            0.9
        );
        assert_eq!(middle.drag_to(10.0, 260.0).unwrap().ratio, 0.4);
        assert_eq!(middle.drag_to(10.0, 5_000.0).unwrap().ratio, 0.9);
        let change = middle.drag_to(10.0, -20.0).unwrap();
        assert_eq!(change.ratio, 0.1);
        assert!(tree.apply_ratio(change.clone(), 123).is_some());
        assert!(tree.apply_ratio(change, 124).is_none());
        assert_eq!(tree.ratio("root"), Some(0.6));
        assert_eq!(tree.layout(VIEW).panes[1].bounds.height, 60.0);
        assert_eq!(tree.layout(VIEW).panes[2].bounds.height, 540.0);
        assert!(
            SplitDirection::Vertical
                .ratio_at(SplitRect::default(), 0.0, 0.0)
                .is_none()
        );
        assert!(
            SplitDirection::Horizontal
                .ratio_at(VIEW, f64::NAN, 0.0)
                .is_none()
        );
        assert!(
            tree.apply_ratio(
                SplitRatioChange {
                    node_id: "left".into(),
                    ratio: 0.3
                },
                124
            )
            .is_none()
        );
    }

    #[test]
    fn persisted_geometry_round_trips_without_losing_tree_or_ancestor_ratio() {
        let mut legacy = snapshot();
        let mut tree = SplitTree::from_snapshot(&legacy).unwrap();
        let change = tree.layout(VIEW).dividers[1].drag_to(610.0, 220.0).unwrap();
        let update = tree.apply_ratio(change, 12345).unwrap();
        let serialized = serde_json::to_value(&update).unwrap();
        assert_eq!(
            serialized,
            json!({
                "sceneId": "scene-a", "ownerKey": "node:right", "layoutRevision": 7.0,
                "geometry": { "ratio": 1.0 / 3.0 }, "now": 12345
            })
        );
        assert_eq!(
            serde_json::from_value::<GeometryUpdate>(serialized.clone()).unwrap(),
            update
        );
        legacy["geometry"][1] = serialized;
        let restored = serde_json::from_str(&serde_json::to_string(&legacy).unwrap()).unwrap();
        assert_eq!(SplitTree::from_snapshot(&restored), Some(tree));
    }

    #[test]
    fn legacy_mounts_collapse_unary_and_empty_nodes_and_ignore_detached_sessions() {
        let mut legacy = snapshot();
        legacy["scene"]
            .as_object_mut()
            .unwrap()
            .remove("rootNodeId");
        legacy["mounts"]
            .as_array_mut()
            .unwrap()
            .retain(|mount| mount["id"] != "mount-b");
        let tree = SplitTree::from_snapshot(&legacy).unwrap();
        assert_eq!(tree.ordered_sessions(), ["a", "c"]);
        let layout = tree.layout(VIEW);
        assert_eq!(layout.dividers.len(), 1);
        assert_eq!(layout.panes[1].mount_id, "mount-c");
        assert_eq!(layout.panes[1].bounds.height, VIEW.height);

        legacy["mounts"].as_array_mut().unwrap().push(json!({
            "id": "mount-wrapper", "sceneNodeId": "right", "sessionId": "wrapper"
        }));
        assert_eq!(
            SplitTree::from_snapshot(&legacy)
                .unwrap()
                .ordered_sessions(),
            ["a", "wrapper"]
        );
    }

    #[test]
    fn invalid_trees_allow_the_caller_to_fall_back_to_scene_recovery() {
        let mut legacy = snapshot();
        legacy["scene"]["rootNodeId"] = json!("missing");
        assert!(SplitTree::from_snapshot(&legacy).is_none());

        let mut legacy = snapshot();
        let nodes = legacy["nodes"].as_array_mut().unwrap();
        nodes.push(json!({ "id": "cycle-a", "parentNodeId": "cycle-b" }));
        nodes.push(json!({ "id": "cycle-b", "parentNodeId": "cycle-a" }));
        assert!(SplitTree::from_snapshot(&legacy).is_none());

        let mut legacy = snapshot();
        legacy["nodes"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "id": "root" }));
        assert!(SplitTree::from_snapshot(&legacy).is_none());

        let mut legacy = snapshot();
        legacy["mounts"] = json!([{ "id": "detached", "sessionId": "away" }]);
        assert!(SplitTree::from_snapshot(&legacy).is_none());
    }

    #[test]
    fn focus_wraps_in_tree_order_and_uses_the_frozen_directional_scores() {
        let tree = SplitTree::from_snapshot(&snapshot()).unwrap();
        assert_eq!(tree.ordered_sessions(), ["a", "b", "c"]);
        assert_eq!(tree.adjacent_session(Some("c"), PaneStep::Next), Some("a"));
        assert_eq!(
            tree.adjacent_session(Some("a"), PaneStep::Previous),
            Some("c")
        );
        assert_eq!(tree.adjacent_session(None, PaneStep::Next), Some("b"));
        assert_eq!(
            tree.directional_session("a", PaneDirection::Right),
            Some("c")
        );
        assert_eq!(
            tree.directional_session("b", PaneDirection::Down),
            Some("a") // Equal scores keep the earlier leaf in tree order.
        );
        assert_eq!(tree.directional_session("c", PaneDirection::Up), Some("a"));
        assert_eq!(
            tree.directional_session("c", PaneDirection::Left),
            Some("a")
        );
        assert_eq!(tree.directional_session("a", PaneDirection::Left), None);
        assert_eq!(tree.directional_session("away", PaneDirection::Left), None);
    }

    #[test]
    fn multi_child_split_uses_one_ratio_for_its_first_child_in_a_deeper_tree() {
        let mut legacy = snapshot();
        let nodes = legacy["nodes"].as_array_mut().unwrap();
        nodes[2]["direction"] = json!("horizontal");
        nodes.extend([
            json!({ "id": "d", "parentNodeId": "root", "ordinal": 30 }),
            json!({ "id": "inner-first", "parentNodeId": "lower", "ordinal": 1 }),
            json!({ "id": "inner-second", "parentNodeId": "lower", "ordinal": 2 }),
        ]);
        let mounts = legacy["mounts"].as_array_mut().unwrap();
        mounts[0]["sceneNodeId"] = json!("inner-first");
        mounts.extend([
            json!({ "id": "mount-d", "sceneNodeId": "d", "sessionId": "d" }),
            json!({ "id": "mount-e", "sceneNodeId": "inner-second", "sessionId": "e" }),
        ]);
        let tree = SplitTree::from_snapshot(&legacy).unwrap();
        assert_eq!(tree.ordered_sessions(), ["a", "b", "c", "e", "d"]);
        let layout = tree.layout(VIEW);
        assert_eq!(layout.dividers[1].after_child, 1);
        assert_eq!(layout.dividers[1].position, 810.0);
        assert_eq!(
            layout.panes[2].bounds,
            SplitRect {
                x: 610.0,
                y: 170.0,
                width: 100.0,
                height: 450.0
            }
        );
        assert_eq!(
            layout.panes[4].bounds,
            SplitRect {
                x: 810.0,
                width: 200.0,
                ..VIEW
            }
        );
    }
}

mod ui {
    use std::{cell::Cell, rc::Rc};

    use gpui::{prelude::*, *};

    use super::*;

    type RatioHandler = dyn Fn(SplitRatioChange, &mut Window, &mut App);

    const DIVIDER_LINE: f32 = 1.;
    const DIVIDER_HIT_INSET: f32 = 3.;
    const DIVIDER_HIT_SIZE: f32 = DIVIDER_LINE + DIVIDER_HIT_INSET * 2.;

    #[derive(IntoElement)]
    struct SplitDividers {
        key: SharedString,
        node_id: String,
        direction: SplitDirection,
        positions: Vec<f32>,
        current_ratio: f64,
        color: Hsla,
        on_ratio: Rc<RatioHandler>,
    }

    impl RenderOnce for SplitDividers {
        fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
            // Keep the press across repaints as changing ratios move the hit targets.
            let pressed = window
                .use_keyed_state(self.key.clone(), cx, |_, _| Rc::new(Cell::new(false)))
                .read(cx)
                .clone();
            let capture = pressed.clone();
            let listeners = canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let reset = capture.clone();
                    window.on_mouse_event(move |event: &MouseDownEvent, phase, _, _| {
                        if phase.capture() && event.button == MouseButton::Left {
                            reset.set(false);
                        }
                    });
                    let release = capture.clone();
                    window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                        if phase.capture()
                            && event.button == MouseButton::Left
                            && release.replace(false)
                        {
                            cx.stop_propagation();
                        }
                    });
                    let parent = SplitRect {
                        x: f64::from(bounds.origin.x),
                        y: f64::from(bounds.origin.y),
                        width: f64::from(bounds.size.width),
                        height: f64::from(bounds.size.height),
                    };
                    let previous = Cell::new(self.current_ratio);
                    // Capture also sees movement outside this split and its divider.
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if !phase.capture() || !capture.get() {
                            return;
                        }
                        if event.pressed_button != Some(MouseButton::Left) {
                            capture.set(false);
                            return;
                        }
                        cx.stop_propagation();
                        let (x, y) = (f64::from(event.position.x), f64::from(event.position.y));
                        if let Some(ratio) = self.direction.ratio_at(parent, x, y)
                            && previous.replace(ratio) != ratio
                        {
                            (self.on_ratio)(
                                SplitRatioChange {
                                    node_id: self.node_id.clone(),
                                    ratio,
                                },
                                window,
                                cx,
                            );
                        }
                    });
                },
            )
            .absolute()
            .inset_0()
            .size_full();

            let horizontal = self.direction == SplitDirection::Horizontal;
            div()
                .absolute()
                .inset_0()
                .size_full()
                .child(listeners)
                .children(
                    self.positions
                        .into_iter()
                        .enumerate()
                        .map(|(index, position)| {
                            let key: SharedString = format!("{}:{index}", self.key).into();
                            let pressed = pressed.clone();
                            let hit = div()
                                .absolute()
                                .group(key.clone())
                                .occlude()
                                .cursor(if horizontal {
                                    CursorStyle::ResizeColumn
                                } else {
                                    CursorStyle::ResizeRow
                                })
                                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                                    pressed.set(true);
                                    window.prevent_default();
                                    cx.stop_propagation();
                                });
                            let line = div()
                                .absolute()
                                .bg(self.color)
                                .group_hover(key, |style| style.bg(rgb(0x58a6ff)));
                            // Place the line in the preceding child's last pixel. The
                            // wider hit target extends across the boundary.
                            if horizontal {
                                hit.top_0()
                                    .bottom_0()
                                    .left(relative(position))
                                    .ml(px(-DIVIDER_LINE - DIVIDER_HIT_INSET))
                                    .w(px(DIVIDER_HIT_SIZE))
                                    .child(
                                        line.top_0()
                                            .bottom_0()
                                            .left(px(DIVIDER_HIT_INSET))
                                            .w(px(DIVIDER_LINE)),
                                    )
                            } else {
                                hit.left_0()
                                    .right_0()
                                    .top(relative(position))
                                    .mt(px(-DIVIDER_LINE - DIVIDER_HIT_INSET))
                                    .h(px(DIVIDER_HIT_SIZE))
                                    .child(
                                        line.left_0()
                                            .right_0()
                                            .top(px(DIVIDER_HIT_INSET))
                                            .h(px(DIVIDER_LINE)),
                                    )
                            }
                        }),
                )
        }
    }

    fn render_node(
        tree: &SplitTree,
        node: &SplitNode,
        instance: &SharedString,
        divider_color: Hsla,
        render_mount: &mut impl FnMut(&str, &str) -> AnyElement,
        on_ratio: &Rc<RatioHandler>,
    ) -> AnyElement {
        match node {
            SplitNode::Mount {
                mount_id,
                session_id,
                ..
            } => div()
                .size_full()
                .min_w_0()
                .min_h_0()
                .child(render_mount(mount_id, session_id))
                .into_any_element(),
            SplitNode::Split {
                id,
                direction,
                children,
            } => {
                let horizontal = *direction == SplitDirection::Horizontal;
                let mut container = div()
                    .relative()
                    .flex()
                    .size_full()
                    .min_w_0()
                    .min_h_0()
                    .overflow_hidden()
                    .when(!horizontal, |container| container.flex_col());

                let count = children.len();
                let mut positions = Vec::with_capacity(count - 1);
                let mut offset = 0.0;
                for (index, child_node) in children.iter().enumerate() {
                    let share = tree.child_share(id, index, count);
                    let child = div().flex_none().min_w_0().min_h_0().overflow_hidden();
                    let child = if horizontal {
                        child.w(relative(share as f32)).h_full()
                    } else {
                        child.h(relative(share as f32)).w_full()
                    };
                    container = container.child(child.child(render_node(
                        tree,
                        child_node,
                        instance,
                        divider_color,
                        render_mount,
                        on_ratio,
                    )));
                    offset += share;
                    if index + 1 < count {
                        positions.push(offset as f32);
                    }
                }

                container
                    .child(SplitDividers {
                        key: format!("split-dividers:{instance}:{id}").into(),
                        node_id: id.clone(),
                        direction: *direction,
                        positions,
                        current_ratio: tree.child_share(id, 0, count),
                        color: divider_color,
                        on_ratio: on_ratio.clone(),
                    })
                    .into_any_element()
            }
        }
    }

    /// Render one leaf per legacy mount and report live resize requests to the host.
    /// Supply a stable instance ID and apply each change to the model before redrawing.
    pub fn split_tree_view(
        tree: &SplitTree,
        instance: SharedString,
        divider_color: Hsla,
        mut render_mount: impl FnMut(&str, &str) -> AnyElement,
        on_ratio: impl Fn(SplitRatioChange, &mut Window, &mut App) + 'static,
    ) -> Div {
        let on_ratio: Rc<RatioHandler> = Rc::new(on_ratio);
        div().size_full().min_w_0().min_h_0().child(render_node(
            tree,
            tree.root(),
            &instance,
            divider_color,
            &mut render_mount,
            &on_ratio,
        ))
    }
}

pub use ui::split_tree_view;
