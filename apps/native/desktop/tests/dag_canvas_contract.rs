use std::sync::LazyLock;

use matou_native_core::{session_graph::project_scene_graph, startup};
use matou_native_desktop::dag_canvas::{
    DagAggregateKind, DagCanvas, DagError, DagGraph, DagGraphUpdate, DagLayout, DagLayoutEdge,
    DagLayoutNode, DagNode, DagPoint, DagRect, DagRelation, DagRenderInput, DagRenderModel,
    DagScope, DagTransform, DagWorldBounds, MAX_LAYOUT_NODES, build_render_model,
};
use serde_json::{Value, json};

static REFERENCE: LazyLock<Value> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../core/tests/fixtures/dag-reference.json")).unwrap()
});

fn case(id: &str) -> &'static Value {
    let cases = REFERENCE["cases"].as_array().unwrap();
    assert_eq!(REFERENCE["schemaVersion"], 1);
    assert_eq!(REFERENCE["gate"], "K-dag-fixture");
    assert_eq!(cases.len(), 16);
    let mut matches = cases.iter().filter(|case| case["id"] == id);
    let case = matches
        .next()
        .unwrap_or_else(|| panic!("missing case {id}"));
    assert!(matches.next().is_none(), "duplicate case {id}");
    case
}

fn str_at(value: &Value) -> &str {
    value.as_str().unwrap()
}

fn number(value: &Value) -> f64 {
    value.as_f64().unwrap()
}

fn uint(value: &Value) -> usize {
    value.as_u64().unwrap().try_into().unwrap()
}

fn array(value: &Value) -> &[Value] {
    value.as_array().unwrap()
}

fn graph(value: &Value) -> DagGraph {
    DagGraph::from_snapshot(value).unwrap()
}

fn case_graph(case: &Value) -> DagGraph {
    graph(&case["frozen"]["graph"])
}

fn dependency_graph(case: &Value) -> DagGraph {
    let id = str_at(&case["input"]["graphCase"]);
    case_graph(self::case(id))
}

fn assert_seed_ids(case: &Value, layout: &DagLayout) {
    let seed = array(&case["input"]["seed"]);
    assert_eq!(layout.nodes.len(), seed.len());
    for input in seed {
        let actual = &layout.node(str_at(&input["id"])).unwrap().node;
        assert_eq!(actual.title, input["title"]);
        assert_eq!(
            actual.sibling_created_seq.unwrap(),
            input["seq"].as_i64().unwrap()
        );
        assert_eq!(
            actual.parent_session_id.as_deref(),
            input["parent"].as_str()
        );
        if let Some(lines) = input.get("lines") {
            assert_eq!(json!(actual.latest_lines), *lines);
        }
    }
}

fn coordinate(value: f64) -> Value {
    if value.fract() == 0. {
        json!(value as i64)
    } else {
        json!(value)
    }
}

fn assert_rect(actual: DagRect, expected: &Value) {
    assert_eq!(actual.x, number(&expected["x"]));
    assert_eq!(actual.y, number(&expected["y"]));
    assert_eq!(actual.width, number(&expected["width"]));
    assert_eq!(actual.height, number(&expected["height"]));
}

fn node_row(actual: &DagLayoutNode) -> Value {
    let rect = actual.rect;
    json!([
        actual.node.session_id,
        actual.depth,
        coordinate(rect.x),
        coordinate(rect.y),
        coordinate(rect.width),
        coordinate(rect.height)
    ])
}

fn edge_object(actual: &DagLayoutEdge) -> Value {
    json!({
        "fromSessionId": actual.from_session_id, "toSessionId": actual.to_session_id,
        "relationKind": actual.relation_kind,
        "from": {"x": coordinate(actual.from.x), "y": coordinate(actual.from.y)},
        "to": {"x": coordinate(actual.to.x), "y": coordinate(actual.to.y)},
    })
}

