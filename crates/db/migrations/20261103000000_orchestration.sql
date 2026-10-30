-- KanbAIn Phase 5: the orchestrator (D22).

-- A ticket's coding run: which approved plan it carries out, the workspace
-- holding the ticket branch (the planner's), and where it stands.
CREATE TABLE ticket_orchestrations (
    issue_id     BLOB PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    plan_id      BLOB NOT NULL REFERENCES plans(id) ON DELETE CASCADE,
    workspace_id BLOB REFERENCES workspaces(id) ON DELETE SET NULL,
    status       TEXT NOT NULL DEFAULT 'running'
                 CHECK (status IN ('running', 'stopped', 'blocked', 'done')),
    error        TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

-- One row per plan subtask: its sub-ticket, its own workspace and branch,
-- the current coder run and the merge into the ticket branch.
CREATE TABLE orchestration_subtasks (
    id                   BLOB PRIMARY KEY,
    issue_id             BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    -- The subtask's id in plan.json
    plan_subtask_id      TEXT NOT NULL,
    position             INTEGER NOT NULL,
    sub_issue_id         BLOB REFERENCES issues(id) ON DELETE SET NULL,
    workspace_id         BLOB REFERENCES workspaces(id) ON DELETE SET NULL,
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    status               TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'running', 'merged', 'failed', 'stopped')),
    -- Re-runs after a merge conflict
    fix_round            INTEGER NOT NULL DEFAULT 0,
    -- JSON: the last conflict (files per repo and the earlier attempt's diff)
    conflict             TEXT,
    -- JSON: merge commit per repo name
    merge_commits        TEXT,
    error                TEXT,
    started_at           TEXT,
    finished_at          TEXT,
    updated_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, plan_subtask_id)
);
CREATE INDEX idx_orchestration_subtasks_run ON orchestration_subtasks(execution_process_id);
CREATE INDEX idx_orchestration_subtasks_workspace ON orchestration_subtasks(workspace_id);
