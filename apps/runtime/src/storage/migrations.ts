import type { Migration } from './migration-runner'

const FOUNDATION_SCHEMA = `
CREATE TABLE workspaces (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  root_directory TEXT NOT NULL,
  task_order_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;

CREATE TABLE execution_contexts (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  kind TEXT NOT NULL CHECK (kind IN ('plain-directory', 'git-worktree')),
  cwd TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;

CREATE TABLE worktrees (
  id TEXT PRIMARY KEY,
  execution_context_id TEXT NOT NULL UNIQUE REFERENCES execution_contexts(id),
  repository_root TEXT NOT NULL,
  worktree_path TEXT NOT NULL UNIQUE,
  branch_name TEXT NOT NULL,
  base_ref TEXT,
  state TEXT NOT NULL CHECK (state IN ('creating', 'ready', 'dirty', 'retained', 'removing', 'removed', 'failed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  cleanup_policy TEXT NOT NULL DEFAULT 'retain-dirty'
) STRICT;

CREATE TABLE tasks (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  parent_task_id TEXT REFERENCES tasks(id),
  execution_context_id TEXT NOT NULL REFERENCES execution_contexts(id),
  title TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('planned', 'active', 'blocked', 'completed', 'archived')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER,
  CHECK (parent_task_id IS NULL OR parent_task_id <> id)
) STRICT;
CREATE INDEX tasks_workspace_idx ON tasks(workspace_id, archived_at, updated_at);

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  execution_context_id TEXT NOT NULL REFERENCES execution_contexts(id),
  kind TEXT NOT NULL CHECK (kind IN ('shell', 'claude-code', 'codex', 'agent-team-member')),
  status TEXT NOT NULL CHECK (status IN ('created', 'starting', 'running', 'waiting', 'interrupted', 'exited', 'archived')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_activity_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;
CREATE INDEX sessions_task_idx ON sessions(task_id, archived_at, last_activity_at);

CREATE TABLE session_runs (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  ordinal INTEGER NOT NULL,
  runtime_generation TEXT NOT NULL,
  pid INTEGER,
  status TEXT NOT NULL CHECK (status IN ('starting', 'running', 'exited', 'failed', 'interrupted')),
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  exit_code INTEGER,
  signal INTEGER,
  UNIQUE(session_id, ordinal)
) STRICT;

CREATE TABLE provider_bindings (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  provider TEXT NOT NULL CHECK (provider IN ('claude-code', 'codex', 'generic')),
  provider_session_id TEXT NOT NULL,
  resume_state TEXT NOT NULL CHECK (resume_state IN ('unknown', 'available', 'resuming', 'resumed', 'failed', 'expired')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(provider, provider_session_id)
) STRICT;

CREATE TABLE domain_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  event_type TEXT NOT NULL,
  aggregate_type TEXT NOT NULL,
  aggregate_id TEXT NOT NULL,
  workspace_id TEXT REFERENCES workspaces(id),
  task_id TEXT REFERENCES tasks(id),
  session_id TEXT REFERENCES sessions(id),
  payload_json TEXT NOT NULL,
  schema_version INTEGER NOT NULL,
  required_terminal_sequence INTEGER,
  command_id TEXT NOT NULL,
  causation_id TEXT,
  correlation_id TEXT,
  occurred_at INTEGER NOT NULL
) STRICT;
CREATE INDEX domain_events_aggregate_idx ON domain_events(aggregate_type, aggregate_id, seq);
CREATE INDEX domain_events_session_idx ON domain_events(session_id, seq);

CREATE TABLE consumer_cursors (
  consumer_id TEXT PRIMARY KEY,
  last_event_seq INTEGER NOT NULL DEFAULT 0,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE command_deduplication (
  command_id TEXT PRIMARY KEY,
  command_type TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  response_json TEXT NOT NULL,
  first_event_seq INTEGER,
  last_event_seq INTEGER,
  committed_at INTEGER NOT NULL
) STRICT;

CREATE TABLE relation_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  relation_id TEXT NOT NULL,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  event_type TEXT NOT NULL CHECK (event_type IN ('relation-created', 'relation-removed', 'relation-metadata-updated')),
  from_session_id TEXT NOT NULL REFERENCES sessions(id),
  to_session_id TEXT NOT NULL REFERENCES sessions(id),
  kind TEXT NOT NULL CHECK (kind IN ('forked-from', 'depends-on', 'supports', 'blocks', 'team-member-of')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  command_id TEXT NOT NULL,
  occurred_at INTEGER NOT NULL
) STRICT;
CREATE INDEX relation_events_relation_idx ON relation_events(relation_id, seq);

CREATE TABLE session_relations_current (
  relation_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  from_session_id TEXT NOT NULL REFERENCES sessions(id),
  to_session_id TEXT NOT NULL REFERENCES sessions(id),
  kind TEXT NOT NULL CHECK (kind IN ('forked-from', 'depends-on', 'supports', 'blocks', 'team-member-of')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  source_event_seq INTEGER NOT NULL REFERENCES relation_events(seq),
  CHECK (from_session_id <> to_session_id)
) STRICT;
CREATE UNIQUE INDEX one_fork_parent_idx ON session_relations_current(from_session_id) WHERE kind = 'forked-from';

CREATE TABLE scenes (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  name TEXT NOT NULL,
  mode TEXT NOT NULL CHECK (mode IN ('tile', 'card', 'dag')),
  root_node_id TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  archived_at INTEGER
) STRICT;

CREATE TABLE scene_nodes (
  id TEXT PRIMARY KEY,
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  parent_node_id TEXT REFERENCES scene_nodes(id),
  kind TEXT NOT NULL CHECK (kind IN ('root', 'split', 'mount', 'group')),
  direction TEXT CHECK (direction IS NULL OR direction IN ('horizontal', 'vertical')),
  ordinal INTEGER NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE session_mounts (
  id TEXT PRIMARY KEY,
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  scene_node_id TEXT REFERENCES scene_nodes(id),
  session_id TEXT NOT NULL REFERENCES sessions(id),
  created_at INTEGER NOT NULL,
  UNIQUE(scene_id, session_id, id)
) STRICT;

CREATE TABLE scene_windows (
  id TEXT PRIMARY KEY,
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  native_window_key TEXT NOT NULL,
  state TEXT NOT NULL CHECK (state IN ('attached', 'detached', 'closed')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(scene_id, native_window_key)
) STRICT;

CREATE TABLE scene_geometry (
  scene_id TEXT NOT NULL REFERENCES scenes(id),
  owner_key TEXT NOT NULL,
  layout_revision INTEGER NOT NULL,
  geometry_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(scene_id, owner_key)
) STRICT;

CREATE TABLE journal_checkpoints (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id),
  generation INTEGER NOT NULL,
  terminal_sequence INTEGER NOT NULL,
  domain_event_sequence INTEGER NOT NULL,
  screen_epoch INTEGER NOT NULL,
  file_path TEXT NOT NULL,
  checksum TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  valid INTEGER NOT NULL DEFAULT 1 CHECK (valid IN (0, 1)),
  UNIQUE(session_id, generation)
) STRICT;

CREATE TABLE annotations (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  session_id TEXT NOT NULL REFERENCES sessions(id),
  kind TEXT NOT NULL,
  text_snapshot TEXT NOT NULL,
  anchor_json TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('active', 'completed', 'degraded', 'archived')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE artifacts (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  producer_session_id TEXT REFERENCES sessions(id),
  path_identity TEXT NOT NULL,
  media_type TEXT,
  state TEXT NOT NULL CHECK (state IN ('observed', 'produced', 'modified', 'missing', 'archived')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(task_id, path_identity)
) STRICT;

CREATE TABLE validation_runs (
  id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  session_id TEXT REFERENCES sessions(id),
  check_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('queued', 'running', 'passed', 'failed', 'cancelled', 'error')),
  summary_json TEXT NOT NULL DEFAULT '{}',
  started_at INTEGER,
  ended_at INTEGER,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE task_status_entries (
  task_id TEXT NOT NULL REFERENCES tasks(id),
  key TEXT NOT NULL,
  value TEXT NOT NULL,
  runtime_generation TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY(task_id, key)
) STRICT;

CREATE TABLE task_progress (
  task_id TEXT PRIMARY KEY REFERENCES tasks(id),
  progress REAL NOT NULL CHECK (progress >= 0 AND progress <= 100),
  label TEXT,
  runtime_generation TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE task_logs (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  level TEXT NOT NULL CHECK (level IN ('debug', 'info', 'warn', 'error')),
  source TEXT NOT NULL,
  message TEXT NOT NULL,
  runtime_generation TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;
CREATE INDEX task_logs_task_idx ON task_logs(task_id, id);

CREATE TABLE preferences (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE feature_campaign_views (
  campaign_id TEXT NOT NULL,
  campaign_version INTEGER NOT NULL,
  viewed_at INTEGER NOT NULL,
  PRIMARY KEY(campaign_id, campaign_version)
) STRICT;

CREATE TABLE preset_capability_state (
  capability_id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  plugin_id TEXT NOT NULL,
  installed_version TEXT,
  desired_version TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('pending', 'installed', 'suppressed', 'failed', 'drifted')),
  source_fingerprint TEXT,
  last_attempt INTEGER,
  last_error TEXT,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE preset_capability_suppressions (
  capability_id TEXT PRIMARY KEY,
  reason TEXT NOT NULL,
  suppressed_at INTEGER NOT NULL
) STRICT;

CREATE TABLE legacy_import_runs (
  id TEXT PRIMARY KEY,
  source_fingerprint TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL CHECK (status IN ('running', 'completed', 'failed')),
  report_json TEXT NOT NULL DEFAULT '{}',
  started_at INTEGER NOT NULL,
  completed_at INTEGER
) STRICT;

CREATE TABLE legacy_entity_mappings (
  import_run_id TEXT NOT NULL REFERENCES legacy_import_runs(id),
  legacy_type TEXT NOT NULL,
  legacy_id TEXT NOT NULL,
  entity_type TEXT NOT NULL,
  entity_id TEXT NOT NULL,
  PRIMARY KEY(import_run_id, legacy_type, legacy_id)
) STRICT;

CREATE TABLE shadow_repair_queue (
  command_id TEXT PRIMARY KEY,
  mutation_type TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  attempt_count INTEGER NOT NULL DEFAULT 0,
  next_attempt_at INTEGER NOT NULL,
  last_error TEXT,
  created_at INTEGER NOT NULL,
  completed_at INTEGER
) STRICT;
`