fn assert_layout(layout: &DagLayout, expected: &Value) {
    assert_eq!(layout.width, number(&expected["width"]));
    assert_eq!(layout.height, number(&expected["height"]));
    assert_eq!(layout.depth_count, uint(&expected["depthCount"]));
    assert_eq!(layout.nodes.len(), array(&expected["nodes"]).len());
    assert_eq!(layout.edges.len(), array(&expected["edges"]).len());
    if expected.get("nodeColumns").is_some() {
        assert_eq!(
            expected["nodeColumns"],
            json!(["sessionId", "depth", "x", "y", "width", "height"])
        );
        assert_eq!(
            expected["edgeColumns"],
            json!([
                "fromSessionId",
                "toSessionId",
                "relationKind",
                "fromX",
                "fromY",
                "toX",
                "toY"
            ])
        );
        for (actual, expected) in layout.nodes.iter().zip(array(&expected["nodes"])) {
            assert_eq!(node_row(actual), *expected);
        }
        for (actual, expected) in layout.edges.iter().zip(array(&expected["edges"])) {
            assert_eq!(
                json!([
                    actual.from_session_id,
                    actual.to_session_id,
                    actual.relation_kind,
                    coordinate(actual.from.x),
                    coordinate(actual.from.y),
                    coordinate(actual.to.x),
                    coordinate(actual.to.y)
                ]),
                *expected
            );
        }
        return;
    }
    for (actual, expected) in layout.nodes.iter().zip(array(&expected["nodes"])) {
        assert_eq!(actual.node.session_id, expected["sessionId"]);
        assert_eq!(actual.depth, uint(&expected["depth"]));
        assert_rect(actual.rect, expected);
    }
    let edges: Vec<_> = layout.edges.iter().map(edge_object).collect();
    assert_eq!(json!(edges), expected["edges"]);
    let ids: Vec<_> = layout.nodes.iter().map(|n| &n.node.session_id).collect();
    assert_eq!(json!(ids), expected["nodeByIdOrder"]);
    let depths: Vec<_> = (0..layout.depth_count)
        .filter_map(|depth| {
            let ids: Vec<_> = layout
                .depth_nodes(depth)
                .iter()
                .map(|n| &n.node.session_id)
                .collect();
            (!ids.is_empty()).then_some((depth, ids))
        })
        .collect();
    assert_eq!(json!(depths), expected["nodesByDepth"]);
}

fn assert_layers(layout: &DagLayout, focus: &str, expected: &Value) {
    let (full, ghost) = layout.visible_layers(focus, 1);
    assert_eq!(json!(full), expected["fullDepths"]);
    assert_eq!(json!(ghost), expected["ghostDepths"]);
}

fn render(layout: &DagLayout, input: &Value) -> Value {
    let depths: Vec<_> = array(&input["fullDepths"]).iter().map(uint).collect();
    let bounds = &input["worldBounds"];
    let model = build_render_model(
        layout,
        DagRenderInput {
            full_depths: &depths,
            world_bounds: DagWorldBounds {
                left: number(&bounds["left"]),
                right: number(&bounds["right"]),
                top: number(&bounds["top"]),
                bottom: number(&bounds["bottom"]),
            },
            center_world_y: number(&input["centerWorldY"]),
            preview_session_id: str_at(&input["previewSessionId"]),
            max_items: input.get("maxItems").map_or(400, uint),
            max_edges: input.get("maxEdges").map_or(800, uint),
        },
    );
    let real: Vec<_> = model
        .real_nodes
        .iter()
        .map(|n| &n.node.session_id)
        .collect();
    let aggregates: Vec<_> = model.aggregates.iter().map(|item| json!({
        "key": item.key,
        "kind": match item.kind { DagAggregateKind::Branch => "branch", DagAggregateKind::LayerOverflow => "layer-overflow" },
        "direction": item.direction.as_str(),
        "branchRootId": item.branch_root_id,
        "targetSessionId": item.target_session_id,
        "sessionIds": item.session_ids,
        "sessionCount": item.session_ids.len(),
        "counts": {"running": item.counts.running, "needsInput": item.counts.needs_input, "error": item.counts.error},
        "x": coordinate(item.rect.x), "y": coordinate(item.rect.y),
        "width": coordinate(item.rect.width), "height": coordinate(item.rect.height),
        "minimumDepth": item.minimum_depth, "maximumDepth": item.maximum_depth,
    })).collect();
    let edges: Vec<_> = model.edges.into_iter().map(edge_object).collect();
    json!({"realNodes": real, "aggregates": aggregates, "edges": edges})
}

fn search_ids(layout: &DagLayout, query: &str) -> Vec<String> {
    layout
        .search(query)
        .into_iter()
        .map(|node| node.session_id.clone())
        .collect()
}

#[test]
fn dag_canvas_l01_two_root_cards() {
    let case = case("L01-two-root-cards");
    let layout = DagLayout::new(case_graph(case)).unwrap();
    assert_seed_ids(case, &layout);
    assert_layout(&layout, &case["frozen"]["layout"]);
    assert_layers(
        &layout,
        str_at(&case["input"]["focus"]),
        &case["frozen"]["layers"],
    );
}

#[test]
fn dag_canvas_l02_multi_root_fork_and_depth() {
    let case = case("L02-multi-root-fork-and-depth");
    let layout = DagLayout::new(case_graph(case)).unwrap();
    assert_seed_ids(case, &layout);
    assert_layout(&layout, &case["frozen"]["layout"]);
    for focus in array(&case["input"]["focus"]).iter().map(str_at) {
        assert_layers(&layout, focus, &case["frozen"]["layers"][focus]);
    }
}

