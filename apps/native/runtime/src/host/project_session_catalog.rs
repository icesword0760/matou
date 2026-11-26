use std::path::Path;

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    antigravity_catalog, codex_catalog, dsh_catalog, provider_catalog::project_scope::ProjectScope,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectSessionProvider {
    Codex,
    Dsh,
    Antigravity,
}

impl ProjectSessionProvider {
    pub fn kind(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Dsh => "dsh",
            Self::Antigravity => "antigravity",
        }
    }
}

#[derive(Serialize, Deserialize)]
pub enum ProjectSessionCatalogOperation {
    List {
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
    Detail {
        provider_session_id: String,
        start_event_index: Option<i64>,
        before_event_index: Option<i64>,
        around_event_index: Option<i64>,
        limit: Option<i64>,
    },
    Search {
        provider_session_id: String,
        query: String,
        offset: Option<i64>,
        limit: Option<i64>,
    },
}

pub(super) struct ProjectCatalogRequest {
    pub request_id: u64,
    pub provider: ProjectSessionProvider,
    pub cwd: String,
    pub workspace_root: Option<String>,
    pub operation: ProjectSessionCatalogOperation,
}

impl ProjectCatalogRequest {
    pub fn scope(&self) -> Result<ProjectScope> {
        project_scope(&self.cwd, self.workspace_root.as_deref()).map_err(|error| anyhow!(error))
    }

    pub fn is_list(&self) -> bool {
        matches!(self.operation, ProjectSessionCatalogOperation::List { .. })
    }

    pub fn reply(&self, result: Result<Value>) -> Value {
        let mut value = result.unwrap_or_else(|error| json!({"error": error.to_string()}));
        value["provider"] = json!(self.provider.kind());
        value["requestId"] = json!(self.request_id);
        value["cwd"] = json!(self.cwd);
        match &self.operation {
            ProjectSessionCatalogOperation::List { .. } => {}
            ProjectSessionCatalogOperation::Detail {
                provider_session_id,
                ..
            } => {
                value["providerSessionId"] = json!(provider_session_id);
            }
            ProjectSessionCatalogOperation::Search {
                provider_session_id,
                ..
            } => {
                value["providerSessionId"] = json!(provider_session_id);
                value["search"] = json!(true);
            }
        }
        value
    }

    pub fn run(
        &self,
        codex: &mut codex_catalog::Catalog,
        dsh: Option<&mut dsh_catalog::DshSessionCatalog>,
        antigravity: &mut antigravity_catalog::Catalog,
    ) -> Result<Value> {
        use ProjectSessionCatalogOperation::{Detail, List, Search};
        use ProjectSessionProvider::{Antigravity, Codex, Dsh};

        let scope = self.scope()?;
        let cwd = self.cwd.as_str();
        let path = Path::new(cwd);
        let mut value = match (&self.operation, self.provider) {
            (
                List {
                    query,
                    offset,
                    limit,
                },
                Codex,
            ) => {
                let page = codex
                    .list_in_project(
                        cwd,
                        &scope,
                        &codex_catalog::ListOptions {
                            query: query.clone(),
                            full_text: false,
                            offset: *offset,
                            limit: *limit,
                        },
                    )
                    .map_err(|error| anyhow!(error))?;
                let mut value = json!(page.window);
                value["sessions"] = json!(page.items);
                value
            }
            (
                List {
                    query,
                    offset,
                    limit,
                },
                Dsh,
            ) => json!(
                dsh.context("DSH session directory is unavailable")?
                    .list_in_project(
                        path,
                        &scope,
                        dsh_catalog::ListOptions {
                            query,
                            scope: dsh_catalog::SearchScope::Metadata,
                            offset: *offset,
                            limit: *limit,
                            ..Default::default()
                        }
                    )?
            ),
            (
                List {
                    query,
                    offset,
                    limit,
                },
                Antigravity,
            ) => json!(antigravity.list_in_project(
                path,
                &scope,
                antigravity_catalog::ListOptions {
                    query,
                    scope: antigravity_catalog::SearchScope::Metadata,
                    offset: offset.unwrap_or_default(),
                    limit: *limit,
                    ..Default::default()
                },
            )?),
            (
                Detail {
                    provider_session_id,
                    start_event_index,
                    around_event_index,
                    limit,
                    ..
                },
                Codex,
            ) => codex
                .detail_in_project(
                    cwd,
                    &scope,
                    provider_session_id,
                    *start_event_index,
                    *around_event_index,
                    *limit,
                )
                .map_err(|error| anyhow!(error))?,
            (
                Detail {
                    provider_session_id,
                    before_event_index,
                    around_event_index,
                    limit,
                    ..
                },
                Dsh,
            ) => json!(
                dsh.context("DSH session directory is unavailable")?
                    .detail_in_project(
                        path,
                        &scope,
                        provider_session_id,
                        dsh_catalog::DetailOptions {
                            before_event_index: *before_event_index,
                            around_event_index: *around_event_index,
                            limit: *limit,
                            ..Default::default()
                        },
                    )?
            ),
            (
                Detail {
                    provider_session_id,
                    before_event_index,
                    around_event_index,
                    limit,
                    ..
                },
                Antigravity,
            ) => json!(antigravity.detail_in_project(
                path,
                &scope,
                provider_session_id,
                antigravity_catalog::DetailOptions {
                    before_event_index: *before_event_index,
                    around_event_index: *around_event_index,
                    limit: *limit,
                    ..Default::default()
                },
            )?),
            (
                Search {
                    provider_session_id,
                    query,
                    offset,
                    limit,
                },
                Codex,
            ) => {
                let page = codex
                    .search_in_project(cwd, &scope, provider_session_id, query, *offset, *limit)
                    .map_err(|error| anyhow!(error))?;
                let mut value = json!(page.window);
                value["hits"] = json!(page.items);
                value
            }
            (
                Search {
                    provider_session_id,
                    query,
                    offset,
                    limit,
                },
                Dsh,
            ) => json!(
                dsh.context("DSH session directory is unavailable")?
                    .search_in_project(path, &scope, provider_session_id, query, *offset, *limit,)?
            ),
            (
                Search {
                    provider_session_id,
                    query,
                    offset,
                    limit,
                },
                Antigravity,
            ) => json!(antigravity.search_in_project(
                path,
                &scope,
                provider_session_id,
                query,
                offset.unwrap_or_default(),
                *limit,
            )?),
        };
        if self.is_list() {
            value["projectRoot"] = json!(scope.project_root());
        }
        Ok(value)
    }
}

pub(super) fn project_scope(
    cwd: &str,
    workspace_root: Option<&str>,
) -> Result<ProjectScope, &'static str> {
    ProjectScope::discover(Path::new(cwd), workspace_root.map(Path::new))
        .map_err(|_| "Could not resolve the catalog project directory")
}
