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
  }
]