#[test]
fn dag_canvas_s01_search_ranking() {
    let case = case("S01-search-ranking");
    let layout = DagLayout::new(dependency_graph(case)).unwrap();
    let queries = array(&case["input"]["queries"]);
    let frozen = array(&case["frozen"]["results"]);
    assert_eq!(queries.len(), frozen.len());
    for (query, expected) in queries.iter().zip(frozen) {
        assert_eq!(query, &expected["query"]);
        let ids: Vec<_> = array(&expected["nodes"])
            .iter()
            .map(|node| str_at(&node["sessionId"]))
            .collect();
        assert_eq!(search_ids(&layout, str_at(query)), ids, "query {query}");
    }
}

#[test]
fn dag_canvas_a01_branches_overflow_and_totals() {
    let case = case("A01-branch-and-overflow-totals");
    let layout = DagLayout::new(dependency_graph(case)).unwrap();
    let expected = &case["frozen"];
    assert_eq!(layout.nodes.len(), uint(&expected["sessionCount"]));
    for child in array(&expected["children"]) {
        let node = &layout.node(str_at(&child["sessionId"])).unwrap().node;
        assert_eq!(node.active_child_count, uint(&child["activeChildCount"]));
        assert_eq!(node.stopped_child_count, uint(&child["stoppedChildCount"]));
        assert_eq!(
            node.shell_child_count,
            uint(&child["childModeCounts"]["shell"])
        );
        assert_eq!(
            node.claude_code_child_count,
            uint(&child["childModeCounts"]["claudeCode"])
        );
    }
    let inputs = array(&case["input"]["renderInputs"]);
    let results = array(&expected["renders"]);
    assert_eq!(inputs.len(), results.len());
    for (input, expected) in inputs.iter().zip(results) {
        assert_eq!(render(&layout, input), *expected);
    }
}

#[test]
fn dag_canvas_l03_empty_scene() {
    let case = case("L03-empty-scene");
    let layout = DagLayout::new(case_graph(case)).unwrap();
    assert_seed_ids(case, &layout);
    assert_layout(&layout, &case["frozen"]["layout"]);
    assert_layers(
        &layout,
        str_at(&case["input"]["focus"]),
        &case["frozen"]["layers"],
    );
    assert_eq!(
        json!(search_ids(&layout, str_at(&case["input"]["query"]))),
        case["frozen"]["search"]
    );
}

fn chain(input: &Value) -> DagGraph {
    let count = uint(&input["count"]);
    let prefix = str_at(&input["idPrefix"]);
    let title = str_at(&input["title"]);
    let pad = uint(&input["zeroPadWidth"]);
    let seq_start = uint(&input["sequenceStart"]);
    let kind = str_at(&input["relationKind"]);
    assert_eq!(
        input["inputOrder"],
        "descending index; parent is preceding index, index 0 has no parent"
    );
    let id = |index: usize| format!("{prefix}{index:0pad$}");
    let nodes = (0..count)
        .rev()
        .map(|index| DagNode {
            session_id: id(index),
            parent_session_id: index.checked_sub(1).map(id),
            sibling_created_seq: Some((seq_start + index).try_into().unwrap()),
            title: title.to_owned(),
            cwd: str_at(&input["nodeTemplate"]["cwd"]).to_owned(),
            current_mode: str_at(&input["nodeTemplate"]["currentMode"]).to_owned(),
            work_status: str_at(&input["nodeTemplate"]["workStatus"]).to_owned(),
            ..DagNode::default()
        })
        .collect();
    let edges = (1..count)
        .rev()
        .map(|index| DagRelation {
            parent_session_id: id(index - 1),
            child_session_id: id(index),
            relation_kind: kind.to_owned(),
        })
        .collect();
    DagGraph {
        scene_id: str_at(&input["sceneId"]).to_owned(),
        focused_session_id: None,
        runtime_generation: None,
        layout_revision: None,
        event_sequence: None,
        nodes,
        edges,
    }
}

#[test]
fn dag_canvas_l04_reverse_chain_10000() {
    let case = case("L04-reverse-chain-10000");
    let input = &case["input"];
    let expected = &case["frozen"];
    let layout = DagLayout::new(chain(input)).unwrap();
    assert_layout(&layout, &expected["layout"]);
    assert_layers(&layout, str_at(&input["focus"]), &expected["layers"]);
    assert_eq!(render(&layout, &input["render"]), expected["render"]);
    assert_eq!(
        json!(search_ids(&layout, str_at(&input["query"]))),
        expected["searchSessionIds"]
    );
}