const AUTHORITATIVE_MODEL_COMPLETION = `
ALTER TABLE workspaces ADD COLUMN path_identity TEXT;
ALTER TABLE workspaces ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

ALTER TABLE tasks ADD COLUMN sort_key TEXT NOT NULL DEFAULT '';
ALTER TABLE tasks ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

ALTER TABLE worktrees ADD COLUMN base_revision TEXT;
ALTER TABLE worktrees ADD COLUMN setup_policy_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE worktrees ADD COLUMN setup_result_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE worktrees ADD COLUMN retained_at INTEGER;

ALTER TABLE sessions ADD COLUMN title TEXT NOT NULL DEFAULT '';
ALTER TABLE sessions ADD COLUMN version INTEGER NOT NULL DEFAULT 1;

ALTER TABLE session_runs ADD COLUMN profile TEXT NOT NULL DEFAULT 'shell';
ALTER TABLE session_runs ADD COLUMN cols INTEGER NOT NULL DEFAULT 80;
ALTER TABLE session_runs ADD COLUMN rows INTEGER NOT NULL DEFAULT 24;

ALTER TABLE provider_bindings ADD COLUMN validated_at INTEGER;
ALTER TABLE provider_bindings ADD COLUMN invalidated_at INTEGER;

DROP TABLE session_relations_current;
ALTER TABLE relation_events RENAME TO relation_events_v1;

CREATE TABLE session_relation_events (
  sequence INTEGER PRIMARY KEY AUTOINCREMENT,
  event_id TEXT NOT NULL UNIQUE,
  relation_id TEXT NOT NULL,
  operation TEXT NOT NULL CHECK (operation IN ('created', 'revoked', 'restored', 'metadata-updated')),
  task_id TEXT NOT NULL REFERENCES tasks(id),
  from_session_id TEXT NOT NULL REFERENCES sessions(id),
  to_session_id TEXT NOT NULL REFERENCES sessions(id),
  relation_kind TEXT NOT NULL CHECK (relation_kind IN ('forked-from', 'derived-from', 'depends-on', 'supports', 'blocks', 'references', 'team-member-of')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  command_id TEXT NOT NULL,
  occurred_at INTEGER NOT NULL,
  CHECK (from_session_id <> to_session_id)
) STRICT;
CREATE INDEX session_relation_events_relation_idx ON session_relation_events(relation_id, sequence);

INSERT INTO session_relation_events (
  sequence, event_id, relation_id, operation, task_id, from_session_id,
  to_session_id, relation_kind, metadata_json, command_id, occurred_at
)
SELECT seq, 'legacy-relation-event-' || seq, relation_id,
       CASE event_type WHEN 'relation-created' THEN 'created'
                       WHEN 'relation-removed' THEN 'revoked'
                       ELSE 'metadata-updated' END,
       task_id, from_session_id, to_session_id, kind, metadata_json, command_id, occurred_at
FROM relation_events_v1;
DROP TABLE relation_events_v1;

CREATE TABLE session_relations_current (
  relation_id TEXT PRIMARY KEY,
  task_id TEXT NOT NULL REFERENCES tasks(id),
  from_session_id TEXT NOT NULL REFERENCES sessions(id),
  to_session_id TEXT NOT NULL REFERENCES sessions(id),
  relation_kind TEXT NOT NULL CHECK (relation_kind IN ('forked-from', 'derived-from', 'depends-on', 'supports', 'blocks', 'references', 'team-member-of')),
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  source_event_sequence INTEGER NOT NULL REFERENCES session_relation_events(sequence),
  CHECK (from_session_id <> to_session_id)
) STRICT;
CREATE UNIQUE INDEX one_fork_parent_idx ON session_relations_current(from_session_id) WHERE relation_kind = 'forked-from';
`

