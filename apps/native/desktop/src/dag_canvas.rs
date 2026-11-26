//! Session graph layout, navigation, and embeddable native canvas.
//!
//! The frozen renderer is the coordinate and interaction reference. Runtime
//! transport, geometry persistence, and owner-window focus belong to the host.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
    ops::Range,
};

use serde_json::Value;

pub const MAX_LAYOUT_NODES: usize = 10_000;
pub const SEARCH_DISPLAY_LIMIT: usize = 12;

#[cfg(any(target_os = "windows", test))]
pub(crate) const DAG_WINDOWS_TOOLBAR_RIGHT_INSET: f32 =
    crate::windows_titlebar::WINDOWS_CAPTION_CONTROLS_WIDTH + 12.;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TOOLBAR_LEFT: f32 = 16.;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TOOLBAR_DRAG_MIN: f32 = 24.;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TOOLBAR_GAP: f32 = 12.;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_ZOOM_RESERVE: f32 = 164.;
#[cfg(any(target_os = "windows", test))]
const WINDOWS_LEGEND_RESERVE: f32 = 180.;

#[cfg(any(target_os = "windows", test))]
fn windows_toolbar_widths(viewport: f32) -> (f32, f32) {
    let available = (viewport
        - WINDOWS_TOOLBAR_LEFT
        - DAG_WINDOWS_TOOLBAR_RIGHT_INSET
        - WINDOWS_TOOLBAR_DRAG_MIN
        - WINDOWS_TOOLBAR_GAP
        - WINDOWS_ZOOM_RESERVE)
        .max(0.);
    let search = (viewport * 0.44)
        .clamp(180., 380.)
        .min((available - WINDOWS_LEGEND_RESERVE).max(0.));
    (search, available - search)
}

#[cfg(test)]
mod windows_toolbar_tests {
    use super::*;

    #[test]
    fn windows_toolbar_stays_outside_caption_controls() {
        assert_eq!(WINDOWS_TOOLBAR_LEFT, 16.);
        assert_eq!(DAG_WINDOWS_TOOLBAR_RIGHT_INSET, 150.);
        assert_eq!(windows_toolbar_widths(680.), (134., 180.));
        assert_eq!(windows_toolbar_widths(1440.).0, 380.);
        for viewport in [680., 720., 960., 1440.] {
            let (search, legend) = windows_toolbar_widths(viewport);
            assert!(
                search
                    + legend
                    + WINDOWS_ZOOM_RESERVE
                    + WINDOWS_TOOLBAR_GAP
                    + WINDOWS_TOOLBAR_DRAG_MIN
                    <= viewport - WINDOWS_TOOLBAR_LEFT - DAG_WINDOWS_TOOLBAR_RIGHT_INSET
            );
        }
    }
}

#[cfg(any(target_os = "macos", test))]
const MAC_TOOLBAR_LEFT: f32 = 80.;

#[cfg(any(target_os = "macos", test))]
fn mac_toolbar_widths(viewport: f32, right_inset: f32) -> (f32, f32) {
    // Leave room for the zoom controls and both gaps. The legend can stack its
    // entries in a narrow window while the search input stays large enough to use.
    let available = (viewport - MAC_TOOLBAR_LEFT - right_inset - 164. - 24.).max(0.);
    let search = (viewport * 0.44)
        .clamp(180., 380.)
        .min((available - 134.).max(0.));
    (search, available - search)
}

#[cfg(test)]
#[test]
fn mac_toolbar_reserves_search_space_at_narrow_viewports() {
    assert_eq!(mac_toolbar_widths(680., 56.).0, 222.);
    assert_eq!(mac_toolbar_widths(720., 56.), (262., 134.));
    assert_eq!(mac_toolbar_widths(900., 56.).0, 380.);
    assert_eq!(mac_toolbar_widths(1200., 56.).0, 380.);
}

const NODE_WIDTH: f64 = 260.;
const BASE_HEIGHT: f64 = 174.;
const X_GAP: f64 = 110.;
const Y_GAP: f64 = 26.;
const INSET: f64 = 50.;
const COLUMN_STEP: f64 = NODE_WIDTH + X_GAP;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DagPoint {
    pub x: f64,
    pub y: f64,
}

fn dag_wheel_pan(delta: gpui::ScrollDelta) -> DagPoint {
    // GPUI deltas are already in the view's direction, opposite to DOM wheel deltas.
    let delta = delta.pixel_delta(gpui::px(20.));
    DagPoint {
        x: f64::from(delta.x),
        y: f64::from(delta.y),
    }
}