#[test]
fn dag_canvas_f01_bad_summary_and_requests() {
    let case = case("F01-malformed-seeds-and-requests");
    let expected = &case["frozen"];
    let input = &case["input"];
    let mut snapshot = expected["graph"].clone();
    let raw_summary = &input["seed"][0]["malformedSummary"];
    assert!(serde_json::from_str::<Value>(str_at(raw_summary)).is_err());
    snapshot["nodes"][0]["latestLines"] = raw_summary.clone();
    let layout = DagLayout::new(graph(&snapshot)).unwrap();
    assert_seed_ids(case, &layout);
    assert_eq!(
        json!(layout.nodes[0].node.latest_lines),
        expected["graph"]["nodes"][0]["latestLines"]
    );
    assert_eq!(
        json!(search_ids(&layout, str_at(&input["query"]))),
        expected["search"]
    );

    let scope = DagScope::from_payload(&input["validPayload"]).unwrap();
    assert!(scope.initial_layout(&expected["graph"]).is_ok());
    let requests = array(&input["badRequests"]);
    let responses = array(&expected["responses"]);
    assert_eq!(requests.len(), responses.len());
    with_empty_db(|connection| {
        for (request, response) in requests.iter().zip(responses) {
            let (code, error) = match DagScope::from_payload(request) {
                Err(error) => ("INVALID_REQUEST", error.to_string()),
                Ok(scope) => ("NOT_FOUND", connection(&scope).unwrap_err().to_string()),
            };
            assert_fault(error, code, &response["error"]);
        }
    });
}

fn assert_fault(message: String, code: &str, expected: &Value) {
    assert_eq!(
        json!({"name":"RpcFault", "message":message, "code":code, "retryable":false}),
        *expected
    );
}

fn relation_candidate(mut graph: DagGraph, input: &Value) -> DagGraph {
    let child = str_at(&input["fromSessionId"]);
    let parent = str_at(&input["toSessionId"]);
    graph
        .nodes
        .iter_mut()
        .find(|node| node.session_id == child)
        .unwrap()
        .parent_session_id = Some(parent.to_owned());
    graph.edges.push(DagRelation {
        child_session_id: child.to_owned(),
        parent_session_id: parent.to_owned(),
        relation_kind: str_at(&input["kind"]).to_owned(),
    });
    graph
}

#[test]
fn dag_canvas_f02_missing_relation_endpoint() {
    let case = case("F02-missing-relation-endpoint");
    let input = &case["input"];
    let expected = &case["frozen"];
    let renderer_graph = graph(&input["rendererGraph"]);
    let before = dependency_graph(case);
    let candidate = relation_candidate(before.clone(), &input["rpcPayload"]["input"]);
    let error = DagLayout::complete(candidate).unwrap_err();
    assert_eq!(error, DagError::MissingEndpoint);
    assert_fault(error.to_string(), "CONFLICT", &expected["write"]["error"]);
    assert_layout(
        &DagLayout::new(renderer_graph).unwrap(),
        &expected["layout"],
    );
    let unchanged = graph(&expected["graphAfter"]);
    assert_eq!(unchanged.event_sequence, before.event_sequence);
    assert_eq!(unchanged.edges.len(), before.edges.len());
    assert_layout(&DagLayout::new(unchanged).unwrap(), &expected["layout"]);
}

#[test]
fn dag_canvas_f03_structural_cycle() {
    let case = case("F03-structural-cycle");
    let renderer_graph = graph(&case["input"]["rendererGraph"]);
    let expected = &case["frozen"];
    let before = graph(&expected["graphAfter"]);
    let candidate = relation_candidate(before.clone(), &case["input"]["rpcPayload"]["input"]);
    let error = DagLayout::complete(candidate).unwrap_err();
    assert_eq!(
        DagLayout::new(renderer_graph.clone()).unwrap_err(),
        DagError::Cycle
    );
    assert_eq!(error, DagError::Cycle);
    assert_fault(error.to_string(), "CONFLICT", &expected["write"]["error"]);
    assert_layout(
        &DagLayout::frozen_coordinates(renderer_graph).unwrap(),
        &expected["layout"],
    );
    let unchanged = DagLayout::complete(before).unwrap();
    assert_seed_ids(case, &unchanged);
    assert_eq!(unchanged.edges.len(), 2);
}

#[test]
fn dag_canvas_f04_extreme_depth_layout() {
    let case = case("F04-extreme-depth-layout");
    let input = &case["input"];
    let count = uint(&input["count"]);
    let error = DagLayout::new(chain(input)).unwrap_err();
    assert!(count > MAX_LAYOUT_NODES);
    assert_eq!(error, DagError::LayoutLimit { nodes: count });
    assert_eq!(
        json!({"name": "RangeError", "message": error.to_string()}),
        case["frozen"]["error"]
    );
}