export const FOUNDATION_MIGRATIONS: readonly Migration[] = [
  {
    version: 1,
    name: 'foundation-schema',
    sql: FOUNDATION_SCHEMA
  },
  {
    version: 2,
    name: 'authoritative-model-completion',
    sql: AUTHORITATIVE_MODEL_COMPLETION
  },
  {
    version: 3,
    name: 'active-task-name-constraint',
    sql: `
      CREATE UNIQUE INDEX active_task_name_idx
      ON tasks(workspace_id, title)
      WHERE archived_at IS NULL;
    `
  },
  {
    version: 4,
    name: 'scene-window-mount-association',
    sql: `
      ALTER TABLE session_mounts ADD COLUMN scene_window_id TEXT REFERENCES scene_windows(id);
      CREATE INDEX session_mounts_window_idx ON session_mounts(scene_window_id);
    `
  },
  {
    version: 5,
    name: 'terminal-command-boundaries',
    sql: `
      CREATE TABLE terminal_commands (
        command_id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        started_sequence INTEGER NOT NULL,
        executed_sequence INTEGER,
        ended_sequence INTEGER,
        command_text TEXT,
        cwd TEXT,
        exit_code INTEGER,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;
      CREATE INDEX terminal_commands_session_sequence_idx
      ON terminal_commands(session_id, started_sequence);
    `
  },
  {
    version: 6,
    name: 'preset-reconcile-idempotency',
    sql: `
      CREATE TABLE preset_reconcile_commands (
        command_id TEXT PRIMARY KEY,
        manifest_fingerprint TEXT NOT NULL,
        result_json TEXT NOT NULL,
        completed_at INTEGER NOT NULL
      ) STRICT;
    `
  },
  {
    version: 7,
    name: 'legacy-bridge-authority-state',
    sql: `
      CREATE TABLE legacy_source_cursors (
        source_id TEXT PRIMARY KEY,
        source_fingerprint TEXT NOT NULL,
        metadata_offset INTEGER NOT NULL DEFAULT 0,
        checkpoint_fingerprint TEXT,
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE migration_authority (
        key TEXT PRIMARY KEY,
        value_json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE migration_telemetry (
        metric TEXT PRIMARY KEY,
        value REAL NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE legacy_projection_diffs (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        source_id TEXT NOT NULL,
        equal INTEGER NOT NULL CHECK (equal IN (0, 1)),
        diff_json TEXT NOT NULL,
        legacy_fingerprint TEXT NOT NULL,
        sqlite_fingerprint TEXT NOT NULL,
        created_at INTEGER NOT NULL
      ) STRICT;
    `
  },
  {
    version: 8,
    name: 'prd-05-hierarchy-state',
    sql: `
      ALTER TABLE scenes ADD COLUMN title_pinned INTEGER NOT NULL DEFAULT 0
        CHECK (title_pinned IN (0, 1));
      ALTER TABLE scenes ADD COLUMN sort_key TEXT NOT NULL DEFAULT '';
      ALTER TABLE scenes ADD COLUMN layout_revision INTEGER NOT NULL DEFAULT 1;

      CREATE UNIQUE INDEX active_pinned_scene_title_idx
      ON scenes(task_id, name)
      WHERE archived_at IS NULL AND title_pinned = 1;

      CREATE TABLE workspace_path_state (
        workspace_id TEXT PRIMARY KEY REFERENCES workspaces(id),
        status TEXT NOT NULL CHECK (status IN ('valid', 'invalid')),
        reason TEXT NOT NULL CHECK (reason IN ('', 'missing', 'not-directory', 'no-access', 'unknown')),
        checked_at INTEGER NOT NULL,
        validation_generation INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE app_windows (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL CHECK (kind IN ('main', 'detached-terminal')),
        state TEXT NOT NULL CHECK (state IN ('visible', 'hidden', 'closed')),
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE window_navigation (
        window_id TEXT PRIMARY KEY REFERENCES app_windows(id),
        active_workspace_id TEXT REFERENCES workspaces(id),
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE TABLE window_workspace_focus (
        window_id TEXT NOT NULL REFERENCES app_windows(id),
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        active_task_id TEXT REFERENCES tasks(id),
        updated_at INTEGER NOT NULL,
        PRIMARY KEY(window_id, workspace_id)
      ) STRICT;

      CREATE TABLE window_task_focus (
        window_id TEXT NOT NULL REFERENCES app_windows(id),
        task_id TEXT NOT NULL REFERENCES tasks(id),
        active_scene_id TEXT REFERENCES scenes(id),
        updated_at INTEGER NOT NULL,
        PRIMARY KEY(window_id, task_id)
      ) STRICT;

      CREATE TABLE window_scene_focus (
        window_id TEXT NOT NULL REFERENCES app_windows(id),
        scene_id TEXT NOT NULL REFERENCES scenes(id),
        active_session_id TEXT REFERENCES sessions(id),
        updated_at INTEGER NOT NULL,
        PRIMARY KEY(window_id, scene_id)
      ) STRICT;

      CREATE TABLE window_task_placements (
        window_id TEXT NOT NULL REFERENCES app_windows(id),
        task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
        ordinal INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        PRIMARY KEY(window_id, task_id)
      ) STRICT;

      CREATE TABLE bootstrap_state (
        key TEXT PRIMARY KEY,
        value_json TEXT NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;
    `
  },
  {
    version: 9,
    name: 'task-window-migrations',
    sql: `
      CREATE TABLE task_window_migrations (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES tasks(id),
        source_window_id TEXT NOT NULL REFERENCES app_windows(id),
        target_window_id TEXT NOT NULL REFERENCES app_windows(id),
        state TEXT NOT NULL CHECK (state IN ('preparing', 'committed', 'failed')),
        failure_reason TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;
      CREATE UNIQUE INDEX active_task_window_migration_idx
      ON task_window_migrations(task_id) WHERE state = 'preparing';
    `
  },
  {
    version: 10,
    name: 'session-working-directory',
    sql: `
      ALTER TABLE sessions ADD COLUMN cwd TEXT NOT NULL DEFAULT '';
      UPDATE sessions
      SET cwd = COALESCE((
        SELECT execution_contexts.cwd
        FROM execution_contexts
        WHERE execution_contexts.id = sessions.execution_context_id
      ), '')
      WHERE cwd = '';

      CREATE TRIGGER sessions_default_cwd_after_insert
      AFTER INSERT ON sessions
      WHEN NEW.cwd = ''
      BEGIN
        UPDATE sessions
        SET cwd = COALESCE((
          SELECT execution_contexts.cwd
          FROM execution_contexts
          WHERE execution_contexts.id = NEW.execution_context_id
        ), '')
        WHERE id = NEW.id;
      END;
    `
  },
  {
    version: 11,
    name: 'prd-06-session-fork-intents',
    sql: `
      CREATE TABLE session_fork_intents (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        source_session_id TEXT NOT NULL REFERENCES sessions(id),
        source_provider TEXT NOT NULL CHECK (source_provider IN ('claude-code')),
        source_provider_session_id TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('pending', 'starting', 'succeeded', 'failed')),
        error_message TEXT,
        created_at INTEGER NOT NULL,
        started_at INTEGER,
        completed_at INTEGER,
        CHECK (source_session_id <> session_id)
      ) STRICT;
      CREATE INDEX session_fork_intents_source_idx
      ON session_fork_intents(source_session_id, created_at);
    `
  },
  {
    version: 12,
    name: 'session-kind-display-titles',
    sql: `
      UPDATE sessions SET title = 'Shell' WHERE kind = 'shell';
      UPDATE sessions SET title = 'Claude' WHERE kind = 'claude-code';
      UPDATE sessions SET title = 'Codex' WHERE kind = 'codex';
    `
  },
  {
    version: 13,
    name: 'flat-workspace-navigation-order',
    sql: `
      ALTER TABLE workspaces ADD COLUMN is_default INTEGER NOT NULL DEFAULT 0
        CHECK (is_default IN (0, 1));
      ALTER TABLE workspaces ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0
        CHECK (is_pinned IN (0, 1));
      ALTER TABLE workspaces ADD COLUMN pin_sort_key TEXT NOT NULL DEFAULT '';
      ALTER TABLE workspaces ADD COLUMN last_opened_at INTEGER NOT NULL DEFAULT 0;
      UPDATE workspaces SET last_opened_at = updated_at WHERE last_opened_at = 0;
      CREATE UNIQUE INDEX one_active_default_workspace_idx
      ON workspaces(is_default) WHERE is_default = 1 AND archived_at IS NULL;

      ALTER TABLE tasks ADD COLUMN is_pinned INTEGER NOT NULL DEFAULT 0
        CHECK (is_pinned IN (0, 1));
      ALTER TABLE tasks ADD COLUMN pin_sort_key TEXT NOT NULL DEFAULT '';
      ALTER TABLE tasks ADD COLUMN last_opened_at INTEGER NOT NULL DEFAULT 0;
      UPDATE tasks SET last_opened_at = updated_at WHERE last_opened_at = 0;
    `
  },
  {
    version: 14,
    name: 'session-canvas-graph-state',
    sql: `
      CREATE TABLE session_canvas_memberships (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id),
        scene_id TEXT NOT NULL REFERENCES scenes(id),
        sibling_created_seq INTEGER NOT NULL,
        last_user_interaction_seq INTEGER NOT NULL DEFAULT 0,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL
      ) STRICT;

      CREATE INDEX session_canvas_memberships_scene_idx
      ON session_canvas_memberships(
        scene_id,
        last_user_interaction_seq DESC,
        sibling_created_seq ASC
      );

      CREATE TABLE runtime_sequences (
        name TEXT PRIMARY KEY,
        value INTEGER NOT NULL
      ) STRICT;

      WITH ranked_mounts AS (
        SELECT
          session_id,
          scene_id,
          created_at,
          id,
          ROW_NUMBER() OVER (
            PARTITION BY session_id
            ORDER BY created_at ASC, id ASC
          ) AS session_mount_rank
        FROM session_mounts
      ),
      first_mounts AS (
        SELECT session_id, scene_id, created_at
        FROM ranked_mounts
        WHERE session_mount_rank = 1
      ),
      ordered_memberships AS (
        SELECT
          first_mounts.session_id,
          first_mounts.scene_id,
          first_mounts.created_at,
          ROW_NUMBER() OVER (
            ORDER BY first_mounts.created_at ASC, first_mounts.session_id ASC
          ) AS sibling_created_seq
        FROM first_mounts
      )
      INSERT INTO session_canvas_memberships (
        session_id,
        scene_id,
        sibling_created_seq,
        last_user_interaction_seq,
        created_at,
        updated_at
      )
      SELECT
        ordered_memberships.session_id,
        ordered_memberships.scene_id,
        ordered_memberships.sibling_created_seq,
        0,
        ordered_memberships.created_at,
        sessions.updated_at
      FROM ordered_memberships
      JOIN sessions ON sessions.id = ordered_memberships.session_id;

      INSERT INTO runtime_sequences(name, value)
      SELECT 'session-sibling-created', COALESCE(MAX(sibling_created_seq), 0)
      FROM session_canvas_memberships;
      INSERT INTO runtime_sequences(name, value)
      VALUES ('session-user-interaction', 0);

      CREATE TRIGGER session_mount_canvas_membership_after_insert
      AFTER INSERT ON session_mounts
      WHEN NOT EXISTS (
        SELECT 1 FROM session_canvas_memberships
        WHERE session_id = NEW.session_id
      )
      BEGIN
        UPDATE runtime_sequences
        SET value = value + 1
        WHERE name = 'session-sibling-created';
        INSERT INTO session_canvas_memberships (
          session_id,
          scene_id,
          sibling_created_seq,
          last_user_interaction_seq,
          created_at,
          updated_at
        )
        SELECT
          NEW.session_id,
          NEW.scene_id,
          value,
          0,
          NEW.created_at,
          NEW.created_at
        FROM runtime_sequences
        WHERE name = 'session-sibling-created';
      END;

      ALTER TABLE provider_bindings ADD COLUMN restore_state TEXT NOT NULL DEFAULT 'none'
        CHECK (restore_state IN ('none', 'restoring', 'failed'));
      ALTER TABLE provider_bindings ADD COLUMN restore_error TEXT;
      ALTER TABLE provider_bindings ADD COLUMN user_exited_at INTEGER;

      DROP INDEX one_fork_parent_idx;
      CREATE UNIQUE INDEX one_structural_parent_idx
      ON session_relations_current(from_session_id)
      WHERE relation_kind IN ('forked-from', 'derived-from');
    `
  },
  {
    version: 15,
    name: 'session-fork-workflow-state',
    sql: `
      ALTER TABLE session_fork_intents ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
      ALTER TABLE session_fork_intents ADD COLUMN worktree_mode TEXT NOT NULL DEFAULT 'current'
        CHECK (worktree_mode IN ('current', 'new'));
      ALTER TABLE session_fork_intents ADD COLUMN worktree_id TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN target_execution_context_id TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN worktree_path TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN branch_name TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0;
      ALTER TABLE session_fork_intents ADD COLUMN updated_at INTEGER NOT NULL DEFAULT 0;
      UPDATE session_fork_intents SET updated_at = created_at WHERE updated_at = 0;
    `
  },
  {
    version: 16,
    name: 'session-graph-live-summaries',
    sql: `
      CREATE TABLE session_graph_summaries (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        latest_lines_json TEXT NOT NULL DEFAULT '[]',
        updated_at INTEGER NOT NULL
      );
    `
  },
  {
    version: 17,
    name: 'session-work-status',
    sql: `
      ALTER TABLE sessions ADD COLUMN work_status TEXT NOT NULL DEFAULT 'idle'
        CHECK (work_status IN (
          'starting', 'idle', 'running', 'needs-input', 'error',
          'interrupted', 'exited'
        ));
      UPDATE sessions
      SET work_status = CASE status
        WHEN 'created' THEN 'idle'
        WHEN 'starting' THEN 'starting'
        WHEN 'running' THEN 'idle'
        WHEN 'waiting' THEN 'needs-input'
        WHEN 'interrupted' THEN 'interrupted'
        WHEN 'exited' THEN 'exited'
        WHEN 'archived' THEN 'exited'
      END;
    `
  },
  {
    version: 18,
    name: 'shell-command-blocks',
    acceptedChecksums: [
      'b34eff91ec349bd3472ab71c46b0bd840ab08f0cafc06957a795187d9d64b0bd'
    ],
    sql: `
      CREATE TABLE shell_history_blocks (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
        command_text TEXT NOT NULL,
        cwd TEXT NOT NULL,
        output TEXT NOT NULL,
        exit_code INTEGER NOT NULL,
        started_at INTEGER NOT NULL,
        completed_at INTEGER NOT NULL
      ) STRICT;
      CREATE INDEX shell_history_blocks_session_completed_idx
      ON shell_history_blocks(session_id, completed_at DESC, id DESC);
    `
  },
  {
    version: 19,
    name: 'fork-inherited-conversation-readiness',
    sql: `
      UPDATE provider_bindings
      SET resume_state = 'available',
          metadata_json = json_set(
            json_remove(metadata_json, '$.provisional'),
            '$.inheritedConversation', json('true'),
            '$.canFork', json('true')
          ),
          validated_at = COALESCE(validated_at, updated_at),
          invalidated_at = NULL
      WHERE provider = 'claude-code'
        AND json_valid(metadata_json) = 1
        AND json_extract(metadata_json, '$.provisional') = 1
        AND EXISTS (
          SELECT 1
          FROM session_fork_intents AS fork
          WHERE fork.session_id = provider_bindings.session_id
            AND fork.state IN ('pending', 'starting')
            AND fork.source_provider_session_id <> provider_bindings.provider_session_id
        );

      UPDATE session_fork_intents
      SET state = 'succeeded',
          error_message = NULL,
          completed_at = COALESCE(completed_at, updated_at, created_at)
      WHERE state IN ('pending', 'starting')
        AND EXISTS (
          SELECT 1
          FROM provider_bindings AS binding
          WHERE binding.session_id = session_fork_intents.session_id
            AND binding.provider = 'claude-code'
            AND binding.resume_state = 'available'
            AND json_valid(binding.metadata_json) = 1
            AND json_extract(binding.metadata_json, '$.inheritedConversation') = 1
            AND json_extract(binding.metadata_json, '$.canFork') = 1
        );
    `
  },
  {
    version: 20,
    name: 'deferred-active-session-order',
    sql: `
      ALTER TABLE session_canvas_memberships
      ADD COLUMN pending_user_interaction_seq INTEGER NOT NULL DEFAULT 0;
    `
  },
  {
    version: 21,
    name: 'provider-session-multi-card-associations',
    sql: `
      ALTER TABLE provider_bindings RENAME TO provider_bindings_single_card;

      CREATE TABLE provider_bindings (
        id TEXT PRIMARY KEY,
        session_id TEXT NOT NULL REFERENCES sessions(id),
        provider TEXT NOT NULL CHECK (provider IN ('claude-code', 'codex', 'generic')),
        provider_session_id TEXT NOT NULL,
        resume_state TEXT NOT NULL CHECK (
          resume_state IN ('unknown', 'available', 'resuming', 'resumed', 'failed', 'expired')
        ),
        metadata_json TEXT NOT NULL DEFAULT '{}',
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        validated_at INTEGER,
        invalidated_at INTEGER,
        restore_state TEXT NOT NULL DEFAULT 'none'
          CHECK (restore_state IN ('none', 'restoring', 'failed')),
        restore_error TEXT,
        user_exited_at INTEGER,
        UNIQUE(session_id, provider, provider_session_id)
      ) STRICT;

      INSERT INTO provider_bindings (
        id, session_id, provider, provider_session_id, resume_state,
        metadata_json, created_at, updated_at, validated_at, invalidated_at,
        restore_state, restore_error, user_exited_at
      )
      SELECT
        id, session_id, provider, provider_session_id, resume_state,
        metadata_json, created_at, updated_at, validated_at, invalidated_at,
        restore_state, restore_error, user_exited_at
      FROM provider_bindings_single_card;

      DROP TABLE provider_bindings_single_card;
    `
  },
  {
    version: 22,
    name: 'session-provider-title-ownership',
    sql: `
      ALTER TABLE sessions ADD COLUMN title_source TEXT NOT NULL DEFAULT 'default'
        CHECK (title_source IN ('default', 'auto', 'manual'));
      ALTER TABLE sessions ADD COLUMN provider_title TEXT;

      UPDATE sessions
      SET title_source = 'manual'
      WHERE title <> CASE kind
        WHEN 'claude-code' THEN 'Claude'
        WHEN 'codex' THEN 'Codex'
        ELSE 'Shell'
      END;
    `
  },
  {
    version: 23,
    name: 'manual-workspace-board-status',
    sql: `
      UPDATE tasks
      SET status = 'planned'
      WHERE status = 'active';
    `
  },
  {
    version: 24,
    name: 'session-environment-bindings',
    sql: `
      CREATE TABLE session_environment_bindings (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        local_execution_context_id TEXT NOT NULL REFERENCES execution_contexts(id),
        managed_worktree_id TEXT UNIQUE REFERENCES worktrees(id),
        active_target TEXT NOT NULL CHECK (active_target IN ('local', 'worktree')),
        state TEXT NOT NULL CHECK (
          state IN ('ready', 'missing', 'recovering', 'handoff', 'failed')
        ),
        error_message TEXT,
        updated_at INTEGER NOT NULL,
        CHECK (active_target <> 'worktree' OR managed_worktree_id IS NOT NULL),
        CHECK (active_target <> 'local' OR state <> 'missing')
      ) STRICT;

      WITH worktree_sessions AS (
        SELECT
          sessions.id AS session_id,
          sessions.created_at,
          worktrees.id AS worktree_id,
          CASE WHEN EXISTS (
            SELECT 1
            FROM session_fork_intents AS fork
            WHERE fork.session_id = sessions.id
              AND fork.worktree_mode = 'new'
              AND fork.worktree_id = worktrees.id
          ) THEN 1 ELSE 0 END AS explicit_owner,
          COUNT(*) OVER (PARTITION BY worktrees.id) AS context_session_count,
          ROW_NUMBER() OVER (
            PARTITION BY worktrees.id
            ORDER BY
              CASE WHEN EXISTS (
                SELECT 1
                FROM session_fork_intents AS fork
                WHERE fork.session_id = sessions.id
                  AND fork.worktree_mode = 'new'
                  AND fork.worktree_id = worktrees.id
              ) THEN 0 ELSE 1 END,
              sessions.created_at,
              sessions.id
          ) AS ownership_rank
        FROM sessions
        JOIN execution_contexts
          ON execution_contexts.id = sessions.execution_context_id
         AND execution_contexts.kind = 'git-worktree'
        JOIN worktrees
          ON worktrees.execution_context_id = sessions.execution_context_id
      ),
      owned_worktrees AS (
        SELECT session_id, worktree_id
        FROM worktree_sessions
        WHERE ownership_rank = 1
          AND (explicit_owner = 1 OR context_session_count = 1)
      )
      INSERT INTO session_environment_bindings (
        session_id, local_execution_context_id, managed_worktree_id,
        active_target, state, error_message, updated_at
      )
      SELECT
        sessions.id,
        CASE WHEN owned_worktrees.worktree_id IS NULL
          THEN sessions.execution_context_id
          ELSE COALESCE(
            (
              SELECT source.execution_context_id
              FROM session_fork_intents AS fork
              JOIN sessions AS source ON source.id = fork.source_session_id
              WHERE fork.session_id = sessions.id
              LIMIT 1
            ),
            tasks.execution_context_id,
            sessions.execution_context_id
          )
        END,
        owned_worktrees.worktree_id,
        CASE WHEN owned_worktrees.worktree_id IS NULL THEN 'local' ELSE 'worktree' END,
        CASE
          WHEN owned_worktrees.worktree_id IS NULL THEN 'ready'
          WHEN worktrees.state IN ('ready', 'dirty', 'retained') THEN 'ready'
          WHEN worktrees.state = 'creating' THEN 'recovering'
          WHEN worktrees.state IN ('removed', 'removing') THEN 'missing'
          ELSE 'failed'
        END,
        CASE
          WHEN worktrees.state IN ('removed', 'removing') THEN 'managed Worktree is unavailable'
          WHEN worktrees.state = 'failed' THEN 'managed Worktree failed before migration'
          ELSE NULL
        END,
        sessions.updated_at
      FROM sessions
      JOIN tasks ON tasks.id = sessions.task_id
      LEFT JOIN owned_worktrees ON owned_worktrees.session_id = sessions.id
      LEFT JOIN worktrees ON worktrees.id = owned_worktrees.worktree_id;

      CREATE INDEX session_environment_local_context_idx
      ON session_environment_bindings(local_execution_context_id);

      CREATE TRIGGER session_environment_binding_after_session_insert
      AFTER INSERT ON sessions
      BEGIN
        INSERT INTO session_environment_bindings (
          session_id, local_execution_context_id, managed_worktree_id,
          active_target, state, error_message, updated_at
        ) VALUES (
          NEW.id, NEW.execution_context_id, NULL,
          'local', 'ready', NULL, NEW.updated_at
        );
      END;
    `
  },
  {
    version: 25,
    name: 'execution-context-git-state',
    sql: `
      CREATE TABLE execution_context_git_states (
        execution_context_id TEXT PRIMARY KEY
          REFERENCES execution_contexts(id) ON DELETE CASCADE,
        repository_root TEXT,
        state TEXT NOT NULL CHECK (state IN ('ready', 'unavailable')),
        branch TEXT,
        detached_head TEXT,
        dirty INTEGER NOT NULL CHECK (dirty IN (0, 1)),
        error_message TEXT,
        updated_at INTEGER NOT NULL,
        CHECK (
          state = 'unavailable' OR (
            repository_root IS NOT NULL AND
            ((branch IS NOT NULL) <> (detached_head IS NOT NULL))
          )
        ),
        CHECK (
          state = 'ready' OR (
            branch IS NULL AND detached_head IS NULL AND dirty = 0
          )
        )
      ) STRICT;

      INSERT INTO execution_context_git_states (
        execution_context_id, repository_root, state, branch, detached_head,
        dirty, error_message, updated_at
      )
      SELECT
        execution_context_id,
        repository_root,
        CASE
          WHEN state IN ('ready', 'dirty', 'retained')
            AND (branch_name <> '(detached)' OR base_revision IS NOT NULL)
          THEN 'ready'
          ELSE 'unavailable'
        END,
        CASE
          WHEN state IN ('ready', 'dirty', 'retained') AND branch_name <> '(detached)'
          THEN branch_name
          ELSE NULL
        END,
        CASE
          WHEN state IN ('ready', 'dirty', 'retained') AND branch_name = '(detached)'
          THEN base_revision
          ELSE NULL
        END,
        CASE
          WHEN state IN ('dirty', 'retained')
            AND (branch_name <> '(detached)' OR base_revision IS NOT NULL)
          THEN 1
          ELSE 0
        END,
        CASE
          WHEN state NOT IN ('ready', 'dirty', 'retained')
          THEN 'registered Worktree is unavailable'
          WHEN branch_name = '(detached)' AND base_revision IS NULL
          THEN 'detached Worktree HEAD is unavailable'
          ELSE NULL
        END,
        updated_at
      FROM worktrees;
    `
  },
  {
    version: 26,
    name: 'session-environment-transitions',
    sql: `
      CREATE TABLE session_environment_transitions (
        session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
        operation_id TEXT NOT NULL UNIQUE,
        kind TEXT NOT NULL CHECK (kind IN ('restore', 'locate', 'handoff')),
        previous_active_target TEXT NOT NULL CHECK (
          previous_active_target IN ('local', 'worktree')
        ),
        previous_state TEXT NOT NULL CHECK (
          previous_state IN ('ready', 'missing', 'failed')
        ),
        target TEXT NOT NULL CHECK (target IN ('local', 'worktree')),
        candidate_path TEXT,
        phase TEXT NOT NULL CHECK (
          phase IN ('accepted', 'external-ready', 'failed')
        ),
        error_message TEXT,
        created_at INTEGER NOT NULL,
        updated_at INTEGER NOT NULL,
        CHECK (kind = 'locate' OR candidate_path IS NULL)
      ) STRICT;
    `
  },
  {
    version: 27,
    name: 'durable-fork-operations',
    sql: `
      ALTER TABLE session_fork_intents ADD COLUMN operation_id TEXT NOT NULL DEFAULT '';
      ALTER TABLE session_fork_intents ADD COLUMN submission_key TEXT NOT NULL DEFAULT '';
      ALTER TABLE session_fork_intents ADD COLUMN stage TEXT NOT NULL DEFAULT 'queued'
        CHECK (stage IN (
          'queued', 'creating-worktree', 'applying-setup', 'binding-session',
          'restoring-provider', 'starting-window', 'succeeded', 'failed'
        ));
      ALTER TABLE session_fork_intents ADD COLUMN completed_steps INTEGER NOT NULL DEFAULT 0
        CHECK (completed_steps >= 0);
      ALTER TABLE session_fork_intents ADD COLUMN total_steps INTEGER NOT NULL DEFAULT 5
        CHECK (total_steps > 0 AND completed_steps <= total_steps);
      ALTER TABLE session_fork_intents ADD COLUMN attempt INTEGER NOT NULL DEFAULT 0
        CHECK (attempt >= 0);
      ALTER TABLE session_fork_intents ADD COLUMN lease_owner TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN lease_token TEXT;
      ALTER TABLE session_fork_intents ADD COLUMN lease_expires_at INTEGER;
      ALTER TABLE session_fork_intents ADD COLUMN lease_fence INTEGER NOT NULL DEFAULT 0
        CHECK (lease_fence >= 0);
      ALTER TABLE session_fork_intents ADD COLUMN last_heartbeat_at INTEGER;

      UPDATE session_fork_intents
      SET operation_id = 'legacy-operation:' || session_id,
          submission_key = 'legacy-submission:' || session_id,
          stage = CASE state
            WHEN 'succeeded' THEN 'succeeded'
            WHEN 'failed' THEN 'failed'
            ELSE 'queued'
          END,
          total_steps = CASE worktree_mode WHEN 'current' THEN 2 ELSE 5 END,
          completed_steps = CASE state
            WHEN 'succeeded' THEN CASE worktree_mode WHEN 'current' THEN 2 ELSE 5 END
            ELSE 0
          END,
          attempt = attempt_count;

      CREATE UNIQUE INDEX session_fork_intents_operation_idx
      ON session_fork_intents(operation_id);
      CREATE UNIQUE INDEX session_fork_intents_submission_idx
      ON session_fork_intents(submission_key);
      CREATE INDEX session_fork_intents_lease_idx
      ON session_fork_intents(stage, lease_expires_at, created_at);
    `
  },
  {
    version: 28,
    name: 'session-structural-relation-lookup',
    sql: `
      CREATE INDEX session_relations_structural_lookup_idx
      ON session_relations_current(from_session_id, relation_kind);
    `
  }
]
