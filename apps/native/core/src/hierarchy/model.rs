use std::collections::BTreeMap;

use rusqlite::{Result, Row};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
    pub root_directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_identity: Option<String>,
    pub task_order: Vec<String>,
    pub is_default: bool,
    pub is_pinned: bool,
    pub pin_sort_key: String,
    pub last_opened_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl Workspace {
    pub(super) fn from_row(row: &Row<'_>) -> Result<Self> {
        let order: String = row.get("task_order_json")?;
        Ok(Self {
            id: row.get("id")?,
            name: row.get("name")?,
            root_directory: row.get("root_directory")?,
            path_identity: row.get("path_identity")?,
            task_order: serde_json::from_str(&order).unwrap_or_default(),
            is_default: row.get::<_, i64>("is_default")? == 1,
            is_pinned: row.get::<_, i64>("is_pinned")? == 1,
            pin_sort_key: row.get("pin_sort_key")?,
            last_opened_at: row.get("last_opened_at")?,
            archived_at: row.get("archived_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            version: row.get("version")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    pub workspace_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_task_id: Option<String>,
    pub title: String,
    pub status: String,
    pub execution_context_id: String,
    pub sort_key: String,
    pub is_pinned: bool,
    pub pin_sort_key: String,
    pub last_opened_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub version: i64,
}

impl Task {
    pub(super) fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            workspace_id: row.get("workspace_id")?,
            parent_task_id: row.get("parent_task_id")?,
            title: row.get("title")?,
            status: row.get("status")?,
            execution_context_id: row.get("execution_context_id")?,
            sort_key: row.get("sort_key")?,
            is_pinned: row.get::<_, i64>("is_pinned")? == 1,
            pin_sort_key: row.get("pin_sort_key")?,
            last_opened_at: row.get("last_opened_at")?,
            archived_at: row.get("archived_at")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            version: row.get("version")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scene {
    pub id: String,
    pub task_id: String,
    pub name: String,
    pub mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_node_id: Option<String>,
    pub title_pinned: bool,
    pub sort_key: String,
    pub layout_revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
}

impl Scene {
    pub(super) fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            name: row.get("name")?,
            mode: row.get("mode")?,
            root_node_id: row.get("root_node_id")?,
            title_pinned: row.get::<_, i64>("title_pinned")? == 1,
            sort_key: row.get("sort_key")?,
            layout_revision: row.get("layout_revision")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            archived_at: row.get("archived_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub task_id: String,
    pub execution_context_id: String,
    pub kind: String,
    pub status: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title_source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_title: Option<String>,
    pub cwd: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
    pub version: i64,
}

impl Session {
    pub(crate) fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            task_id: row.get("task_id")?,
            execution_context_id: row.get("execution_context_id")?,
            kind: row.get("kind")?,
            status: row.get("status")?,
            title: row.get("title")?,
            title_source: row.get("title_source")?,
            provider_title: row.get("provider_title")?,
            cwd: row.get("cwd")?,
            created_at: row.get("created_at")?,
            updated_at: row.get("updated_at")?,
            last_activity_at: row.get("last_activity_at")?,
            archived_at: row.get("archived_at")?,
            version: row.get("version")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionContext {
    pub kind: String,
    pub id: String,
    pub workspace_id: String,
    pub cwd: String,
    pub created_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived_at: Option<i64>,
}

impl ExecutionContext {
    pub(super) fn plain_directory_from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self {
            kind: "plain-directory".into(),
            id: row.get("id")?,
            workspace_id: row.get("workspace_id")?,
            cwd: row.get("cwd")?,
            created_at: row.get("created_at")?,
            archived_at: row.get("archived_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub id: String,
    pub scene_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene_window_id: Option<String>,
    pub session_id: String,
    pub created_at: i64,
}

impl Mount {
    pub(super) fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            scene_id: row.get("scene_id")?,
            scene_node_id: row.get("scene_node_id")?,
            scene_window_id: row.get("scene_window_id")?,
            session_id: row.get("session_id")?,
            created_at: row.get("created_at")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowNavigation {
    pub window_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_workspace_id: Option<String>,
    pub task_by_workspace: BTreeMap<String, String>,
    pub scene_by_task: BTreeMap<String, String>,
    pub session_by_scene: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HierarchyResult {
    pub workspace: Option<Workspace>,
    pub execution_context: Option<ExecutionContext>,
    pub task: Option<Task>,
    pub scene: Option<Scene>,
    pub session: Option<Session>,
    pub mount: Option<Mount>,
    pub navigation: WindowNavigation,
}