// The transport is owned by DagWindow. Use fixture wire records as input to
// the canvas, and compare its host-facing selection and transform with them.
fn trace_records<'a>(case: &'a Value, key: &'a str) -> impl Iterator<Item = &'a Value> {
    array(&case["frozen"]["trace"])
        .iter()
        .filter_map(move |item| item.get(key))
}

fn graph_snapshots(case: &Value) -> impl Iterator<Item = &Value> {
    trace_records(case, "runtimeToRenderer").filter_map(|message| {
        let graph = &message["result"];
        graph["nodes"].is_array().then_some(graph)
    })
}

fn graph_requests(case: &Value) -> impl Iterator<Item = &Value> {
    trace_records(case, "rendererToRuntime")
        .filter(|message| message["method"] == "hierarchy.get-scene-session-graph")
        .map(|message| &message["payload"])
}

fn step<'a>(case: &'a Value, label: &str) -> &'a Value {
    array(&case["frozen"]["steps"])
        .iter()
        .find(|step| step["label"] == label)
        .unwrap_or_else(|| panic!("missing {label}"))
}

fn canvas_for(case: &Value, graph: DagGraph) -> DagCanvas {
    DagCanvas::new(
        DagLayout::new(graph).unwrap(),
        str_at(&case["input"]["context"]["sessionId"]),
        None,
    )
}

fn assert_viewport(canvas: &DagCanvas, expected: &Value) {
    let viewport = &expected["viewport"];
    let (x, y) = str_at(&viewport["pan"]).split_once(',').unwrap();
    let near = |actual: f64, frozen: &str| {
        assert!(
            (actual - frozen.parse::<f64>().unwrap()).abs() < 1e-9,
            "{actual} != {frozen}"
        )
    };
    near(canvas.transform.x, x);
    near(canvas.transform.y, y);
    near(canvas.transform.scale, str_at(&viewport["scale"]));

    let model = canvas.render_model();
    assert_eq!(
        model.real_nodes.len().to_string(),
        viewport["renderedNodeCount"]
    );
    assert_eq!(
        model.aggregates.len().to_string(),
        viewport["renderedAggregateCount"]
    );
    let node_ids: Vec<_> = model
        .real_nodes
        .iter()
        .map(|n| n.node.session_id.as_str())
        .collect();
    let expected_ids: Vec<_> = array(&expected["nodes"])
        .iter()
        .map(|n| str_at(&n["id"]))
        .collect();
    assert_eq!(node_ids, expected_ids, "{}", expected["label"]);
    let keys: Vec<_> = model.aggregates.iter().map(|n| n.key.as_str()).collect();
    let expected_keys: Vec<_> = array(&expected["aggregates"])
        .iter()
        .map(|n| str_at(&n["key"]))
        .collect();
    assert_eq!(keys, expected_keys, "{}", expected["label"]);
    if let Some(focused) = array(&expected["nodes"])
        .iter()
        .find(|n| str_at(&n["class"]).contains("is-focused"))
    {
        assert_eq!(canvas.preview_session_id, focused["id"]);
    }
}

fn assert_dropdown(canvas: &DagCanvas, expected: &Value) {
    let dropdown = &expected["dropdown"];
    if dropdown.is_null() {
        assert!(canvas.search.query.is_empty());
        return;
    }
    let options = array(&dropdown["options"]);
    assert_eq!(canvas.search.visible_results().len(), options.len());
    assert_eq!(
        canvas.search.results.is_empty(),
        dropdown.get("empty").is_some()
    );
    for (index, (id, option)) in canvas
        .search
        .visible_results()
        .iter()
        .zip(options)
        .enumerate()
    {
        assert_eq!(canvas.layout.node(id).unwrap().node.title, option["title"]);
        assert_eq!(
            if canvas.search.selected == index {
                "true"
            } else {
                "false"
            },
            option["selected"]
        );
    }
}

fn assert_selection(
    case: &Value,
    selection: &matou_native_desktop::dag_canvas::DagSelection,
    canvas: &DagCanvas,
) {
    let ipc = array(&case["frozen"]["trace"])
        .iter()
        .find(|message| message["ipcInvoke"] == "matou:select-dag-node")
        .unwrap();
    let args = &ipc["args"][0];
    assert_eq!(selection.session_id, args["sessionId"]);
    assert_eq!(
        selection.target_window_id.as_deref(),
        args["targetWindowId"].as_str()
    );
    let last_geometry = trace_records(case, "rendererToRuntime")
        .filter(|message| message["method"] == "geometry.put")
        .last()
        .unwrap();
    let payload = &last_geometry["payload"];
    assert_eq!(
        canvas.layout.layout_revision,
        payload["layoutRevision"].as_u64()
    );
    assert_eq!(canvas.layout.scene_id(), payload["sceneId"]);
    let geometry = &payload["geometry"];
    assert_eq!(canvas.transform.x, number(&geometry["panX"]));
    assert_eq!(canvas.transform.y, number(&geometry["panY"]));
    assert_eq!(canvas.transform.scale, number(&geometry["zoom"]));
}