fn dag_wheel_zoom_factor(dy: f64) -> f64 {
    (dy * 0.002).exp()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DagSize {
    pub width: f64,
    pub height: f64,
}

impl Default for DagSize {
    fn default() -> Self {
        Self {
            width: 1000.,
            height: 700.,
        }
    }
}

impl DagSize {
    pub fn center(self) -> DagPoint {
        DagPoint {
            x: self.width / 2.,
            y: self.height / 2.,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DagRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl DagRect {
    fn center_y(self) -> f64 {
        self.y + self.height / 2.
    }

    fn intersects(self, bounds: DagWorldBounds) -> bool {
        self.x + self.width >= bounds.left
            && self.x <= bounds.right
            && self.y + self.height >= bounds.top
            && self.y <= bounds.bottom
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DagTransform {
    pub x: f64,
    pub y: f64,
    pub scale: f64,
}

impl Default for DagTransform {
    fn default() -> Self {
        Self {
            x: 70.,
            y: 40.,
            scale: 1.,
        }
    }
}

impl DagTransform {
    pub fn zoom_at(self, scale: f64, anchor: DagPoint) -> Self {
        if !scale.is_finite() {
            return self;
        }
        let scale = scale.clamp(0.4, 2.);
        Self {
            x: anchor.x - (anchor.x - self.x) / self.scale * scale,
            y: anchor.y - (anchor.y - self.y) / self.scale * scale,
            scale,
        }
    }

    fn world(self, point: DagPoint) -> DagPoint {
        DagPoint {
            x: (point.x - self.x) / self.scale,
            y: (point.y - self.y) / self.scale,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DagError {
    InvalidRequest,
    MissingSceneId,
    InvalidWindowId,
    InvalidGraph(&'static str),
    InvalidNode(usize),
    InvalidEdge(usize),
    DuplicateNode(String),
    MissingEndpoint,
    InconsistentParent,
    Cycle,
    LayoutLimit { nodes: usize },
    WrongScene,
}

impl fmt::Display for DagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidRequest => f.write_str("RPC payload must be an object"),
            Self::MissingSceneId => f.write_str("sceneId must be a non-empty string"),
            Self::InvalidWindowId => f.write_str("windowId must be a non-empty string"),
            Self::InvalidGraph(field) => write!(f, "invalid DAG graph field {field}"),
            Self::InvalidNode(index) => write!(f, "invalid DAG session at index {index}"),
            Self::InvalidEdge(index) => write!(f, "invalid DAG relation at index {index}"),
            Self::DuplicateNode(id) => write!(f, "duplicate DAG session {id}"),
            Self::MissingEndpoint => f.write_str("relation endpoints must exist in the same Task"),
            Self::InconsistentParent => f.write_str("DAG parent and structural edge disagree"),
            Self::Cycle => f.write_str("creating structural relation would introduce a cycle"),
            Self::LayoutLimit { .. } => f.write_str("Maximum call stack size exceeded"),
            Self::WrongScene => f.write_str("DAG snapshot belongs to a different scene"),
        }
    }
}

impl std::error::Error for DagError {}

#[derive(Clone, Debug, Default)]
pub struct DagNode {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub sibling_created_seq: Option<i64>,
    pub title: String,
    pub cwd: String,
    pub current_mode: String,
    pub work_status: String,
    pub latest_lines: Vec<String>,
    pub archived: bool,
    pub branch: Option<String>,
    pub git_branch: Option<String>,
    pub git_dirty: bool,
    pub shared: bool,
    pub active_child_count: usize,
    pub stopped_child_count: usize,
    pub shell_child_count: usize,
    pub claude_code_child_count: usize,
    pub detached_window_id: Option<String>,
    pub last_activity_at: Option<i64>,
    pub last_user_interaction_seq: u64,
}

fn non_empty(value: &Value) -> Option<&str> {
    value
        .as_str()
        .filter(|value| !crate::rename_input::trim_name(value).is_empty())
}

fn string(value: &Value) -> Option<String> {
    value.as_str().map(str::to_owned)
}

fn count(value: &Value) -> usize {
    value
        .as_u64()
        .and_then(|count| count.try_into().ok())
        .unwrap_or(0)
}

impl DagNode {
    fn from_snapshot(value: &Value, index: usize, scene_id: &str) -> Result<Self, DagError> {
        let invalid = || DagError::InvalidNode(index);
        let session_id = non_empty(&value["sessionId"])
            .ok_or_else(invalid)?
            .to_owned();
        let title = string(&value["title"]).ok_or_else(invalid)?;
        let cwd = string(&value["cwd"]).ok_or_else(invalid)?;
        let parent_session_id = match value.get("parentSessionId") {
            None | Some(Value::Null) => None,
            Some(value) => Some(non_empty(value).ok_or_else(invalid)?.to_owned()),
        };
        if value.get("sceneId").is_some_and(|scene| scene != scene_id) {
            return Err(invalid());
        }
        let latest_lines = value["latestLines"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(string)
            .collect();
        Ok(Self {
            session_id,
            parent_session_id,
            sibling_created_seq: value["siblingCreatedSeq"].as_i64(),
            title,
            cwd,
            current_mode: value["currentMode"].as_str().unwrap_or("shell").into(),
            work_status: value["workStatus"].as_str().unwrap_or("idle").into(),
            latest_lines,
            archived: !value["archivedAt"].is_null(),
            branch: string(&value["worktree"]["branch"]),
            git_branch: string(&value["git"]["branch"]),
            git_dirty: value["git"]["dirty"] == true,
            shared: value["sharedWorkingDirectory"] == true || value["worktree"]["shared"] == true,
            active_child_count: count(&value["activeChildCount"]),
            stopped_child_count: count(&value["stoppedChildCount"]),
            shell_child_count: count(&value["childModeCounts"]["shell"]),
            claude_code_child_count: count(&value["childModeCounts"]["claudeCode"]),
            detached_window_id: string(&value["detachedWindowId"]),
            last_activity_at: value["lastActivityAt"].as_i64(),
            last_user_interaction_seq: value["lastUserInteractionSeq"].as_u64().unwrap_or(0),
        })
    }
}

#[derive(Clone, Debug)]
pub struct DagRelation {
    pub parent_session_id: String,
    pub child_session_id: String,
    pub relation_kind: String,
}

/// Coordinates the scene and the owner asking for its graph.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagScope {
    pub scene_id: String,
    pub window_id: Option<String>,
}

impl DagScope {
    pub fn from_payload(payload: &Value) -> Result<Self, DagError> {
        Self::parse(payload, "windowId")
    }

    pub fn from_context(context: &Value) -> Result<Self, DagError> {
        Self::parse(context, "mainWindowId")
    }

    fn parse(payload: &Value, window_key: &str) -> Result<Self, DagError> {
        let payload = payload.as_object().ok_or(DagError::InvalidRequest)?;
        let scene_id = payload
            .get("sceneId")
            .and_then(non_empty)
            .ok_or(DagError::MissingSceneId)?;
        let window_id = payload
            .get(window_key)
            .map(|value| non_empty(value).ok_or(DagError::InvalidWindowId))
            .transpose()?;
        let trim = |id| crate::rename_input::trim_name(id).to_owned();
        Ok(Self {
            scene_id: trim(scene_id),
            window_id: window_id.map(trim),
        })
    }

    pub fn initial_layout(&self, graph: &Value) -> Result<DagLayout, DagError> {
        DagLayout::new(DagGraph::from_initial_context(&self.scene_id, graph)?)
    }
}

#[derive(Clone, Debug)]
pub struct DagGraph {
    pub scene_id: String,
    pub focused_session_id: Option<String>,
    pub runtime_generation: Option<String>,
    pub layout_revision: Option<u64>,
    pub event_sequence: Option<u64>,
    pub nodes: Vec<DagNode>,
    pub edges: Vec<DagRelation>,
}

fn check_node_limit(nodes: usize) -> Result<(), DagError> {
    if nodes > MAX_LAYOUT_NODES {
        return Err(DagError::LayoutLimit { nodes });
    }
    Ok(())
}

impl DagGraph {
    pub fn from_snapshot(value: &Value) -> Result<Self, DagError> {
        let scene_id = non_empty(&value["sceneId"]).ok_or(DagError::InvalidGraph("sceneId"))?;
        let nodes = value["nodes"]
            .as_array()
            .ok_or(DagError::InvalidGraph("nodes"))?;
        let edges = value["edges"]
            .as_array()
            .ok_or(DagError::InvalidGraph("edges"))?;
        check_node_limit(nodes.len())?;
        if edges.len() > nodes.len() {
            return Err(DagError::InconsistentParent);
        }
        Ok(Self {
            scene_id: scene_id.to_owned(),
            focused_session_id: string(&value["focusedSessionId"]),
            runtime_generation: string(&value["runtimeGeneration"]),
            layout_revision: value["layoutRevision"].as_u64(),
            event_sequence: value["eventSequence"].as_u64(),
            nodes: nodes
                .iter()
                .enumerate()
                .map(|(index, node)| DagNode::from_snapshot(node, index, scene_id))
                .collect::<Result<_, _>>()?,
            edges: edges
                .iter()
                .enumerate()
                .map(|(index, edge)| {
                    let invalid = || DagError::InvalidEdge(index);
                    let parent_session_id = non_empty(&edge["parentSessionId"])
                        .ok_or_else(invalid)?
                        .to_owned();
                    let child_session_id = non_empty(&edge["childSessionId"])
                        .ok_or_else(invalid)?
                        .to_owned();
                    let relation_kind = match edge["relationKind"].as_str() {
                        Some(kind @ ("derived-from" | "forked-from")) => kind.to_owned(),
                        _ => return Err(invalid()),
                    };
                    Ok(DagRelation {
                        parent_session_id,
                        child_session_id,
                        relation_kind,
                    })
                })
                .collect::<Result<_, _>>()?,
        })
    }

    /// Validate a scene-scoped bootstrap value. An invalid bootstrap lets the
    /// host request a scoped snapshot, as in DagWindowApp.tsx:289–303.
    pub fn from_initial_context(scene_id: &str, value: &Value) -> Result<Self, DagError> {
        let decoded;
        let value = if let Some(encoded) = value.as_str() {
            decoded = serde_json::from_str(encoded)
                .map_err(|_| DagError::InvalidGraph("initialGraph"))?;
            &decoded
        } else {
            value
        };
        if value["sceneId"] != scene_id {
            return Err(DagError::WrongScene);
        }
        Self::from_snapshot(value)
    }
}

#[derive(Clone, Debug)]
pub struct DagLayoutNode {
    pub node: DagNode,
    pub depth: usize,
    pub rect: DagRect,
    parent_index: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct DagLayoutEdge {
    pub from_session_id: String,
    pub to_session_id: String,
    pub relation_kind: String,
    pub from: DagPoint,
    pub to: DagPoint,
}

#[derive(Debug)]
pub struct DagLayout {
    pub width: f64,
    pub height: f64,
    pub depth_count: usize,
    pub layout_revision: Option<u64>,
    pub nodes: Vec<DagLayoutNode>,
    pub edges: Vec<DagLayoutEdge>,
    by_id: HashMap<String, usize>,
    by_depth: Vec<Range<usize>>,
    scene_id: String,
    first_session_id: Option<String>,
    focused_session_id: Option<String>,
    runtime_generation: Option<String>,
    event_sequence: Option<u64>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Topology {
    Scene,
    Complete,
    Frozen,
}

impl DagLayout {
    /// Entry point for live projections. Invalid topologies never replace the
    /// last usable canvas.
    pub fn new(graph: DagGraph) -> Result<Self, DagError> {
        Self::build(graph, Topology::Scene)
    }

    /// Validate a whole graph whose parent and child endpoints must both exist.
    pub fn complete(graph: DagGraph) -> Result<Self, DagError> {
        Self::build(graph, Topology::Complete)
    }

    /// Reproduce the frozen renderer's coordinates for historical damaged
    /// projections. This is useful for fixture replay and diagnostics.
    pub fn frozen_coordinates(graph: DagGraph) -> Result<Self, DagError> {
        Self::build(graph, Topology::Frozen)
    }

    fn build(graph: DagGraph, topology: Topology) -> Result<Self, DagError> {
        check_node_limit(graph.nodes.len())?;
        let invalid_id = |id: &str| crate::rename_input::trim_name(id).is_empty();
        if invalid_id(&graph.scene_id) {
            return Err(DagError::InvalidGraph("sceneId"));
        }
        let mut by_id = HashMap::with_capacity(graph.nodes.len());
        for (index, node) in graph.nodes.iter().enumerate() {
            let id = node.session_id.as_str();
            if invalid_id(id) || node.parent_session_id.as_deref().is_some_and(invalid_id) {
                return Err(DagError::InvalidNode(index));
            }
            if by_id.insert(id, index).is_some() {
                return Err(DagError::DuplicateNode(id.to_owned()));
            }
        }
        let parents: Vec<_> = graph
            .nodes
            .iter()
            .map(|node| {
                node.parent_session_id
                    .as_deref()
                    .and_then(|id| by_id.get(id).copied())
            })
            .collect();
        if topology != Topology::Frozen {
            validate_parents(&graph, &by_id, &parents, topology)?;
        }
        let depths = walk_depths(&parents, topology != Topology::Frozen)?;
        let first_session_id = graph.nodes.first().map(|node| node.session_id.clone());
        let depth_count = depths.iter().max().map_or(1, |depth| depth + 1);
        let mut columns = vec![Vec::new(); depth_count];
        for (node, depth) in graph.nodes.into_iter().zip(depths) {
            let height = node_height(&node.title);
            columns[depth].push((node, height));
        }
        let heights: Vec<_> = columns
            .iter()
            .map(|column| {
                column.iter().map(|(_, height)| height).sum::<f64>()
                    + column.len().saturating_sub(1) as f64 * Y_GAP
            })
            .collect();
        let max_height = heights.iter().copied().fold(BASE_HEIGHT, f64::max);
        let mut nodes = Vec::with_capacity(parents.len());
        let mut by_depth = Vec::with_capacity(depth_count);
        for (depth, (mut column, height)) in columns.into_iter().zip(heights).enumerate() {
            column.sort_by(|(left, _), (right, _)| stable_node_order(left, right));
            let mut y = INSET + (max_height - height) / 2.;
            let x = INSET + depth as f64 * COLUMN_STEP;
            let start = nodes.len();
            for (node, height) in column {
                nodes.push(DagLayoutNode {
                    node,
                    depth,
                    rect: DagRect {
                        x,
                        y,
                        width: NODE_WIDTH,
                        height,
                    },
                    parent_index: None,
                });
                y += height + Y_GAP;
            }
            by_depth.push(start..nodes.len());
        }
        let by_id: HashMap<_, _> = nodes
            .iter()
            .enumerate()
            .map(|(index, node)| (node.node.session_id.clone(), index))
            .collect();
        for positioned in &mut nodes {
            positioned.parent_index = positioned
                .node
                .parent_session_id
                .as_deref()
                .and_then(|parent| by_id.get(parent).copied());
        }
        let edges = graph
            .edges
            .into_iter()
            .filter_map(|edge| {
                let parent = nodes.get(*by_id.get(&edge.parent_session_id)?)?.rect;
                let child = nodes.get(*by_id.get(&edge.child_session_id)?)?.rect;
                Some(DagLayoutEdge {
                    from_session_id: edge.parent_session_id,
                    to_session_id: edge.child_session_id,
                    relation_kind: edge.relation_kind,
                    from: DagPoint {
                        x: parent.x + parent.width,
                        y: parent.center_y(),
                    },
                    to: DagPoint {
                        x: child.x,
                        y: child.center_y(),
                    },
                })
            })
            .collect();
        Ok(Self {
            width: 2. * INSET + depth_count as f64 * NODE_WIDTH + (depth_count - 1) as f64 * X_GAP,
            height: 2. * INSET + max_height,
            depth_count,
            layout_revision: graph.layout_revision,
            nodes,
            edges,
            by_id,
            by_depth,
            scene_id: graph.scene_id,
            first_session_id,
            focused_session_id: graph.focused_session_id,
            runtime_generation: graph.runtime_generation,
            event_sequence: graph.event_sequence,
        })
    }

    pub fn scene_id(&self) -> &str {
        &self.scene_id
    }

    pub fn event_sequence(&self) -> Option<u64> {
        self.event_sequence
    }

    pub fn node(&self, session_id: &str) -> Option<&DagLayoutNode> {
        self.by_id.get(session_id).map(|&index| &self.nodes[index])
    }

    pub fn depth_nodes(&self, depth: usize) -> &[DagLayoutNode] {
        self.by_depth
            .get(depth)
            .map_or(&[], |range| &self.nodes[range.clone()])
    }

    pub fn visible_layers(&self, session_id: &str, radius: usize) -> (Vec<usize>, Vec<usize>) {
        let focus_depth = self.node(session_id).map_or(0, |node| node.depth);
        (0..self.depth_count).partition(|depth| depth.abs_diff(focus_depth) <= radius)
    }

    pub fn search(&self, query: &str) -> Vec<&DagNode> {
        let query = crate::rename_input::trim_name(query).to_lowercase();
        if query.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<_> = self
            .nodes
            .iter()
            .filter_map(|positioned| {
                let node = &positioned.node;
                let score = search_score(node, &query);
                (score > 0).then_some((score, node))
            })
            .collect();
        hits.sort_by(|(left_score, left), (right_score, right)| {
            right_score
                .cmp(left_score)
                .then_with(|| stable_node_order(left, right))
        });
        hits.into_iter().map(|(_, node)| node).collect()
    }
}

fn validate_parents(
    graph: &DagGraph,
    by_id: &HashMap<&str, usize>,
    parents: &[Option<usize>],
    topology: Topology,
) -> Result<(), DagError> {
    let mut seen = vec![false; parents.len()];
    for (index, edge) in graph.edges.iter().enumerate() {
        if !matches!(edge.relation_kind.as_str(), "derived-from" | "forked-from") {
            return Err(DagError::InvalidEdge(index));
        }
        let child = *by_id
            .get(edge.child_session_id.as_str())
            .ok_or(DagError::MissingEndpoint)?;
        // A scene may contain a child whose parent is in another scene.
        // The projection still carries its parent ID and structural edge.
        if !by_id.contains_key(edge.parent_session_id.as_str()) && topology == Topology::Complete {
            return Err(DagError::MissingEndpoint);
        }
        if graph.nodes[child].parent_session_id.as_deref() != Some(&edge.parent_session_id)
            || std::mem::replace(&mut seen[child], true)
        {
            return Err(DagError::InconsistentParent);
        }
    }
    for ((node, parent), seen) in graph.nodes.iter().zip(parents).zip(seen) {
        if topology == Topology::Complete && node.parent_session_id.is_some() && parent.is_none() {
            return Err(DagError::MissingEndpoint);
        }
        if node.parent_session_id.is_some() != seen {
            return Err(DagError::InconsistentParent);
        }
    }
    Ok(())
}

fn walk_depths(parents: &[Option<usize>], validate: bool) -> Result<Vec<usize>, DagError> {
    let mut depths = vec![None; parents.len()];
    let mut visiting = vec![false; parents.len()];
    let mut path = Vec::new();
    for start in 0..parents.len() {
        let mut cursor = Some(start);
        let mut next_depth = 0;
        while let Some(index) = cursor {
            if let Some(depth) = depths[index] {
                next_depth = depth + 1;
                break;
            }
            if visiting[index] {
                if validate {
                    return Err(DagError::Cycle);
                }
                // The frozen fallback assigns its first cyclic node depth 1.
                next_depth = 1;
                break;
            }
            visiting[index] = true;
            path.push(index);
            cursor = parents[index];
        }
        for index in path.drain(..).rev() {
            depths[index] = Some(next_depth);
            visiting[index] = false;
            next_depth += 1;
        }
    }
    Ok(depths.into_iter().map(|depth| depth.unwrap_or(0)).collect())
}

fn node_height(title: &str) -> f64 {
    let units = title.chars().fold(0., |units, ch| {
        units
            + match ch {
                '\u{2e80}'..='\u{9fff}'
                | '\u{f900}'..='\u{faff}'
                | '\u{ff01}'..='\u{ff60}'
                | '\u{ffe0}'..='\u{ffe6}' => 1.,
                _ => 0.55,
            }
    });
    BASE_HEIGHT + ((units / 18_f64).ceil().max(1.) - 1.) * 18.
}

fn stable_node_order(left: &DagNode, right: &DagNode) -> Ordering {
    const JS_MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;
    left.sibling_created_seq
        .unwrap_or(JS_MAX_SAFE_INTEGER)
        .cmp(&right.sibling_created_seq.unwrap_or(JS_MAX_SAFE_INTEGER))
        .then_with(|| session_id_order(&left.session_id, &right.session_id))
}

fn session_id_order(left: &str, right: &str) -> Ordering {
    // Runtime IDs are ASCII. Fold case for the primary collation and sort
    // lowercase first when the strings differ only in case.
    left.bytes()
        .map(|ch| ch.to_ascii_lowercase())
        .cmp(right.bytes().map(|ch| ch.to_ascii_lowercase()))
        .then_with(|| right.cmp(left))
}

fn search_score(node: &DagNode, query: &str) -> u8 {
    let title = node.title.to_lowercase();
    if title == query {
        100
    } else if title.starts_with(query) {
        80
    } else if title.contains(query) {
        60
    } else if node
        .branch
        .as_ref()
        .is_some_and(|branch| branch.to_lowercase().contains(query))
    {
        45
    } else if node.cwd.to_lowercase().contains(query) {
        30
    } else if node.latest_lines.join("\n").to_lowercase().contains(query) {
        20
    } else {
        0
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DagWorldBounds {
    pub left: f64,
    pub right: f64,
    pub top: f64,
    pub bottom: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DagCounts {
    pub running: usize,
    pub needs_input: usize,
    pub error: usize,
}

impl DagCounts {
    fn record(&mut self, node: &DagNode) {
        if node.archived {
            return;
        }
        match node.work_status.as_str() {
            "running" | "starting" => self.running += 1,
            "needs-input" => self.needs_input += 1,
            "error" => self.error += 1,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DagAggregateKind {
    Branch,
    LayerOverflow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DagDirection {
    After,
    Before,
}

impl DagDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::After => "after",
            Self::Before => "before",
        }
    }
}

#[derive(Debug)]
pub struct DagAggregate<'a> {
    pub key: String,
    pub kind: DagAggregateKind,
    pub direction: DagDirection,
    pub branch_root_id: String,
    pub target_session_id: &'a str,
    pub rect: DagRect,
    pub session_ids: Vec<&'a str>,
    pub counts: DagCounts,
    pub minimum_depth: usize,
    pub maximum_depth: usize,
}

#[derive(Debug, Default)]
pub struct DagRenderModel<'a> {
    pub real_nodes: Vec<&'a DagLayoutNode>,
    pub aggregates: Vec<DagAggregate<'a>>,
    pub edges: Vec<&'a DagLayoutEdge>,
}

#[derive(Clone, Copy, Debug)]
pub struct DagRenderInput<'a> {
    pub full_depths: &'a [usize],
    pub world_bounds: DagWorldBounds,
    pub center_world_y: f64,
    pub preview_session_id: &'a str,
    pub max_items: usize,
    pub max_edges: usize,
}

pub fn build_render_model<'a>(
    layout: &'a DagLayout,
    input: DagRenderInput<'_>,
) -> DagRenderModel<'a> {
    let mut depths = input.full_depths.to_vec();
    depths.sort_unstable();
    depths.dedup();
    let Some((&first, &last)) = depths.first().zip(depths.last()) else {
        return DagRenderModel::default();
    };
    if input.max_items == 0 {
        return DagRenderModel::default();
    }

    let mut real_nodes: Vec<_> = depths
        .into_iter()
        .flat_map(|depth| {
            let nodes = layout.depth_nodes(depth);
            let first = nodes
                .partition_point(|node| node.rect.y + node.rect.height < input.world_bounds.top);
            let last = nodes.partition_point(|node| node.rect.y <= input.world_bounds.bottom);
            nodes.get(first..last).unwrap_or_default()
        })
        .filter(|node| {
            node.node.session_id != input.preview_session_id
                && node.rect.intersects(input.world_bounds)
        })
        .collect();
    real_nodes.sort_by(|left, right| {
        vertical_distance(left.rect, input.center_world_y)
            .total_cmp(&vertical_distance(right.rect, input.center_world_y))
            .then_with(|| left.depth.cmp(&right.depth))
            .then_with(|| left.rect.y.total_cmp(&right.rect.y))
            .then_with(|| session_id_order(&left.node.session_id, &right.node.session_id))
    });
    if let Some(preview) = layout
        .node(input.preview_session_id)
        .filter(|node| input.full_depths.contains(&node.depth))
    {
        real_nodes.insert(0, preview);
    }

    let groups = aggregate_far_nodes(layout, first, last, input.center_world_y);
    real_nodes.truncate(input.max_items - usize::from(!groups.is_empty()));
    let aggregates = cap_aggregates(groups, input.max_items - real_nodes.len());
    let visible: HashSet<&str> = real_nodes
        .iter()
        .map(|node| node.node.session_id.as_str())
        .chain(aggregates.iter().map(|item| item.target_session_id))
        .collect();
    let edges = layout
        .edges
        .iter()
        .filter(|edge| {
            visible.contains(edge.from_session_id.as_str())
                && visible.contains(edge.to_session_id.as_str())
        })
        .take(input.max_edges)
        .collect();
    DagRenderModel {
        real_nodes,
        aggregates,
        edges,
    }
}

fn vertical_distance(rect: DagRect, center: f64) -> f64 {
    (rect.center_y() - center).abs()
}

fn aggregate_far_nodes(
    layout: &DagLayout,
    first: usize,
    last: usize,
    center: f64,
) -> Vec<DagAggregate<'_>> {
    if first == 0 && last >= layout.depth_count - 1 {
        return Vec::new();
    }
    let mut roots = vec![None; layout.nodes.len()];
    let mut groups = HashMap::<_, Vec<_>>::new();
    for (index, node) in layout.nodes.iter().enumerate() {
        let id = node.node.session_id.as_str();
        let parent_root = node.parent_index.and_then(|parent| roots[parent]);
        let (direction, root) = if node.depth < first {
            (
                DagDirection::Before,
                parent_root
                    .or(node.node.parent_session_id.as_deref())
                    .unwrap_or(id),
            )
        } else if node.depth > last {
            (DagDirection::After, parent_root.unwrap_or(id))
        } else {
            continue;
        };
        roots[index] = Some(root);
        groups.entry((direction, root)).or_default().push(node);
    }
    let mut aggregates: Vec<_> = groups
        .into_iter()
        .map(|((direction, branch_root_id), members)| {
            let minimum_depth = members[0].depth;
            let maximum_depth = members[members.len() - 1].depth;
            let boundary_depth = match direction {
                DagDirection::After => minimum_depth,
                DagDirection::Before => maximum_depth,
            };
            let target = members
                .iter()
                .filter(|node| node.depth == boundary_depth)
                .min_by(|left, right| {
                    vertical_distance(left.rect, center)
                        .total_cmp(&vertical_distance(right.rect, center))
                })
                .expect("an aggregate has a boundary node");
            let mut counts = DagCounts::default();
            let session_ids = members
                .iter()
                .map(|node| {
                    counts.record(&node.node);
                    node.node.session_id.as_str()
                })
                .collect();
            DagAggregate {
                key: format!("aggregate:{}:{branch_root_id}", direction.as_str()),
                kind: DagAggregateKind::Branch,
                direction,
                branch_root_id: branch_root_id.to_owned(),
                target_session_id: &target.node.session_id,
                rect: target.rect,
                session_ids,
                counts,
                minimum_depth,
                maximum_depth,
            }
        })
        .collect();
    aggregates.sort_by(|left, right| {
        vertical_distance(left.rect, center)
            .total_cmp(&vertical_distance(right.rect, center))
            .then_with(|| session_id_order(&left.key, &right.key))
    });
    aggregates
}

fn cap_aggregates(mut aggregates: Vec<DagAggregate<'_>>, budget: usize) -> Vec<DagAggregate<'_>> {
    if budget == 0 {
        return Vec::new();
    }
    if aggregates.len() <= budget {
        return aggregates;
    }
    let mut overflow = aggregates.drain(budget - 1..);
    let mut merged = overflow.next().expect("overflow is non-empty");
    for item in overflow {
        merged.minimum_depth = merged.minimum_depth.min(item.minimum_depth);
        merged.maximum_depth = merged.maximum_depth.max(item.maximum_depth);
        merged.session_ids.extend(item.session_ids);
        merged.counts.running += item.counts.running;
        merged.counts.needs_input += item.counts.needs_input;
        merged.counts.error += item.counts.error;
    }
    merged.branch_root_id = format!("layer:{}-{}", merged.minimum_depth, merged.maximum_depth);
    merged.key = format!(
        "aggregate:{}:layer-overflow:{}-{}",
        merged.direction.as_str(),
        merged.minimum_depth,
        merged.maximum_depth
    );
    merged.kind = DagAggregateKind::LayerOverflow;
    aggregates.push(merged);
    aggregates
}

#[derive(Debug, Default)]
pub struct DagSearch {
    pub query: String,
    pub selected: usize,
    pub results: Vec<String>,
}

impl DagSearch {
    pub fn visible_results(&self) -> &[String] {
        &self.results[..self.results.len().min(SEARCH_DISPLAY_LIMIT)]
    }

    pub fn move_selection(&mut self, delta: isize) {
        if !self.results.is_empty() {
            // The frozen dropdown draws twelve rows, but the keyboard wraps
            // around the complete results, including an unrendered 13th match.
            self.selected = (self.selected as i128 + delta as i128)
                .rem_euclid(self.results.len() as i128) as usize;
        }
    }

    pub fn hover(&mut self, index: usize) {
        if index < self.visible_results().len() {
            self.selected = index;
        }
    }

    fn refresh(&mut self, layout: &DagLayout) {
        let selected_id = self.results.get(self.selected);
        let results: Vec<_> = layout
            .search(&self.query)
            .into_iter()
            .map(|node| node.session_id.clone())
            .collect();
        self.selected = selected_id
            .and_then(|id| results.iter().position(|result| result == id))
            .unwrap_or_else(|| self.selected.min(results.len().saturating_sub(1)));
        self.results = results;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DagSelection {
    pub session_id: String,
    pub target_window_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DagGraphUpdate {
    Queued,
    Ignored,
    RefreshRequired,
}

pub struct DagCanvas {
    pub layout: DagLayout,
    pub transform: DagTransform,
    pub preview_session_id: String,
    pub viewport: DagSize,
    pub search: DagSearch,
    initial_focus_pending: bool,
    pending_graph: Option<DagGraph>,
    awaiting_snapshot: bool,
}

impl DagCanvas {
    pub fn new(layout: DagLayout, focused: &str, transform: Option<DagTransform>) -> Self {
        let preview_session_id = preferred_session(&layout, focused);
        let transform = transform.filter(|value| {
            value.x.is_finite()
                && value.y.is_finite()
                && value.scale.is_finite()
                && value.scale > 0.
        });
        Self {
            layout,
            transform: transform.unwrap_or_default(),
            preview_session_id,
            viewport: DagSize::default(),
            search: DagSearch::default(),
            initial_focus_pending: transform.is_none(),
            pending_graph: None,
            awaiting_snapshot: false,
        }
    }

    pub fn focus_initial(&mut self) -> bool {
        if !std::mem::take(&mut self.initial_focus_pending) {
            return false;
        }
        let id = self.preview_session_id.clone();
        self.focus_node(&id)
    }

    #[cfg(any(target_os = "windows", test))]
    fn focus_initial_below_toolbar(&mut self, toolbar_bottom: Option<f64>) -> bool {
        if !self.focus_initial() {
            return false;
        }
        let Some(toolbar_bottom) = toolbar_bottom else {
            return true;
        };
        // The layout includes INSET on all sides. Clear the toolbar only
        // when every card still fits in the viewport at the initial zoom.
        let minimum_y = toolbar_bottom + 8. - INSET;
        if self.transform.scale == 1.
            && self.transform.y < minimum_y
            && self.transform.x + INSET >= 0.
            && self.transform.x + self.layout.width - INSET <= self.viewport.width
            && minimum_y + self.layout.height - INSET <= self.viewport.height
        {
            self.transform.y = minimum_y;
        }
        true
    }

    pub fn focus_node(&mut self, id: &str) -> bool {
        let Some(node) = self.layout.node(id) else {
            return false;
        };
        self.preview_session_id = id.to_owned();
        let center = self.viewport.center();
        self.transform.x = center.x - (node.rect.x + node.rect.width / 2.) * self.transform.scale;
        self.transform.y = center.y - node.rect.center_y() * self.transform.scale;
        true
    }

    pub fn select_node(&mut self, id: &str) -> Option<DagSelection> {
        let selection = self.selection_for(id)?;
        self.preview_session_id = id.to_owned();
        Some(selection)
    }

    pub fn choose_search(&mut self, index: Option<usize>) -> Option<DagSelection> {
        let id = self
            .search
            .results
            .get(index.unwrap_or(self.search.selected))?
            .clone();
        let selection = self.selection_for(&id)?;
        self.focus_node(&id);
        Some(selection)
    }

    fn selection_for(&self, id: &str) -> Option<DagSelection> {
        let node = &self.layout.node(id)?.node;
        Some(DagSelection {
            session_id: id.to_owned(),
            target_window_id: node.detached_window_id.clone(),
        })
    }

    pub fn set_query(&mut self, query: &str) {
        if self.search.query == query {
            return;
        }
        self.search.query = query.to_owned();
        self.search.results.clear();
        self.search.selected = 0;
        self.search.refresh(&self.layout);
    }

    pub fn replace_layout(&mut self, layout: DagLayout) {
        if layout.scene_id != self.layout.scene_id {
            return;
        }
        self.preview_session_id = preferred_session(&layout, &self.preview_session_id);
        self.search.refresh(&layout);
        self.layout = layout;
    }

    /// Accept one authoritative snapshot without remounting the canvas.
    pub fn apply_snapshot(&mut self, mut graph: DagGraph) -> Result<bool, DagError> {
        if graph.scene_id != self.layout.scene_id {
            return Err(DagError::WrongScene);
        }
        if !self.awaiting_snapshot && self.is_stale(&graph, false) {
            return Ok(false);
        }
        let generation_changed = graph.runtime_generation.is_some()
            && graph.runtime_generation != self.layout.runtime_generation;
        if graph.runtime_generation.is_none() {
            graph
                .runtime_generation
                .clone_from(&self.layout.runtime_generation);
        }
        if graph.layout_revision.is_none() {
            graph.layout_revision = self.layout.layout_revision;
        }
        if !generation_changed && !self.awaiting_snapshot && graph.event_sequence.is_none() {
            graph.event_sequence = self.layout.event_sequence;
        }
        let layout = DagLayout::new(graph)?;
        if generation_changed || self.awaiting_snapshot {
            self.pending_graph = None;
        }
        self.replace_layout(layout);
        self.awaiting_snapshot = false;
        Ok(true)
    }

    pub fn begin_reconnect(&mut self) {
        self.pending_graph = None;
        self.awaiting_snapshot = true;
    }

    /// Keep the newest full graph event for the next frame. A generation
    /// change asks the host for a scoped snapshot before accepting new events.
    pub fn queue_graph(&mut self, graph: DagGraph) -> DagGraphUpdate {
        if graph.scene_id != self.layout.scene_id {
            return DagGraphUpdate::Ignored;
        }
        if self.awaiting_snapshot {
            return DagGraphUpdate::Ignored;
        }
        if graph.runtime_generation.is_some()
            && self.layout.runtime_generation.is_some()
            && graph.runtime_generation != self.layout.runtime_generation
        {
            self.begin_reconnect();
            return DagGraphUpdate::RefreshRequired;
        }
        if self.is_stale(&graph, true)
            || self.pending_graph.as_ref().is_some_and(|pending| {
                pending.runtime_generation == graph.runtime_generation
                    && pending.event_sequence > graph.event_sequence
            })
        {
            return DagGraphUpdate::Ignored;
        }
        self.pending_graph = Some(graph);
        DagGraphUpdate::Queued
    }

    pub fn apply_graph_frame(&mut self) -> Result<bool, DagError> {
        match self.pending_graph.take() {
            Some(graph) => self.apply_snapshot(graph),
            None => Ok(false),
        }
    }

    fn is_stale(&self, graph: &DagGraph, include_equal: bool) -> bool {
        let same_generation = graph.runtime_generation.is_none()
            || graph.runtime_generation == self.layout.runtime_generation;
        same_generation
            && graph
                .event_sequence
                .zip(self.layout.event_sequence)
                .is_some_and(|(incoming, current)| {
                    incoming < current || include_equal && incoming == current
                })
    }

    pub fn zoom_at(&mut self, scale: f64, anchor: DagPoint) {
        self.transform = self.transform.zoom_at(scale, anchor);
    }

    pub fn pan_by(&mut self, delta: DagPoint) {
        self.transform.x += delta.x;
        self.transform.y += delta.y;
    }

    pub fn reset_transform(&mut self) {
        self.transform = DagTransform::default();
    }

    pub fn world_bounds(&self) -> DagWorldBounds {
        let top_left = self.transform.world(DagPoint::default());
        let bottom_right = self.transform.world(DagPoint {
            x: self.viewport.width,
            y: self.viewport.height,
        });
        DagWorldBounds {
            left: top_left.x - 360.,
            right: bottom_right.x + 360.,
            top: top_left.y - 260.,
            bottom: bottom_right.y + 260.,
        }
    }

    pub fn render_model(&self) -> DagRenderModel<'_> {
        let center = self.transform.world(self.viewport.center());
        let center_depth = ((center.x - INSET) / COLUMN_STEP)
            .round()
            .clamp(0., (self.layout.depth_count - 1) as f64) as usize;
        let preview_depth = self
            .layout
            .node(&self.preview_session_id)
            .map_or(0, |node| node.depth);
        let focus_depth = if center_depth.abs_diff(preview_depth) > 1 {
            center_depth
        } else {
            preview_depth
        };
        let full_depths: Vec<_> = (focus_depth.saturating_sub(1)
            ..=(focus_depth + 1).min(self.layout.depth_count - 1))
            .collect();
        build_render_model(
            &self.layout,
            DagRenderInput {
                full_depths: &full_depths,
                world_bounds: self.world_bounds(),
                center_world_y: center.y,
                preview_session_id: &self.preview_session_id,
                max_items: 400,
                max_edges: 800,
            },
        )
    }
}

fn preferred_session(layout: &DagLayout, preferred: &str) -> String {
    [
        Some(preferred),
        layout.focused_session_id.as_deref(),
        layout.first_session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|id| layout.node(id).is_some())
    .unwrap_or_default()
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{ScrollDelta, point, px};
    use serde_json::json;

    #[test]
    fn dag_wheel_pan_follows_the_gesture_for_pixels_and_lines() {
        for (dx, dy) in [(-20., 40.), (20., -40.)] {
            let dom_x = -f64::from(dx);
            let dom_y = -f64::from(dy);
            let expected = DagPoint {
                x: -dom_x,
                y: -dom_y,
            };
            assert_eq!(
                dag_wheel_pan(ScrollDelta::Pixels(point(px(dx), px(dy)))),
                expected
            );
            assert_eq!(
                dag_wheel_pan(ScrollDelta::Lines(point(dx / 20., dy / 20.))),
                expected
            );
        }
    }

    #[test]
    fn dag_wheel_zoom_increases_for_positive_gpui_delta() {
        for (delta, zoom_in) in [
            (ScrollDelta::Pixels(point(px(0.), px(20.))), true),
            (ScrollDelta::Pixels(point(px(0.), px(-20.))), false),
            (ScrollDelta::Lines(point(0., 1.)), true),
            (ScrollDelta::Lines(point(0., -1.)), false),
        ] {
            let dy = dag_wheel_pan(delta).y;
            let dom_delta_y = -dy;
            let factor = dag_wheel_zoom_factor(dy);
            assert_eq!(factor > 1., zoom_in);
            assert_eq!(factor, (-dom_delta_y * 0.002).exp());
        }
    }

    fn initial_focus_fixture(roots: usize, saved: Option<DagTransform>) -> DagCanvas {
        let nodes: Vec<_> = (0..roots)
            .map(|index| json!({"sessionId": index.to_string(), "title": "Shell", "cwd": "/fixture"}))
            .collect();
        let graph = DagGraph::from_snapshot(&json!({
            "sceneId": "scene",
            "nodes": nodes,
            "edges": [],
        }))
        .unwrap();
        let mut canvas = DagCanvas::new(
            DagLayout::new(graph).unwrap(),
            &(roots - 1).to_string(),
            saved,
        );
        canvas.viewport = DagSize {
            width: 1065.,
            height: 589.5,
        };
        canvas
    }

    #[test]
    fn dag_first_focus_clears_windows_toolbar_with_two_roots() {
        let mut canvas = initial_focus_fixture(2, None);
        let focused = canvas.preview_session_id.clone();
        let first = canvas.layout.node("0").unwrap().rect;
        let second = canvas.layout.node(&focused).unwrap().rect;
        assert!(canvas.focus_node(&focused));
        let centered = canvas.transform;
        assert!(first.y + centered.y < 68.);

        assert!(canvas.focus_initial_below_toolbar(Some(68.)));
        assert_eq!(canvas.preview_session_id, focused);
        assert_eq!(canvas.transform.scale, 1.);
        assert_eq!(canvas.transform.x, centered.x);
        assert_eq!(first.y + canvas.transform.y, 76.);
        assert!(second.y + second.height + canvas.transform.y <= canvas.viewport.height);

        assert!(canvas.focus_node(&focused));
        assert_eq!(canvas.transform, centered);
        assert!(!canvas.focus_initial_below_toolbar(Some(68.)));
        assert_eq!(canvas.transform, centered);
        canvas.reset_transform();
        assert_eq!(canvas.transform, DagTransform::default());
    }

    #[test]
    fn dag_first_focus_keeps_single_and_oversize_graphs_centered() {
        for roots in [1, 3] {
            let mut canvas = initial_focus_fixture(roots, None);
            let focused = canvas.preview_session_id.clone();
            assert!(canvas.focus_node(&focused));
            let centered = canvas.transform;
            assert!(canvas.focus_initial_below_toolbar(Some(68.)));
            assert_eq!(canvas.transform, centered);
        }
    }

    #[test]
    fn dag_first_focus_preserves_saved_transform() {
        let saved = DagTransform {
            x: 112.,
            y: -37.,
            scale: 1.25,
        };
        let mut canvas = initial_focus_fixture(2, Some(saved));
        assert!(!canvas.focus_initial_below_toolbar(Some(68.)));
        assert_eq!(canvas.transform, saved);
    }

    #[test]
    fn dag_canvas_invalid_snapshot_keeps_last_usable_state() {
        let valid = json!({
            "sceneId": "scene",
            "runtimeGeneration": "first",
            "eventSequence": 2,
            "nodes": [{"sessionId": "root", "title": "Draft", "cwd": "/fixture"}],
            "edges": [],
        });
        let graph = DagGraph::from_snapshot(&valid).unwrap();
        let mut canvas = DagCanvas::new(DagLayout::new(graph.clone()).unwrap(), "root", None);
        canvas.set_query("Draft");
        canvas.focus_initial();
        let transform = canvas.transform;

        let mut bad_node = valid.clone();
        bad_node["nodes"][0]["sessionId"] = json!("");
        assert_eq!(
            DagGraph::from_snapshot(&bad_node).unwrap_err(),
            DagError::InvalidNode(0)
        );

        let mut duplicate = graph.clone();
        duplicate.event_sequence = Some(3);
        duplicate.nodes.push(duplicate.nodes[0].clone());
        assert_eq!(canvas.queue_graph(duplicate), DagGraphUpdate::Queued);
        assert_eq!(
            canvas.apply_graph_frame(),
            Err(DagError::DuplicateNode("root".into()))
        );
        assert_eq!(canvas.queue_graph(graph), DagGraphUpdate::Ignored);
        assert_eq!(canvas.preview_session_id, "root");
        assert_eq!(canvas.search.results, ["root"]);
        assert_eq!(canvas.transform, transform);
    }
}

mod view {
    use super::*;

    use std::{collections::HashSet, time::Instant};

    use gpui::{prelude::*, *};
    use matou_native_core::sessions::InterfaceLocale;

    use crate::carousel_motion::Curve;
    use crate::rename_input::{RenameInput, RenameInputEvent};
    use crate::ui_text::ui_text;
    use crate::workspace::card_tooltip_box;

    // Frozen visual values:
    // - archived renderer: dag/dag.css:2-94 (dark palette and light overrides)
    // - archived renderer: dag/DagCanvas.tsx:173-285 (toolbar/cards)
    // - archived renderer: dag/DagSearch.tsx:22-41 (search dropdown)
    // - archived renderer: i18n/messages/dag.ts (bilingual copy)
    const FORK_EDGE: u32 = 0x5c91cf;
    const DERIVED_EDGE: u32 = 0x4d6076;
    const ARROW_EDGE: u32 = 0x57708b;
    const ROOT_GUIDE: u32 = 0x5b7ea452;
    const STATUS_ACTIVE: u32 = 0x4cbd78;
    const STATUS_INPUT: u32 = 0xe9ae43;
    const STATUS_ERROR: u32 = 0xe66b64;
    const STATUS_IDLE: u32 = 0x8d95a0;

    const SEARCH_TOP: f32 = 44.;
    const SEARCH_MAX_HEIGHT: f32 = 350.;

    /// The frozen DAG has a complete dark canvas and a light override for the
    /// workspace chrome. The host selects the same choice it gives DagWindowApp.
    #[derive(Clone, Copy)]
    struct Palette {
        canvas: u32,
        grid: u32,
        toolbar_border: u32,
        toolbar_text: u32,
        toolbar_background: u32,
        toolbar_hover: u32,
        search_border: u32,
        search_focus: u32,
        search_background: u32,
        search_result_background: u32,
        search_result_text: u32,
        search_result_meta: u32,
        search_result_selected: u32,
        card_border: u32,
        card_text: u32,
        card_background: u32,
        card_shadow: u32,
        card_shadow_alpha: f32,
        title: u32,
        path: u32,
        cwd: u32,
        pre_text: u32,
        pre_background: u32,
        meta: u32,
        top: u32,
        focus: u32,
        focus_shadow_alpha: f32,
        shared_text: u32,
        shared_background: u32,
        stopped_border: u32,
        stopped_text: u32,
        stopped_background: u32,
        stopped_title: u32,
        stopped_pre_text: u32,
        stopped_pre_background: u32,
        aggregate_border: u32,
        aggregate_background: u32,
        aggregate_hover: u32,
        aggregate_title: u32,
        aggregate_eyebrow: u32,
        aggregate_range: u32,
        aggregate_count: u32,
        legend_derived: u32,
    }

    impl Palette {
        fn new(dark: bool) -> Self {
            if dark {
                Self {
                    canvas: 0x17191d,
                    grid: 0xffffff13,
                    toolbar_border: 0x363c45,
                    toolbar_text: 0x929aa6,
                    toolbar_background: 0x1e2126eb,
                    toolbar_hover: 0xffffff14,
                    search_border: 0x3a4049,
                    search_focus: 0x568fd0,
                    search_background: 0x1f2227f2,
                    search_result_background: 0x24272c,
                    search_result_text: 0xdfe3e9,
                    search_result_meta: 0x818994,
                    search_result_selected: 0x5b9de929,
                    card_border: 0x3a4049,
                    card_text: 0xccd1d9,
                    card_background: 0x22252a,
                    card_shadow: 0x000000,
                    card_shadow_alpha: 0.24,
                    title: 0xf0f2f5,
                    path: 0x858e9b,
                    cwd: 0x6f7885,
                    pre_text: 0xaeb5bf,
                    pre_background: 0x00000026,
                    meta: 0x717985,
                    top: 0x8d95a0,
                    focus: 0x5b9de9,
                    focus_shadow_alpha: 0.3,
                    shared_text: 0xe2b35d,
                    shared_background: 0xe2b35d1c,
                    stopped_border: 0x343941,
                    stopped_text: 0x858d98,
                    stopped_background: 0x1c1f23,
                    stopped_title: 0xaab0b8,
                    stopped_pre_text: 0x7b838e,
                    stopped_pre_background: 0x0000001a,
                    aggregate_border: 0x536274,
                    aggregate_background: 0x2a3038eb,
                    aggregate_hover: 0x303943f5,
                    aggregate_title: 0xe3e7ec,
                    aggregate_eyebrow: 0x8190a2,
                    aggregate_range: 0x8994a2,
                    aggregate_count: 0xaab2bd,
                    legend_derived: 0x708196,
                }
            } else {
                Self {
                    canvas: 0xf7f8fa,
                    grid: 0x2c38481c,
                    toolbar_border: 0xd5dae2,
                    toolbar_text: 0x596270,
                    toolbar_background: 0xfffffff5,
                    toolbar_hover: 0x0000000d,
                    search_border: 0xd5dae2,
                    search_focus: 0x568fd0,
                    search_background: 0xfffffff5,
                    search_result_background: 0xffffff,
                    search_result_text: 0x303744,
                    search_result_meta: 0x818994,
                    search_result_selected: 0x5b9de929,
                    card_border: 0xd5dae2,
                    card_text: 0x505966,
                    card_background: 0xffffff,
                    card_shadow: 0x2e3949,
                    card_shadow_alpha: 0.1,
                    title: 0x2d3541,
                    path: 0x858e9b,
                    cwd: 0x6f7885,
                    pre_text: 0x586271,
                    pre_background: 0xf6f7f9,
                    meta: 0x717985,
                    top: 0x8d95a0,
                    focus: 0x5b9de9,
                    focus_shadow_alpha: 0.16,
                    shared_text: 0x91620e,
                    shared_background: 0xfff3d5,
                    stopped_border: 0xd7dbe1,
                    stopped_text: 0x7e8793,
                    stopped_background: 0xeef0f3,
                    stopped_title: 0x6b7480,
                    stopped_pre_text: 0x7c8590,
                    stopped_pre_background: 0xe7e9ed,
                    aggregate_border: 0xaeb9c7,
                    aggregate_background: 0xeff3f7f5,
                    aggregate_hover: 0xeef3f8,
                    aggregate_title: 0x303946,
                    aggregate_eyebrow: 0x8190a2,
                    aggregate_range: 0x8994a2,
                    aggregate_count: 0xaab2bd,
                    legend_derived: 0x708196,
                }
            }
        }
    }

    /// Events that belong to the host. The view itself never persists geometry,
    /// selects an owner window, or resolves another window's lifetime.
    #[derive(Clone, Debug)]
    pub enum DagCanvasEvent {
        /// The latest viewport after a paint-aligned gesture commit. The host
        /// owns persistence; a native close must flush this value first.
        TransformChanged(DagTransform),
        /// A session must receive selection. Search Enter/click emits this one
        /// frame after centering, so the host must wait for its geometry write
        /// acknowledgement before closing this window and focusing the session.
        Select(DagSelection),
        /// Escape (including Escape inside the search field). The host decides
        /// whether to close the DAG window.
        CloseRequested,
    }

    struct Centering {
        from: DagTransform,
        started: Instant,
    }

    impl Centering {
        fn sample(&self, target: DagTransform) -> Option<DagTransform> {
            let elapsed = self.started.elapsed().as_secs_f64() / 0.24;
            if elapsed >= 1. {
                return None;
            }
            // Curve::Width is cubic-bezier(.2,.8,.2,1), as in dag.css:12.
            let progress = Curve::Width.ease(elapsed);
            Some(DagTransform {
                x: self.from.x + (target.x - self.from.x) * progress,
                y: self.from.y + (target.y - self.from.y) * progress,
                scale: target.scale,
            })
        }
    }

    fn reduced_motion() -> bool {
        #[cfg(target_os = "macos")]
        {
            crate::macos::reduced_motion()
        }
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW,
            };
            let mut enabled: i32 = 1;
            // Read the user's existing accessibility preference.
            unsafe {
                SystemParametersInfoW(
                    SPI_GETCLIENTAREAANIMATION,
                    0,
                    (&mut enabled as *mut i32).cast(),
                    0,
                );
            }
            enabled == 0
        }
    }

    struct DragState {
        last: Point<Pixels>,
    }

    /// An embeddable GPUI view over the frozen DAG canvas.
    ///
    /// Interaction contract:
    /// - The first usable viewport schedules `focus_initial` on the next frame.
    /// - Search Enter/click centers immediately and emits `Select` on the next
    ///   frame; a normal card emits `Select` immediately; an aggregate only
    ///   recenters. The host waits for the geometry write acknowledgement before
    ///   closing and focusing another session window.
    /// - RenameInput owns IME composition and emits Changed/Confirm/Cancel,
    ///   while this view owns the Up/Down result keys. Cmd/Ctrl +, -, 0 and F
    ///   stay active while the field has focus, matching the frozen window
    ///   handler.
    /// - `new` starts dark like the DOM fixture; workspace light mode uses
    ///   `new_light` or `set_dark(false)`.
    pub struct DagCanvasView {
        canvas: DagCanvas,
        locale: InterfaceLocale,
        focus: FocusHandle,
        search_input: Entity<RenameInput>,
        search_subscription: Option<Subscription>,
        toolbar_right_inset: Pixels,
        bounds: Option<Bounds<Pixels>>,
        drag: Option<DragState>,
        centering: Option<Centering>,
        notifications: HashSet<String>,
        initial_focus_done: bool,
        #[cfg(target_os = "windows")]
        initial_toolbar_bottom: Option<f64>,
        dark: bool,
        transform_frame_pending: bool,
        committed_transform: (f64, f64, f64),
    }

    impl EventEmitter<DagCanvasEvent> for DagCanvasView {}

    impl Focusable for DagCanvasView {
        fn focus_handle(&self, _: &App) -> FocusHandle {
            self.focus.clone()
        }
    }

    impl DagCanvasView {
        /// Default to the frozen DOM fixture's dark canvas. Workspace chrome
        /// light mode must call `new_light` or `set_dark(false)`.
        pub fn new(state: DagCanvas, locale: InterfaceLocale, cx: &mut Context<Self>) -> Self {
            Self::new_with_dark(state, locale, true, cx)
        }

        pub fn new_light(
            state: DagCanvas,
            locale: InterfaceLocale,
            cx: &mut Context<Self>,
        ) -> Self {
            Self::new_with_dark(state, locale, false, cx)
        }

        pub fn new_with_dark(
            state: DagCanvas,
            locale: InterfaceLocale,
            dark: bool,
            cx: &mut Context<Self>,
        ) -> Self {
            // A restored DagCanvas already carries its search query; the native
            // editor must mount with that text instead of an empty field.
            let initial_query = state.search.query.clone();
            let committed_transform = transform_tuple(state.transform);
            let search_input =
                cx.new(|cx| make_search_input(dark, initial_query, search_placeholder(locale), cx));
            Self {
                canvas: state,
                locale,
                focus: cx.focus_handle().tab_stop(true),
                search_input,
                search_subscription: None,
                toolbar_right_inset: px(16.),
                bounds: None,
                drag: None,
                centering: None,
                notifications: HashSet::new(),
                initial_focus_done: false,
                #[cfg(target_os = "windows")]
                initial_toolbar_bottom: None,
                dark,
                transform_frame_pending: false,
                committed_transform,
            }
        }

        pub fn canvas(&self) -> &DagCanvas {
            &self.canvas
        }

        pub fn canvas_mut(&mut self) -> &mut DagCanvas {
            &mut self.canvas
        }

        /// Reserve space for a host control alongside the floating toolbar.
        pub fn with_toolbar_right_inset(mut self, inset: Pixels) -> Self {
            self.toolbar_right_inset = inset;
            self
        }

        /// Swap the projected layout without remounting the view. The transform
        /// and search query survive, matching the frozen live-refresh behavior.
        pub fn replace_layout(&mut self, layout: DagLayout, cx: &mut Context<Self>) {
            self.canvas.replace_layout(layout);
            cx.notify();
        }

        /// Update the blue notification ring without rebuilding the canvas.
        pub fn set_notifications(&mut self, session_ids: &[String], cx: &mut Context<Self>) {
            let next = session_ids.iter().cloned().collect();
            if self.notifications != next {
                self.notifications = next;
                cx.notify();
            }
        }

        #[cfg(debug_assertions)]
        pub fn has_notification_for_probe(&self, session_id: &str) -> bool {
            self.notifications.contains(session_id)
        }

        pub fn set_locale(&mut self, locale: InterfaceLocale, cx: &mut Context<Self>) {
            self.locale = locale;
            #[cfg(not(target_os = "windows"))]
            self.set_search_placeholder(search_placeholder(locale), cx);
            cx.notify();
        }

        fn set_search_placeholder(&self, placeholder: &str, cx: &mut Context<Self>) {
            if self.search_input.read(cx).placeholder != placeholder {
                self.search_input.update(cx, |input, cx| {
                    input.placeholder = placeholder.to_owned();
                    cx.notify();
                });
            }
        }

        /// Switch between dag.css's dark canvas and its light chrome overrides.
        /// RenameInput's private search palette differs per theme, so the editor
        /// entity is rebuilt with the current query preserved.
        pub fn set_dark(&mut self, dark: bool, cx: &mut Context<Self>) {
            if self.dark == dark {
                return;
            }
            self.dark = dark;
            let text = self.search_input.read(cx).text.clone();
            let placeholder = search_placeholder(self.locale);
            self.search_input = cx.new(|cx| make_search_input(dark, text, placeholder, cx));
            self.search_subscription = None;
            cx.notify();
        }

        /// Focus the canvas itself. Returns the handle so hosts can route Tab or
        /// menu-driven focus without reaching into private state.
        pub fn background(&self) -> Rgba {
            rgb(Palette::new(self.dark).canvas)
        }

        pub fn canvas_focus(&self) -> FocusHandle {
            self.focus.clone()
        }

        pub fn focus_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            window.focus(&self.search_input.read(cx).focus);
        }

        pub fn search_focused(&self, window: &Window, cx: &App) -> bool {
            self.search_input.read(cx).focus.is_focused(window)
        }

        #[cfg(all(target_os = "macos", debug_assertions))]
        pub fn dark_for_probe(&self) -> bool {
            self.dark
        }

        fn ensure_search_subscription(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            if self.search_subscription.is_some() {
                return;
            }
            self.search_subscription = Some(cx.subscribe_in(
                &self.search_input,
                window,
                |this, _, event, window, cx| match event {
                    RenameInputEvent::Changed => {
                        // IME composition updates arrive as Changed as well.
                        let text = this.search_input.read(cx).text.clone();
                        this.canvas.set_query(&text);
                        cx.notify();
                    }
                    RenameInputEvent::Confirm => {
                        let selected = this.canvas.search.selected;
                        this.choose_search(Some(selected), window, cx);
                    }
                    RenameInputEvent::Cancel => {
                        // The frozen window closes on Escape, including from the
                        // search field. The host decides how to tear down.
                        cx.emit(DagCanvasEvent::CloseRequested);
                        cx.stop_propagation();
                    }
                },
            ));
        }

        fn measure(
            &mut self,
            bounds: Bounds<Pixels>,
            size: DagSize,
            cx: &mut Context<Self>,
        ) -> bool {
            self.bounds = Some(bounds);
            let changed = (self.canvas.viewport.width - size.width).abs() > 0.1
                || (self.canvas.viewport.height - size.height).abs() > 0.1;
            if changed {
                self.canvas.viewport = size;
                cx.notify();
            }
            !self.initial_focus_done && size.width > 0. && size.height > 0.
        }

        fn finish_initial_focus(&mut self, cx: &mut Context<Self>) {
            self.initial_focus_done = true;
            #[cfg(target_os = "windows")]
            let focused = self.canvas.focus_initial_below_toolbar(
                self.initial_toolbar_bottom
                    .zip(self.bounds)
                    .map(|(bottom, bounds)| bottom - f64::from(bounds.top())),
            );
            #[cfg(not(target_os = "windows"))]
            let focused = self.canvas.focus_initial();
            if focused {
                self.commit_transform(cx);
            }
            cx.notify();
        }

        fn commit_transform(&mut self, cx: &mut Context<Self>) {
            self.transform_frame_pending = false;
            let transform = self.canvas.transform;
            let tuple = transform_tuple(transform);
            if tuple != self.committed_transform {
                self.committed_transform = tuple;
                cx.emit(DagCanvasEvent::TransformChanged(transform));
                cx.notify();
            }
        }

        /// Coalesce pointer/wheel visual updates and persistence into one frame,
        /// mirroring DagCanvas.tsx:59-85. The final mouse-up flushes even when
        /// the next frame has not arrived yet.
        fn schedule_transform(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            if self.transform_frame_pending {
                return;
            }
            self.transform_frame_pending = true;
            cx.on_next_frame(window, |this, _, cx| {
                if !std::mem::take(&mut this.transform_frame_pending) {
                    return;
                }
                this.commit_transform(cx);
            });
            cx.notify();
        }

        fn flush_transform(&mut self, cx: &mut Context<Self>) {
            self.commit_transform(cx);
        }

        fn display_transform(&self) -> DagTransform {
            let target = self.canvas.transform;
            self.centering
                .as_ref()
                .and_then(|motion| motion.sample(target))
                .unwrap_or(target)
        }

        fn start_centering(&mut self, from: DagTransform) {
            self.centering =
                (from != self.canvas.transform && !reduced_motion()).then(|| Centering {
                    from,
                    started: Instant::now(),
                });
        }

        fn choose_search(
            &mut self,
            index: Option<usize>,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let from = self.display_transform();
            let Some(selection) = self.canvas.choose_search(index) else {
                return;
            };
            self.start_centering(from);
            // choose_search already centers on the target. Publish the final
            // viewport before the next frame's Select so the host's geometry
            // write cannot race the window switch.
            self.commit_transform(cx);
            cx.on_next_frame(window, move |this, window, cx| {
                this.focus.focus(window);
                cx.emit(DagCanvasEvent::Select(selection));
                cx.notify();
            });
            cx.notify();
        }

        fn select_card(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
            self.focus.focus(window);
            if let Some(selection) = self.canvas.select_node(id) {
                cx.emit(DagCanvasEvent::Select(selection));
                cx.notify();
            }
        }

        pub fn center_node(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
            self.focus.focus(window);
            let from = self.display_transform();
            if self.canvas.focus_node(id) {
                self.start_centering(from);
                self.commit_transform(cx);
                cx.notify();
            }
        }

        fn zoom_by(&mut self, delta: f64, cx: &mut Context<Self>) {
            self.centering = None;
            let scale = self.canvas.transform.scale + delta;
            let anchor = self.canvas.viewport.center();
            self.canvas.zoom_at(scale, anchor);
            self.commit_transform(cx);
        }

        fn reset_viewport(&mut self, cx: &mut Context<Self>) {
            self.centering = None;
            self.canvas.reset_transform();
            self.commit_transform(cx);
        }

        fn focus_current_node(&mut self, window: &mut Window, cx: &mut Context<Self>) {
            let id = self.canvas.preview_session_id.clone();
            self.center_node(&id, window, cx);
        }

        fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
            let key = event.keystroke.key.as_str();
            let modifiers = event.keystroke.modifiers;
            // DagCanvas.tsx binds these on the window, so they must still work
            // while the RenameInput search field has focus. RenameInput leaves
            // them unhandled and they bubble here.
            if modifiers.platform || modifiers.control {
                match key {
                    "f" | "F" => {
                        self.focus_search(window, cx);
                        cx.stop_propagation();
                        return;
                    }
                    "=" | "+" => {
                        self.zoom_by(0.1, cx);
                        cx.stop_propagation();
                        return;
                    }
                    "-" => {
                        self.zoom_by(-0.1, cx);
                        cx.stop_propagation();
                        return;
                    }
                    "0" => {
                        self.reset_viewport(cx);
                        cx.stop_propagation();
                        return;
                    }
                    _ => {}
                }
            }
            let search_focused = self.search_input.read(cx).focus.is_focused(window);
            if search_focused {
                // RenameInput deliberately leaves Up/Down unhandled so the
                // parent can drive the frozen result navigation. Arrow keys
                // wrap over every match, including an unrendered 13th item.
                if self.canvas.search.results.is_empty() {
                    return;
                }
                match key {
                    "down" => {
                        self.canvas.search.move_selection(1);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "up" => {
                        self.canvas.search.move_selection(-1);
                        cx.stop_propagation();
                        cx.notify();
                    }
                    _ => {}
                }
                return;
            }
            if key == "escape" {
                cx.emit(DagCanvasEvent::CloseRequested);
                cx.stop_propagation();
            }
        }

        fn mouse_down(
            &mut self,
            event: &MouseDownEvent,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            if event.button != MouseButton::Left {
                return;
            }
            self.focus.focus(window);
            self.drag = Some(DragState {
                last: event.position,
            });
            cx.notify();
        }

        fn mouse_move(
            &mut self,
            event: &MouseMoveEvent,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            let Some(drag) = self.drag.as_mut() else {
                return;
            };
            let dx = f32::from(event.position.x - drag.last.x);
            let dy = f32::from(event.position.y - drag.last.y);
            drag.last = event.position;
            if dx == 0. && dy == 0. {
                return;
            }
            self.centering = None;
            self.canvas.pan_by(DagPoint {
                x: f64::from(dx),
                y: f64::from(dy),
            });
            self.schedule_transform(window, cx);
        }

        fn mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
            self.drag = None;
            self.flush_transform(cx);
        }

        fn mouse_up_out(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
            self.drag = None;
            self.flush_transform(cx);
        }

        fn scroll_wheel(
            &mut self,
            event: &ScrollWheelEvent,
            window: &mut Window,
            cx: &mut Context<Self>,
        ) {
            self.centering = None;
            let movement = dag_wheel_pan(event.delta);
            if event.modifiers.platform || event.modifiers.control {
                let factor = dag_wheel_zoom_factor(movement.y);
                let scale = self.canvas.transform.scale * factor;
                let anchor = self
                    .bounds
                    .as_ref()
                    .map(|bounds| DagPoint {
                        x: f64::from(event.position.x - bounds.left()),
                        y: f64::from(event.position.y - bounds.top()),
                    })
                    .unwrap_or_else(|| self.canvas.viewport.center());
                self.canvas.zoom_at(scale, anchor);
            } else {
                self.canvas.pan_by(movement);
            }
            self.schedule_transform(window, cx);
            cx.stop_propagation();
        }

        fn magnify(&mut self, event: &MagnifyEvent, window: &mut Window, cx: &mut Context<Self>) {
            self.centering = None;
            let factor = 1. + f64::from(event.magnification);
            if !factor.is_finite() || factor <= 0. {
                return;
            }
            let scale = self.canvas.transform.scale * factor;
            let anchor = self
                .bounds
                .as_ref()
                .map(|bounds| DagPoint {
                    x: f64::from(event.position.x - bounds.left()),
                    y: f64::from(event.position.y - bounds.top()),
                })
                .unwrap_or_else(|| self.canvas.viewport.center());
            self.canvas.zoom_at(scale, anchor);
            self.schedule_transform(window, cx);
            cx.stop_propagation();
        }

        fn render_toolbar(&self, window: &Window, cx: &mut Context<Self>) -> Div {
            let p = Palette::new(self.dark);
            let scale = self.canvas.transform.scale;
            #[cfg(target_os = "windows")]
            let (search_width, legend_max_width) =
                windows_toolbar_widths(f32::from(window.viewport_size().width));
            #[cfg(target_os = "windows")]
            self.set_search_placeholder(windows_search_placeholder(self.locale, search_width), cx);
            #[cfg(target_os = "windows")]
            let compact_legend = legend_max_width < 320.;
            #[cfg(target_os = "macos")]
            let (search_width, legend_max_width) = mac_toolbar_widths(
                f32::from(window.viewport_size().width),
                f32::from(self.toolbar_right_inset),
            );
            #[cfg(target_os = "macos")]
            let compact_legend = legend_max_width < 395.;
            let search_focused = self.search_input.read(cx).focus.is_focused(window);
            let query = self.canvas.search.query.clone();
            let selected = self.canvas.search.selected;
            let results = self.canvas.search.visible_results().to_vec();
            let has_results = !self.canvas.search.results.is_empty();

            let search = div()
                .relative()
                .min_w_0()
                .flex_none()
                .w(px(search_width))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .w_full()
                        .h(px(38.))
                        .px(px(if cfg!(target_os = "macos") && search_width < 280. {
                            8.
                        } else {
                            14.
                        }))
                        .rounded(px(10.))
                        .border_1()
                        .border_color(rgb(if search_focused {
                            p.search_focus
                        } else {
                            p.search_border
                        }))
                        .bg(rgba(p.search_background))
                        .shadow(if search_focused {
                            vec![
                                shadow(p.search_focus, 0.18, 0., 0., 2.),
                                shadow(p.card_shadow, p.card_shadow_alpha, 9., 28., 0.),
                            ]
                        } else {
                            vec![shadow(p.card_shadow, p.card_shadow_alpha, 9., 28., 0.)]
                        })
                        .child(self.search_input.clone()),
                )
                .when(!query.is_empty(), |search| {
                    search.child(
                        div()
                            .id("dag-search-results")
                            .absolute()
                            .top(px(SEARCH_TOP))
                            .left(px(0.))
                            .right(px(0.))
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .max_h(px(SEARCH_MAX_HEIGHT))
                            .overflow_y_scroll()
                            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                            .p(px(6.))
                            .rounded(px(10.))
                            .border_1()
                            .border_color(rgb(p.search_border))
                            .bg(rgb(p.search_result_background))
                            .shadow(vec![shadow(
                                p.card_shadow,
                                p.card_shadow_alpha,
                                16.,
                                40.,
                                0.,
                            )])
                            .children(results.iter().enumerate().map(|(index, id)| {
                                let node = self.canvas.layout.node(id);
                                let title = node
                                    .map(|node| node.node.title.clone())
                                    .unwrap_or_else(|| id.clone());
                                let meta = node.map_or_else(String::new, |node| {
                                    format!(
                                        "{} · {}",
                                        mode_label(&node.node.current_mode, self.locale),
                                        node.node.cwd
                                    )
                                });
                                let selected_style = index == selected;
                                div()
                                    .id(SharedString::from(format!("dag-search-result-{index}")))
                                    .flex()
                                    .flex_col()
                                    .gap(px(3.))
                                    .px(px(10.))
                                    .py(px(8.))
                                    .rounded(px(7.))
                                    .cursor(CursorStyle::PointingHand)
                                    .bg(if selected_style {
                                        rgba(p.search_result_selected)
                                    } else {
                                        rgba(0x00000000)
                                    })
                                    .hover(|style| style.bg(rgba(p.search_result_selected)))
                                    .on_hover(cx.listener(move |this, hovered, _, cx| {
                                        if *hovered && this.canvas.search.selected != index {
                                            this.canvas.search.hover(index);
                                            cx.notify();
                                        }
                                    }))
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.choose_search(Some(index), window, cx);
                                    }))
                                    .child(
                                        div()
                                            .text_size(px(13.))
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(rgb(p.search_result_text))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(ui_text(title)),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(rgb(p.search_result_meta))
                                            .overflow_hidden()
                                            .whitespace_nowrap()
                                            .text_ellipsis()
                                            .child(ui_text(meta)),
                                    )
                            }))
                            .when(!has_results, |results| {
                                results.child(
                                    div()
                                        .px(px(10.))
                                        .py(px(8.))
                                        .text_size(px(11.))
                                        .text_color(rgb(p.search_result_meta))
                                        .child(ui_text(search_empty(self.locale))),
                                )
                            }),
                    )
                });

            #[cfg(target_os = "windows")]
            let search = search.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

            let zoom_label = format!("{}%", (scale * 100.).round() as i32);
            let tooltip = |text| TooltipText::builder(text, cx.weak_entity());
            let tooltip_zoom_out = self.locale.text("缩小", "Zoom out");
            let tooltip_reset = self.locale.text("恢复 100%", "Reset to 100%");
            let tooltip_zoom_in = self.locale.text("放大", "Zoom in");
            let tooltip_focus = self.locale.text("聚焦当前节点", "Focus the current node");
            let zoom = div()
                .flex()
                .flex_none()
                .items_center()
                .p(px(4.))
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(p.toolbar_border))
                .bg(rgba(p.toolbar_background))
                .text_color(rgb(p.toolbar_text))
                .shadow(vec![shadow(
                    p.card_shadow,
                    p.card_shadow_alpha,
                    9.,
                    28.,
                    0.,
                )])
                .child(
                    div()
                        .id("dag-zoom-out")
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(30.))
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(rgba(p.toolbar_hover)))
                        .tooltip(tooltip(tooltip_zoom_out))
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_by(-0.1, cx)))
                        .child(ui_text("−")),
                )
                .child(
                    div()
                        .id("dag-zoom-reset")
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(30.))
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(rgba(p.toolbar_hover)))
                        .tooltip(tooltip(tooltip_reset))
                        .on_click(cx.listener(|this, _, _, cx| this.reset_viewport(cx)))
                        .child(ui_text(zoom_label)),
                )
                .child(
                    div()
                        .id("dag-zoom-in")
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(30.))
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(rgba(p.toolbar_hover)))
                        .tooltip(tooltip(tooltip_zoom_in))
                        .on_click(cx.listener(|this, _, _, cx| this.zoom_by(0.1, cx)))
                        .child(ui_text("＋")),
                )
                .child(
                    div()
                        .id("dag-zoom-focus")
                        .flex()
                        .items_center()
                        .justify_center()
                        .min_w(px(30.))
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
                        .cursor(CursorStyle::PointingHand)
                        .hover(|style| style.bg(rgba(p.toolbar_hover)))
                        .tooltip(tooltip(tooltip_focus))
                        .on_click(
                            cx.listener(|this, _, window, cx| this.focus_current_node(window, cx)),
                        )
                        .child(ui_text("⌖")),
                );

            let legend = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(10.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(9.))
                .border_1()
                .border_color(rgb(p.toolbar_border))
                .bg(rgba(p.toolbar_background))
                .text_color(rgb(p.toolbar_text))
                .text_size(px(9.))
                .child(legend_item(
                    self.locale
                        .text("Fork：继承对话", "Fork: inherits the conversation"),
                    FORK_EDGE,
                    false,
                    compact_legend,
                ))
                .child(legend_item(
                    self.locale.text(
                        "普通关联：不继承对话",
                        "Link: does not inherit the conversation",
                    ),
                    p.legend_derived,
                    true,
                    compact_legend,
                ));
            let legend = legend
                .max_w(px(legend_max_width))
                .when(compact_legend, |legend| {
                    // Stack the full labels when there is too little room for a horizontal legend.
                    legend
                        .flex_col()
                        .items_start()
                        .gap(px(6.))
                        .w(px(legend_max_width))
                        .when(
                            cfg!(target_os = "macos")
                                || (cfg!(target_os = "windows")
                                    && self.locale == InterfaceLocale::English
                                    && legend_max_width < 220.),
                            |legend| {
                                // Reserve room for wrapped labels so the last line
                                // stays inside the legend's border.
                                let height = if legend_max_width < 160. { 92. } else { 68. };
                                legend.min_h(px(height)).line_height(px(13.))
                            },
                        )
                });

            let right = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(12.))
                .child(zoom)
                .child(legend);
            let toolbar = div()
                .absolute()
                .top(px(14.))
                .right(self.toolbar_right_inset)
                .flex()
                .items_start()
                .justify_between();
            #[cfg(target_os = "windows")]
            let toolbar = toolbar
                .left(px(WINDOWS_TOOLBAR_LEFT))
                .child(search)
                .child(
                    div()
                        .id("dag-toolbar-drag")
                        .flex_1()
                        .min_w(px(WINDOWS_TOOLBAR_DRAG_MIN))
                        .h(px(38.))
                        .window_control_area(WindowControlArea::Drag)
                        .occlude(),
                )
                .child(right.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()))
                .when(!self.initial_focus_done, |toolbar| {
                    let view = cx.entity().downgrade();
                    toolbar.on_children_prepainted(move |children, _, cx| {
                        let bottom = children
                            .iter()
                            .map(|bounds| f64::from(bounds.bottom()))
                            .reduce(f64::max);
                        let _ = view.update(cx, |this, _| this.initial_toolbar_bottom = bottom);
                    })
                });
            #[cfg(not(target_os = "windows"))]
            let toolbar = toolbar
                .left(px(MAC_TOOLBAR_LEFT))
                .gap(px(12.))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(search)
                .child(right);
            toolbar
        }
    }

    impl Render for DagCanvasView {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.ensure_search_subscription(window, cx);
            let p = Palette::new(self.dark);
            let toolbar = self.render_toolbar(window, cx);
            let transform = self.display_transform();
            if transform != self.canvas.transform {
                window.request_animation_frame();
            } else {
                self.centering = None;
            }
            let scale = transform.scale as f32;
            let tx = transform.x;
            let ty = transform.y;
            let viewport = self.canvas.viewport;
            let layout_height = self.canvas.layout.height;
            let world_bounds = self.canvas.world_bounds();
            let model = self.canvas.render_model();
            let roots: Vec<DagPoint> = self
                .canvas
                .layout
                .depth_nodes(0)
                .iter()
                .filter(|node| {
                    node.rect.y + node.rect.height >= world_bounds.top
                        && node.rect.y <= world_bounds.bottom
                })
                .map(|node| DagPoint {
                    x: node.rect.x,
                    y: node.rect.y + node.rect.height / 2.,
                })
                .collect();
            let edges: Vec<(DagPoint, DagPoint, bool)> = model
                .edges
                .iter()
                .map(|edge| (edge.from, edge.to, edge.relation_kind == "forked-from"))
                .collect();

            let entity = cx.entity().downgrade();
            let measure = canvas(
                move |bounds, window, cx| {
                    let size = DagSize {
                        width: f64::from(bounds.size.width),
                        height: f64::from(bounds.size.height),
                    };
                    let schedule = entity
                        .update(cx, |this, cx| this.measure(bounds, size, cx))
                        .unwrap_or(false);
                    if schedule {
                        window.request_animation_frame();
                        let entity = entity.clone();
                        window.on_next_frame(move |_, cx| {
                            let _ = entity.update(cx, |this, cx| this.finish_initial_focus(cx));
                        });
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full();

            let grid = canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let width = f32::from(bounds.size.width);
                    let height = f32::from(bounds.size.height);
                    let mut y = 1.;
                    while y < height {
                        let mut x = 1.;
                        while x < width {
                            window.paint_quad(fill(
                                Bounds {
                                    origin: bounds.origin + point(px(x), px(y)),
                                    size: size(px(2.), px(2.)),
                                },
                                rgba(p.grid),
                            ));
                            x += 22.;
                        }
                        y += 22.;
                    }
                },
            )
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full();

            let edge_layer = canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let origin_x = bounds.origin.x;
                    let origin_y = bounds.origin.y;
                    window.with_content_mask(Some(ContentMask { bounds }), |window| {
                        // The canvas can be embedded at a non-zero window origin.
                        let screen = |x: f64, y: f64| {
                            point(
                                origin_x + px((tx + x * f64::from(scale)) as f32),
                                origin_y + px((ty + y * f64::from(scale)) as f32),
                            )
                        };
                        for root in &roots {
                            let mut guide = PathBuilder::stroke(px((1.5 * scale).max(1.)));
                            guide = guide
                                .dash_array(&[px((4. * scale).max(1.)), px((4. * scale).max(1.))]);
                            guide.move_to(screen(12., layout_height * 0.5));
                            guide.cubic_bezier_to(
                                screen(root.x, root.y),
                                screen(28., layout_height * 0.5),
                                screen(32., root.y),
                            );
                            if let Ok(path) = guide.build() {
                                window.paint_path(path, rgba(ROOT_GUIDE));
                            }
                        }
                        for (from, to, fork) in &edges {
                            let start = screen(from.x, from.y);
                            let end = screen(to.x, to.y);
                            let mid = (from.x + to.x) / 2.;
                            let mut path = PathBuilder::stroke(px((2. * scale).max(1.)));
                            if !*fork {
                                path = path.dash_array(&[
                                    px((6. * scale).max(2.)),
                                    px((4. * scale).max(1.)),
                                ]);
                            }
                            path.move_to(start);
                            path.cubic_bezier_to(end, screen(mid, from.y), screen(mid, to.y));
                            if let Ok(path) = path.build() {
                                window.paint_path(
                                    path,
                                    rgb(if *fork { FORK_EDGE } else { DERIVED_EDGE }),
                                );
                            }
                            let arrow_size = (9. * scale).max(3.);
                            let mut arrow = PathBuilder::fill();
                            arrow.move_to(end);
                            arrow.line_to(point(
                                end.x - px(arrow_size),
                                end.y - px(arrow_size * 0.5),
                            ));
                            arrow.line_to(point(
                                end.x - px(arrow_size),
                                end.y + px(arrow_size * 0.5),
                            ));
                            arrow.close();
                            if let Ok(path) = arrow.build() {
                                window.paint_path(path, rgb(ARROW_EDGE));
                            }
                        }
                    });
                },
            )
            .absolute()
            .left(px(0.))
            .top(px(0.))
            .size_full();

            let mut cards = Vec::with_capacity(model.real_nodes.len());
            for positioned in &model.real_nodes {
                let x = (tx + positioned.rect.x * f64::from(scale)) as f32;
                let y = (ty + positioned.rect.y * f64::from(scale)) as f32;
                let width = positioned.rect.width as f32 * scale;
                let height = positioned.rect.height as f32 * scale;
                if x + width < -24.
                    || y + height < -24.
                    || x > viewport.width as f32 + 24.
                    || y > viewport.height as f32 + 24.
                {
                    continue;
                }
                let focused = positioned.node.session_id == self.canvas.preview_session_id;
                let notified = !positioned.node.archived
                    && self.notifications.contains(&positioned.node.session_id);
                cards.push(node_card(
                    positioned,
                    focused,
                    notified,
                    scale,
                    (tx, ty),
                    &p,
                    self.locale,
                    cx,
                ));
            }

            let mut aggregates = Vec::with_capacity(model.aggregates.len());
            for aggregate in &model.aggregates {
                let x = (tx + aggregate.rect.x * f64::from(scale)) as f32;
                let y = (ty + aggregate.rect.y * f64::from(scale)) as f32;
                let width = aggregate.rect.width as f32 * scale;
                let height = aggregate.rect.height as f32 * scale;
                if x + width < -24.
                    || y + height < -24.
                    || x > viewport.width as f32 + 24.
                    || y > viewport.height as f32 + 24.
                {
                    continue;
                }
                aggregates.push(aggregate_card(
                    aggregate,
                    scale,
                    (tx, ty),
                    &p,
                    self.locale,
                    cx,
                ));
            }

            div()
                .id("dag-canvas-view")
                .relative()
                .size_full()
                .overflow_hidden()
                .bg(rgb(p.canvas))
                .cursor(if self.drag.is_some() {
                    CursorStyle::ClosedHand
                } else {
                    CursorStyle::OpenHand
                })
                .track_focus(&self.focus)
                .on_key_down(cx.listener(Self::key_down))
                .on_mouse_down(MouseButton::Left, cx.listener(Self::mouse_down))
                .on_mouse_move(cx.listener(Self::mouse_move))
                .on_mouse_up(MouseButton::Left, cx.listener(Self::mouse_up))
                .on_mouse_up_out(MouseButton::Left, cx.listener(Self::mouse_up_out))
                .on_scroll_wheel(cx.listener(Self::scroll_wheel))
                .on_magnify(cx.listener(Self::magnify))
                .child(measure)
                .child(grid)
                .child(edge_layer)
                .children(cards)
                .children(aggregates)
                .child(toolbar)
        }
    }

    fn node_card(
        positioned: &DagLayoutNode,
        focused: bool,
        notified: bool,
        scale: f32,
        offset: (f64, f64),
        p: &Palette,
        locale: InterfaceLocale,
        cx: &mut Context<DagCanvasView>,
    ) -> AnyElement {
        let node = &positioned.node;
        let (tx, ty) = offset;
        let x = (tx + positioned.rect.x * f64::from(scale)) as f32;
        let y = (ty + positioned.rect.y * f64::from(scale)) as f32;
        let width = positioned.rect.width as f32 * scale;
        let height = positioned.rect.height as f32 * scale;
        let stopped = node.archived;
        let visual_status = if stopped {
            "exited"
        } else {
            node.work_status.as_str()
        };
        let border = if notified {
            0x2588ff
        } else if focused {
            p.focus
        } else if stopped {
            p.stopped_border
        } else {
            p.card_border
        };
        let background = if stopped {
            p.stopped_background
        } else {
            p.card_background
        };
        let text = if stopped { p.stopped_text } else { p.card_text };
        let title_color = if stopped { p.stopped_title } else { p.title };
        let pre_text = if stopped {
            p.stopped_pre_text
        } else {
            p.pre_text
        };
        let pre_background = if stopped {
            p.stopped_pre_background
        } else {
            p.pre_background
        };
        let dot = if stopped {
            p.stopped_text
        } else {
            status_dot_color(visual_status)
        };
        let branch = node.branch.as_deref().or(node.git_branch.as_deref());
        let path = branch.map_or_else(
            || node.cwd.clone(),
            |branch| {
                if node.git_dirty {
                    format!("{branch}*")
                } else {
                    branch.to_owned()
                }
            },
        );
        let lines = if node.latest_lines.is_empty() {
            String::new()
        } else {
            let start = node.latest_lines.len().saturating_sub(4);
            node.latest_lines[start..].join("\n")
        };
        let lines = if lines.is_empty() {
            awaiting_output(locale).to_owned()
        } else {
            lines
        };
        let child_count = node.active_child_count + node.stopped_child_count;
        let meta = format!(
            "{}{} · {}",
            child_count_label(child_count, locale),
            if stopped {
                locale.text(" · 已停止", " · Stopped")
            } else {
                ""
            },
            activity_label(node, locale)
        );
        let shared = if branch.is_some() {
            locale.text("共享工作树", "Shared worktree")
        } else {
            locale.text("共享目录", "Shared directory")
        };
        let monospace = if cfg!(target_os = "macos") {
            "SF Mono"
        } else {
            "Consolas"
        };
        let compact_details = !(40..=49).contains(&((f64::from(scale) * 100.).round() as u8));
        let session_id = node.session_id.clone();
        let selector = format!("dag-node-{session_id}");
        let mut shadows = Vec::new();
        if notified {
            shadows.push(shadow(0x2588ff, 0.32, 0., 0., 2.));
        } else if focused {
            shadows.push(shadow(p.focus, 0.24, 0., 0., 2.));
        }
        shadows.push(shadow(
            p.card_shadow,
            if focused {
                p.focus_shadow_alpha
            } else {
                p.card_shadow_alpha
            },
            (14. * scale).max(2.),
            (34. * scale).max(4.),
            0.,
        ));

        let status_dot = if stopped {
            div()
                .w(px((6. * scale).max(4.)))
                .h(px((6. * scale).max(4.)))
                .flex_none()
                .rounded(px(3.))
                .border_1()
                .border_color(rgb(0x757e89))
        } else {
            div()
                .w(px((6. * scale).max(4.)))
                .h(px((6. * scale).max(4.)))
                .flex_none()
                .rounded(px(3.))
                .bg(rgb(dot))
        };

        let card = div()
            .id(SharedString::from(selector))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .rounded(px((12. * scale).max(6.)))
            .border_1()
            .border_color(rgb(border))
            .bg(rgb(background))
            .overflow_hidden()
            .shadow(shadows)
            .cursor(CursorStyle::PointingHand)
            .when(!notified, |card| {
                card.hover(|style| style.border_color(rgb(0x596575)))
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window);
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.select_card(&session_id, window, cx);
            }))
            .child(
                div()
                    .relative()
                    .size_full()
                    .flex()
                    .flex_col()
                    .gap(px((5. * scale).max(2.)))
                    .p(px((12. * scale).max(5.)))
                    .text_color(rgb(text))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(px((5. * scale).max(2.)))
                            .text_color(rgb(if stopped { p.stopped_text } else { p.top }))
                            .text_size(px((9. * scale).max(7.)))
                            .child(status_dot)
                            .child(ui_text(status_label(visual_status, locale)))
                            .child(
                                div()
                                    .ml_auto()
                                    .child(ui_text(mode_label(&node.current_mode, locale))),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_color(rgb(title_color))
                            .text_size(px((13. * scale).max(9.)))
                            .line_height(relative(1.35))
                            .font_weight(FontWeight::BOLD)
                            .whitespace_normal()
                            .child(ui_text(node.title.clone())),
                    )
                    .when(compact_details, |card| {
                        card.child(
                            div()
                                .flex_none()
                                .w_full()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .text_color(rgb(p.path))
                                .text_size(px((9. * scale).max(7.)))
                                .child(ui_text(path)),
                        )
                    })
                    .when(branch.is_some() && compact_details, |card| {
                        card.child(
                            div()
                                .flex_none()
                                .w_full()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis()
                                .mt(px(-5. * scale))
                                .text_color(rgb(p.cwd))
                                .text_size(px((8. * scale).max(6.5)))
                                .child(ui_text(compact_path(&node.cwd))),
                        )
                    })
                    .when(compact_details, |card| {
                        card.child(
                            div()
                                .flex_none()
                                .w_full()
                                .h(px((48. * scale).max(20.)))
                                .overflow_hidden()
                                .p(px((6. * scale).max(2.)))
                                .rounded(px((6. * scale).max(3.)))
                                .bg(rgb(pre_background))
                                .font_family(monospace)
                                .text_color(rgb(pre_text))
                                .text_size(px((9. * scale).max(6.5)))
                                .line_height(relative(1.5))
                                .whitespace_normal()
                                .child(lines.clone()),
                        )
                    })
                    .when(compact_details, |card| {
                        card.child(
                            div()
                                .flex_none()
                                .text_color(rgb(p.meta))
                                .text_size(px((9. * scale).max(7.)))
                                .child(ui_text(meta.clone())),
                        )
                    })
                    .when(node.shared, |card| {
                        card.child(
                            div()
                                .absolute()
                                .right(px((11. * scale).max(4.)))
                                .bottom(px((10. * scale).max(3.)))
                                .px(px((5. * scale).max(2.)))
                                .py(px((2. * scale).max(1.)))
                                .rounded(px((7. * scale).max(3.)))
                                .bg(rgb(p.shared_background))
                                .text_color(rgb(p.shared_text))
                                .text_size(px((8. * scale).max(6.)))
                                .child(ui_text(shared)),
                        )
                    }),
            )
            .when(notified, |card| {
                card.child(
                    div()
                        .absolute()
                        .top(px((8. * scale).max(3.)))
                        .right(px((8. * scale).max(3.)))
                        .w(px((10. * scale).max(4.)))
                        .h(px((10. * scale).max(4.)))
                        .rounded(px(5.))
                        .bg(rgb(0x1677e8)),
                )
            });

        card.into_any_element()
    }

    fn aggregate_card(
        aggregate: &DagAggregate<'_>,
        scale: f32,
        offset: (f64, f64),
        p: &Palette,
        locale: InterfaceLocale,
        cx: &mut Context<DagCanvasView>,
    ) -> AnyElement {
        let (tx, ty) = offset;
        let x = (tx + aggregate.rect.x * f64::from(scale)) as f32;
        let y = (ty + aggregate.rect.y * f64::from(scale)) as f32;
        let width = aggregate.rect.width as f32 * scale;
        let height = aggregate.rect.height as f32 * scale;
        let target = aggregate.target_session_id.to_owned();
        let count = aggregate.session_ids.len();
        let from = aggregate.minimum_depth + 1;
        let to = aggregate.maximum_depth + 1;
        let key = aggregate.key.clone();
        let eyebrow = match aggregate.kind {
            DagAggregateKind::Branch => locale.text("远层分支", "Distant branch"),
            DagAggregateKind::LayerOverflow => locale.text("远层层级", "Distant level"),
        };
        let counts = aggregate.counts;
        let status_border = if counts.error > 0 {
            Some(0xe66b64)
        } else if counts.needs_input > 0 {
            Some(0xe9ae43)
        } else if counts.running > 0 {
            Some(0x4cbd78)
        } else {
            None
        };
        let selector = format!("dag-aggregate-{key}");

        div()
            .id(SharedString::from(selector))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(width))
            .h(px(height))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px((9. * scale).max(3.)))
            .overflow_hidden()
            .p(px((16. * scale).max(6.)))
            .rounded(px((12. * scale).max(6.)))
            .border_1()
            .border_dashed()
            .border_color(rgb(status_border.unwrap_or(p.aggregate_border)))
            .bg(rgba(p.aggregate_background))
            .shadow(vec![shadow(
                p.card_shadow,
                0.09,
                (10. * scale).max(2.),
                (28. * scale).max(4.),
                0.,
            )])
            .cursor(CursorStyle::PointingHand)
            .hover(|style| style.bg(rgb(p.aggregate_hover)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| {
                    this.focus.focus(window);
                    cx.stop_propagation();
                }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.center_node(&target, window, cx);
            }))
            .child(
                div()
                    .text_color(rgb(p.aggregate_eyebrow))
                    .text_size(px((9. * scale).max(7.)))
                    .child(ui_text(eyebrow)),
            )
            .child(
                div()
                    .text_color(rgb(p.aggregate_title))
                    .text_size(px((15. * scale).max(9.)))
                    .font_weight(FontWeight::BOLD)
                    .child(ui_text(session_total(count, locale))),
            )
            .child(
                div()
                    .text_color(rgb(p.aggregate_range))
                    .text_size(px((9. * scale).max(7.)))
                    .child(ui_text(depth_range(from, to, locale))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px((5. * scale).max(2.)))
                    .text_color(rgb(p.aggregate_count))
                    .text_size(px((8. * scale).max(6.)))
                    .whitespace_nowrap()
                    .child(count_dot(STATUS_ACTIVE, scale))
                    .child(ui_text(count_running(counts.running, locale)))
                    .child(count_dot(STATUS_INPUT, scale))
                    .child(ui_text(count_needs_input(counts.needs_input, locale)))
                    .child(count_dot(STATUS_ERROR, scale))
                    .child(ui_text(count_error(counts.error, locale))),
            )
            .into_any_element()
    }

    fn count_dot(color: u32, scale: f32) -> Div {
        div()
            .w(px((5. * scale).max(3.)))
            .h(px((5. * scale).max(3.)))
            .rounded(px(3.))
            .bg(rgb(color))
    }

    fn legend_item(label: &'static str, color: u32, dashed: bool, compact: bool) -> Div {
        let item = div().flex().items_center().gap(px(5.)).child(
            canvas(
                |_, _, _| (),
                move |bounds, _, window, _| {
                    let y = bounds.origin.y + bounds.size.height * 0.5;
                    let mut path = PathBuilder::stroke(px(2.));
                    if dashed {
                        path = path.dash_array(&[px(4.), px(3.)]);
                    }
                    path.move_to(point(bounds.origin.x, y));
                    path.line_to(point(bounds.origin.x + bounds.size.width, y));
                    if let Ok(path) = path.build() {
                        window.paint_path(path, rgb(color));
                    }
                },
            )
            .w(px(18.))
            .h(px(4.))
            .flex_none(),
        );
        if compact {
            return item
                .w_full()
                .min_w_0()
                .flex_none()
                .child(div().flex_1().min_w_0().child(ui_text(label)));
        }
        item.child(ui_text(label))
    }

    fn count_running(count: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("运行中 {count}"),
            InterfaceLocale::English => format!("{count} running"),
        }
    }

    fn count_needs_input(count: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("等待输入 {count}"),
            InterfaceLocale::English if count == 1 => "1 needs input".to_owned(),
            InterfaceLocale::English => format!("{count} need input"),
        }
    }

    fn count_error(count: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("异常 {count}"),
            InterfaceLocale::English if count == 1 => "1 error".to_owned(),
            InterfaceLocale::English => format!("{count} errors"),
        }
    }

    fn session_total(count: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("共 {count} 个会话"),
            InterfaceLocale::English if count == 1 => "1 session in total".to_owned(),
            InterfaceLocale::English => format!("{count} sessions in total"),
        }
    }

    fn depth_range(from: usize, to: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("第 {from}–{to} 层 · 点击展开"),
            InterfaceLocale::English if from == to => {
                format!("Level {from} · click to expand")
            }
            InterfaceLocale::English => format!("Levels {from}–{to} · click to expand"),
        }
    }

    fn make_search_input(
        dark: bool,
        text: String,
        placeholder: &str,
        cx: &mut Context<RenameInput>,
    ) -> RenameInput {
        if dark {
            // RenameInput's session-loader style is the only public dark field
            // palette; it also leaves the field unbounded.
            let mut input = RenameInput::session_loader_search(placeholder, cx);
            input.text = text;
            input
        } else {
            let mut input = RenameInput::new(text, cx);
            input.search_style = true;
            input.placeholder = placeholder.to_owned();
            input
        }
    }

    fn search_placeholder(locale: InterfaceLocale) -> &'static str {
        locale.text(
            "搜索名称、路径、分支或输出…",
            "Search by name, path, branch or output…",
        )
    }

    #[cfg(any(target_os = "windows", test))]
    fn windows_search_placeholder(locale: InterfaceLocale, search_width: f32) -> &'static str {
        if search_width < 260. {
            locale.text("搜索会话…", "Search sessions…")
        } else {
            search_placeholder(locale)
        }
    }

    fn search_empty(locale: InterfaceLocale) -> &'static str {
        locale.text("没有匹配的会话", "No matching sessions")
    }

    fn awaiting_output(locale: InterfaceLocale) -> &'static str {
        locale.text("等待会话输出…", "Waiting for session output…")
    }

    fn child_count_label(count: usize, locale: InterfaceLocale) -> String {
        match locale {
            InterfaceLocale::Chinese => format!("子会话 {count}"),
            InterfaceLocale::English if count == 1 => "1 child session".to_owned(),
            InterfaceLocale::English => format!("{count} child sessions"),
        }
    }

    fn status_label(status: &str, locale: InterfaceLocale) -> &'static str {
        match status {
            "starting" => locale.text("运行中", "Running"),
            "idle" => locale.text("空闲", "Idle"),
            "running" => locale.text("运行中", "Running"),
            "needs-input" => locale.text("等待输入", "Needs input"),
            "error" => locale.text("异常", "Error"),
            "interrupted" => locale.text("中断", "Interrupted"),
            "exited" => locale.text("已停止", "Stopped"),
            _ => "",
        }
    }

    fn mode_label(mode: &str, locale: InterfaceLocale) -> &'static str {
        match mode {
            "shell" => "Shell",
            "claude-code" => "Claude",
            "codex" => "Codex",
            "antigravity" => "Antigravity",
            "dsh" => "DSH",
            "agent-team-member" => locale.text("队友", "Teammate"),
            _ => "",
        }
    }

    fn status_dot_color(status: &str) -> u32 {
        match status {
            "running" | "starting" => STATUS_ACTIVE,
            "needs-input" => STATUS_INPUT,
            "error" => STATUS_ERROR,
            _ => STATUS_IDLE,
        }
    }

    fn activity_label(node: &DagNode, locale: InterfaceLocale) -> String {
        if let Some(activity) = node.last_activity_at.filter(|value| *value > 0) {
            if let Some(clock) = clock_time(activity) {
                return match locale {
                    InterfaceLocale::Chinese => format!("最近活动 {clock}"),
                    InterfaceLocale::English => format!("Last activity {clock}"),
                };
            }
        }
        match locale {
            InterfaceLocale::Chinese => format!("活动记录 #{}", node.last_user_interaction_seq),
            InterfaceLocale::English => {
                format!("Activity record #{}", node.last_user_interaction_seq)
            }
        }
    }

    fn clock_time(milliseconds: i64) -> Option<String> {
        crate::database_recovery_page::format_backup_time(milliseconds)
            .rsplit_once(' ')
            .map(|(_, clock)| clock.to_owned())
    }

    fn compact_path(value: &str) -> String {
        if value.chars().count() <= 42 {
            return value.to_owned();
        }
        let parts = value
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() > 1 {
            format!("…/{}", parts[parts.len() - 2..].join("/"))
        } else {
            value.to_owned()
        }
    }

    fn transform_tuple(transform: DagTransform) -> (f64, f64, f64) {
        (transform.x, transform.y, transform.scale)
    }

    fn shadow(color: u32, alpha: f32, y: f32, blur: f32, spread: f32) -> BoxShadow {
        let mut color: Hsla = rgb(color).into();
        color.a = alpha;
        BoxShadow {
            color,
            offset: point(px(0.), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
        }
    }

    struct TooltipText {
        text: &'static str,
        canvas: WeakEntity<DagCanvasView>,
    }

    impl TooltipText {
        fn builder(
            text: &'static str,
            canvas: WeakEntity<DagCanvasView>,
        ) -> impl Fn(&mut Window, &mut App) -> AnyView {
            move |_, cx| {
                cx.new(|_| Self {
                    text,
                    canvas: canvas.clone(),
                })
                .into()
            }
        }

        fn element(text: &'static str, dark: bool) -> Div {
            card_tooltip_box(dark)
                .max_w(px(320.))
                .whitespace_normal()
                .child(ui_text(text))
        }
    }

    impl Render for TooltipText {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let dark = self
                .canvas
                .upgrade()
                .is_some_and(|canvas| canvas.read(cx).dark);
            Self::element(self.text, dark)
        }
    }

    #[cfg(test)]
    #[::core::prelude::v1::test]
    fn dag_canvas_tooltip_uses_light_and_dark_surfaces() {
        for (dark, background, border, text) in [
            (false, 0xfffffffa, 0xd0d7de, 0x57606a),
            (true, 0x1c2128ff, 0x30363d, 0xc9d1d9),
        ] {
            let mut element = TooltipText::element("Zoom out", dark);
            let style = element.style();
            assert_eq!(style.background, Some(rgba(background).into()));
            assert_eq!(style.border_color, Some(rgb(border).into()));
            assert_eq!(style.text.as_ref().unwrap().color, Some(rgb(text).into()));
        }
    }

    #[cfg(test)]
    #[::core::prelude::v1::test]
    fn dag_search_placeholder_switches_at_allocated_width_in_both_languages() {
        let (narrow, _) = windows_toolbar_widths(725.);
        let (wide, _) = windows_toolbar_widths(1065.);
        assert_eq!(
            windows_search_placeholder(InterfaceLocale::Chinese, narrow),
            "搜索会话…"
        );
        assert_eq!(
            windows_search_placeholder(InterfaceLocale::English, narrow),
            "Search sessions…"
        );
        for locale in [InterfaceLocale::Chinese, InterfaceLocale::English] {
            assert_eq!(
                windows_search_placeholder(locale, 260.),
                search_placeholder(locale)
            );
            assert_eq!(
                windows_search_placeholder(locale, wide),
                search_placeholder(locale)
            );
        }
    }

    #[cfg(test)]
    #[::core::prelude::v1::test]
    fn dag_search_placeholder_preserves_focused_input_and_query() {
        use gpui::TestAppContext;

        let graph = DagGraph::from_snapshot(&serde_json::json!({
            "sceneId": "scene",
            "nodes": [{"sessionId": "root", "title": "Draft", "cwd": "/fixture"}],
            "edges": [],
        }))
        .unwrap();
        let state = DagCanvas::new(DagLayout::new(graph).unwrap(), "root", None);
        let mut cx = TestAppContext::single();
        let (view, cx) =
            cx.add_window_view(|_, cx| DagCanvasView::new(state, InterfaceLocale::Chinese, cx));
        cx.update(|window, cx| {
            view.update(cx, |view, cx| view.focus_search(window, cx));
            window.refresh();
        });
        cx.run_until_parked();
        cx.simulate_keystrokes("d r a f t");
        let editor = view.read_with(cx, |view, _| view.search_input.clone());

        for (locale, viewport_width) in [
            (InterfaceLocale::Chinese, 725.),
            (InterfaceLocale::English, 1065.),
            (InterfaceLocale::English, 725.),
        ] {
            let (search_width, _) = windows_toolbar_widths(viewport_width);
            let placeholder = windows_search_placeholder(locale, search_width);
            #[cfg(target_os = "windows")]
            cx.simulate_resize(size(px(viewport_width), px(600.)));
            view.update(cx, |view, cx| {
                view.set_locale(locale, cx);
                #[cfg(not(target_os = "windows"))]
                view.set_search_placeholder(placeholder, cx);
            });
            cx.update(|window, _| window.refresh());
            cx.run_until_parked();
            cx.update(|window, cx| {
                let view = view.read(cx);
                assert!(view.search_input == editor);
                assert!(view.search_focused(window, cx));
                assert_eq!(view.search_input.read(cx).placeholder, placeholder);
                assert_eq!(view.search_input.read(cx).text, "draft");
                assert_eq!(view.canvas.search.query, "draft");
            });
        }
    }

    #[cfg(test)]
    #[::core::prelude::v1::test]
    fn trackpad_pinch_zooms_around_the_pointer() {
        use gpui::TestAppContext;

        let graph = DagGraph::from_snapshot(&serde_json::json!({
            "sceneId": "scene",
            "runtimeGeneration": "first",
            "eventSequence": 1,
            "nodes": [{"sessionId": "root", "title": "Draft", "cwd": "/fixture"}],
            "edges": [],
        }))
        .unwrap();
        let state = DagCanvas::new(DagLayout::new(graph).unwrap(), "root", None);
        let mut cx = TestAppContext::single();
        let (view, cx) =
            cx.add_window_view(|_, cx| DagCanvasView::new(state, InterfaceLocale::Chinese, cx));
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        view.update(cx, |view, cx| {
            view.canvas.transform = DagTransform {
                x: 10.,
                y: 20.,
                scale: 1.,
            };
            cx.notify();
        });
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();

        cx.simulate_event(MagnifyEvent {
            position: point(px(300.), px(200.)),
            magnification: 0.5,
            ..Default::default()
        });

        let transform = view.read_with(cx, |view, _| view.canvas.transform);
        assert!(
            (transform.scale - 1.5).abs() < 1e-9,
            "pinch should scale the canvas: {transform:?}"
        );
        // The zoom keeps the pointer anchored: the same world point stays under
        // the cursor when the gesture grows the canvas.
        let cursor = DagPoint { x: 300., y: 200. };
        let world_scale = 1.5;
        let expected = DagPoint {
            x: cursor.x - (cursor.x - 10.) / 1. * world_scale,
            y: cursor.y - (cursor.y - 20.) / 1. * world_scale,
        };
        assert!(
            (transform.x - expected.x).abs() < 1e-9 && (transform.y - expected.y).abs() < 1e-9,
            "pinch should zoom around the pointer: {transform:?}"
        );
    }
}

pub use view::{DagCanvasEvent, DagCanvasView};