#[test]
fn dag_canvas_s02_keyboard_locate_and_owner_target() {
    let case = case("S02-dropdown-keyboard-locate-and-ipc");
    let mut canvas = canvas_for(case, dependency_graph(case));
    let actions = array(&case["input"]["actions"]);
    let steps = array(&case["frozen"]["steps"]);
    assert_eq!(actions.len() + 2, steps.len());
    assert_viewport(&canvas, &steps[1]);
    let mut chosen = None;
    for (action, expected) in actions.iter().zip(&steps[2..]) {
        match str_at(&action[0]) {
            "frame" => assert!(canvas.focus_initial()),
            "type" => canvas.set_query(str_at(&action[1])),
            "key" => match str_at(&action[1]) {
                "ArrowDown" => canvas.search.move_selection(1),
                "ArrowUp" => canvas.search.move_selection(-1),
                "Enter" => {
                    chosen = canvas.choose_search(None);
                    assert_eq!(
                        chosen.is_some(),
                        expected["label"] != "enter-on-zero-results"
                    );
                }
                key => panic!("unexpected key {key}"),
            },
            "hold-next-geometry" | "release-geometry" => assert!(chosen.is_some()),
            action => panic!("unexpected action {action}"),
        }
        assert_viewport(&canvas, expected);
        assert_dropdown(&canvas, expected);
    }
    assert_selection(case, &chosen.unwrap(), &canvas);
}

#[test]
fn dag_canvas_s03_twelve_visible_thirteenth_selectable() {
    let case = case("S03-dropdown-display-cap");
    let mut canvas = canvas_for(case, graph(graph_snapshots(case).next().unwrap()));
    assert_seed_ids(case, &canvas.layout);
    let actions = array(&case["input"]["actions"]);
    let steps = array(&case["frozen"]["steps"]);
    assert_eq!(actions.len() + 2, steps.len());
    assert_viewport(&canvas, &steps[1]);
    let mut chosen = None;
    for (action, expected) in actions.iter().zip(&steps[2..]) {
        match (str_at(&action[0]), action[1].as_str()) {
            ("frame", _) if chosen.is_none() => assert!(canvas.focus_initial()),
            ("frame", _) => assert_selection(case, chosen.as_ref().unwrap(), &canvas),
            ("type", Some(query)) => {
                canvas.set_query(query);
                assert_eq!(
                    json!(canvas.search.results),
                    case["frozen"]["fullSearchSessionIds"]
                );
            }
            ("key", Some("ArrowUp")) => {
                canvas.search.move_selection(-1);
                assert_eq!(canvas.search.selected, 12);
            }
            ("key", Some("Enter")) => chosen = canvas.choose_search(None),
            _ => panic!("unexpected action {action}"),
        }
        assert_viewport(&canvas, expected);
        assert_dropdown(&canvas, expected);
    }
    assert_eq!(chosen.unwrap().session_id, canvas.search.results[12]);
}

#[test]
fn dag_canvas_a02_aggregate_locates_without_owner_selection() {
    let case = case("A02-aggregate-card-messages-and-locate");
    let mut canvas = canvas_for(case, dependency_graph(case));
    let steps = array(&case["frozen"]["steps"]);
    assert_viewport(&canvas, &steps[1]);
    for (action, expected) in array(&case["input"]["actions"]).iter().zip(&steps[2..]) {
        match str_at(&action[0]) {
            "frame" => assert!(canvas.focus_initial()),
            "zoom-in" => canvas.zoom_at(canvas.transform.scale + 0.1, canvas.viewport.center()),
            "click-first-aggregate" => {
                let target = canvas.render_model().aggregates[0]
                    .target_session_id
                    .to_owned();
                assert!(canvas.focus_node(&target));
            }
            "zoom-reset" => canvas.reset_transform(),
            "key" => assert_eq!(action[1], "Escape"),
            action => panic!("unexpected action {action}"),
        }
        assert_viewport(&canvas, expected);
    }
    assert!(trace_records(case, "ipcInvoke").all(|name| name != "matou:select-dag-node"));
    assert!(trace_records(case, "ipcInvoke").any(|name| name == "matou:close-dag-window"));
}

fn event_graph(batch: &Value, event: &Value) -> DagGraph {
    let mut graph = graph(&event["payload"]["graph"]);
    graph.runtime_generation = Some(str_at(&batch["runtimeGeneration"]).to_owned());
    graph.event_sequence = event["sequence"].as_u64();
    graph
}

fn assert_background(canvas: &DagCanvas, expected: &Value) {
    let node = &canvas.layout.node("background").unwrap().node;
    let projected = array(&expected["nodes"])
        .iter()
        .find(|n| n["id"] == "background")
        .unwrap();
    assert!(str_at(&projected["class"]).contains(&format!("status-{}", node.work_status)));
    assert_eq!(node.latest_lines.join("\n"), projected["summary"]);
    assert_viewport(canvas, expected);
}

#[test]
fn dag_canvas_e01_event_burst_and_reconnect() {
    let case = case("E01-graph-event-burst-and-reconnect");
    let mut snapshots = graph_snapshots(case);
    let mut canvas = canvas_for(case, graph(snapshots.next().unwrap()));
    assert_seed_ids(case, &canvas.layout);
    assert!(canvas.focus_initial());
    let mut batches = trace_records(case, "runtimeToRenderer")
        .filter(|message| message["type"] == "events.batch");
    for batch in batches.by_ref().take(3) {
        for event in array(&batch["events"]) {
            let seq = uint(&event["sequence"]);
            assert_eq!(event, &case["frozen"]["storedEvents"][seq - 1]);
            let expected = if seq == 1 && batch["events"].as_array().unwrap().len() == 1 {
                DagGraphUpdate::Ignored
            } else {
                DagGraphUpdate::Queued
            };
            assert_eq!(canvas.queue_graph(event_graph(batch, event)), expected);
        }
    }
    assert_background(&canvas, step(case, "batches-arrive-before-frame"));
    assert!(canvas.apply_graph_frame().unwrap());
    assert_eq!(canvas.layout.event_sequence(), Some(3));
    assert_background(&canvas, step(case, "frame-applies-sequence-3"));
    let stale = batches.next().unwrap();
    assert_eq!(
        canvas.queue_graph(event_graph(stale, &stale["events"][0])),
        DagGraphUpdate::Ignored
    );
    assert!(!canvas.apply_graph_frame().unwrap());
    assert_background(&canvas, step(case, "frame-after-stale-event"));
    canvas.begin_reconnect();
    assert_background(&canvas, step(case, "reconnecting-keeps-last-graph"));
    assert!(
        canvas
            .apply_snapshot(graph(snapshots.next().unwrap()))
            .unwrap()
    );
    assert!(snapshots.next().is_none());
    assert_eq!(canvas.layout.event_sequence(), Some(0));
    assert_background(&canvas, step(case, "ready-refetches-and-resubscribes"));
    let query = &case["input"]["actions"].as_array().unwrap().last().unwrap()[1];
    canvas.set_query(str_at(query));
    assert_dropdown(&canvas, step(case, "search-resumed-background-output"));
}

#[test]
fn dag_canvas_f05_invalid_initial_context_and_wrong_scene_reopen() {
    let case = case("F05-invalid-ipc-context-fallback");
    let context = &case["input"]["context"];
    let scope = DagScope::from_context(context).unwrap();
    assert_eq!(
        scope.initial_layout(&context["initialGraph"]).unwrap_err(),
        DagError::InvalidGraph("initialGraph")
    );
    let requests: Vec<_> = graph_requests(case)
        .map(|payload| DagScope::from_payload(payload).unwrap())
        .collect();
    assert_eq!(requests, [scope.clone()]);
    let fallback = graph_snapshots(case).next().unwrap();
    let layout = scope.initial_layout(fallback).unwrap();
    let mut canvas = DagCanvas::new(layout, str_at(&context["sessionId"]), None);
    assert!(canvas.focus_initial());
    assert_viewport(&canvas, step(case, "initial-focus-frame"));

    let reopen = &case["input"]["actions"][1][1];
    assert_eq!(DagScope::from_context(reopen).unwrap(), scope);
    assert_eq!(
        scope.initial_layout(&reopen["initialGraph"]).unwrap_err(),
        DagError::WrongScene
    );
    let invalid = step(case, "reopen-wrong-scene-graph");
    assert!(invalid["viewport"].is_null());
    assert_eq!(invalid["state"], step(case, "mount-request")["state"]);
    let range = uint(&invalid["traceFrom"])..uint(&invalid["traceToExclusive"]);
    assert!(array(&case["frozen"]["trace"])[range].iter().all(
        |message| message["rendererToRuntime"]["method"] != "hierarchy.get-scene-session-graph"
    ));
}

#[test]
fn dag_canvas_f06_missing_scene_retries_same_scope() {
    let case = case("F06-no-scene-window-retry");
    let requested = DagScope::from_context(&case["input"]["context"]).unwrap();
    let requests: Vec<_> = graph_requests(case)
        .map(|payload| DagScope::from_payload(payload).unwrap())
        .collect();
    assert_eq!(requests, [requested.clone(), requested]);
    let responses: Vec<_> = trace_records(case, "runtimeToRenderer")
        .filter(|message| message["type"] == "rpc.error")
        .collect();
    assert_eq!(requests.len(), responses.len());
    with_empty_db(|query| {
        for (scope, response) in requests.iter().zip(responses) {
            let error = query(scope).unwrap_err();
            assert_eq!(error.to_string(), response["message"]);
            assert_eq!(response["code"], "NOT_FOUND");
            assert_eq!(response["retryable"], false);
        }
    });
    let before = step(case, "open-and-ready");
    let retry = step(case, "retry-missing-scene");
    assert_eq!(before["state"], retry["state"]);
    assert!(retry["viewport"].is_null());
}

fn with_empty_db(run: impl FnOnce(&dyn Fn(&DagScope) -> anyhow::Result<Value>)) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target")
        .join(format!("dag-contract-{}", uuid::Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    let startup::BootstrapResult::Writable {
        connection,
        ownership: _ownership,
        ..
    } = startup::open(&root, "dag-fixture").unwrap()
    else {
        panic!("expected writable isolated fixture database");
    };
    run(&|scope| project_scene_graph(&connection, &scope.scene_id, scope.window_id.as_deref()));
}

fn ids(model: &DagRenderModel<'_>) -> Vec<String> {
    model
        .real_nodes
        .iter()
        .map(|node| node.node.session_id.clone())
        .collect()
}

#[test]
fn dag_canvas_zoom_pan_culling_and_recenter() {
    let case = case("A02-aggregate-card-messages-and-locate");
    let mut canvas = canvas_for(case, dependency_graph(case));
    let anchor = DagPoint { x: 151., y: 235. };
    let old = canvas.transform;
    canvas.zoom_at(12., anchor);
    let zoomed = canvas.transform;
    assert_eq!(zoomed.scale, 2.);
    assert_eq!(
        (anchor.x - zoomed.x) / zoomed.scale,
        (anchor.x - old.x) / old.scale
    );
    assert_eq!(
        (anchor.y - zoomed.y) / zoomed.scale,
        (anchor.y - old.y) / old.scale
    );
    canvas.zoom_at(0., anchor);
    assert_eq!(canvas.transform.scale, 0.4);
    canvas.reset_transform();
    assert_eq!(canvas.transform, DagTransform::default());
    let initial = ids(&canvas.render_model());
    canvas.pan_by(DagPoint { x: -1300., y: 0. });
    assert_eq!(canvas.transform.x, -1230.);
    let shifted = ids(&canvas.render_model());
    assert_ne!(shifted, initial);
    assert!(!shifted.iter().any(|id| id == "root"));
    assert!(shifted.iter().any(|id| id == "tail"));
    canvas.pan_by(DagPoint { x: 0., y: 50_000. });
    assert!(canvas.render_model().real_nodes.is_empty());
    let target = canvas.render_model().aggregates[0]
        .target_session_id
        .to_owned();
    assert!(canvas.focus_node(&target));
    assert_eq!(canvas.render_model().real_nodes[0].node.session_id, target);
}

#[test]
fn dag_canvas_reconnect_preserves_search_preview_and_selection_identity() {
    let case = case("E01-graph-event-burst-and-reconnect");
    let mut snapshots = graph_snapshots(case);
    let mut first = graph(snapshots.next().unwrap());
    first.event_sequence = Some(17);
    let mut canvas = canvas_for(case, first);
    canvas.set_query("/fixture/project");
    canvas.search.move_selection(-1);
    let selection = canvas.choose_search(None).unwrap();
    assert_eq!(selection.session_id, "background");
    let transform = canvas.transform;
    let mut incoming = graph(snapshots.next().unwrap());
    assert_eq!(
        canvas.queue_graph(incoming.clone()),
        DagGraphUpdate::RefreshRequired
    );
    assert_eq!(
        canvas.queue_graph(incoming.clone()),
        DagGraphUpdate::Ignored
    );
    incoming
        .nodes
        .iter_mut()
        .find(|node| node.session_id == "background")
        .unwrap()
        .sibling_created_seq = Some(0);
    incoming
        .nodes
        .iter_mut()
        .find(|node| node.session_id == "background")
        .unwrap()
        .detached_window_id = Some("detached-fixture".into());
    assert!(canvas.apply_snapshot(incoming).unwrap());
    assert_eq!(canvas.search.query, "/fixture/project");
    assert_eq!(canvas.search.selected, 0);
    assert_eq!(canvas.preview_session_id, "background");
    assert_eq!(canvas.transform, transform);
    assert_eq!(
        canvas
            .choose_search(None)
            .unwrap()
            .target_window_id
            .as_deref(),
        Some("detached-fixture")
    );
    canvas.set_query("unique-reconnected-background-18");
    assert_eq!(canvas.search.results, ["background"]);
}
