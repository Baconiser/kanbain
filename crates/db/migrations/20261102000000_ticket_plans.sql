-- KanbAIn Phase 4: planning flow and approval gate (D20, D21).

-- Plan versions of a ticket. Every planner result and every edit is a new
-- version; only the newest one can be approved.
CREATE TABLE plans (
    id                   BLOB PRIMARY KEY,
    issue_id             BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    version              INTEGER NOT NULL,
    -- The `plan.json` document (subtasks, acceptance criteria, ...)
    plan                 TEXT NOT NULL,
    status               TEXT NOT NULL DEFAULT 'draft'
                         CHECK (status IN ('draft', 'approved', 'rejected')),
    -- Who wrote this version
    source               TEXT NOT NULL CHECK (source IN ('planner', 'edited')),
    -- The user's reason when sending it back
    feedback             TEXT,
    -- The planner run that wrote it
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    decided_at           TEXT,
    UNIQUE (issue_id, version)
);
CREATE INDEX idx_plans_issue ON plans(issue_id, version);

-- A ticket's planning state: the planner's workspace and session (answers
-- and send-backs resume it), the current run and where the flow stands.
CREATE TABLE issue_planning (
    issue_id             BLOB PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    workspace_id         BLOB REFERENCES workspaces(id) ON DELETE SET NULL,
    session_id           BLOB REFERENCES sessions(id) ON DELETE SET NULL,
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    status               TEXT NOT NULL DEFAULT 'idle'
                         CHECK (status IN ('idle', 'planning', 'waiting_for_answer',
                                           'awaiting_approval', 'approved', 'failed')),
    -- Automatic follow-ups asking the planner to fix an invalid plan.json
    repair_attempts      INTEGER NOT NULL DEFAULT 0,
    error                TEXT,
    updated_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_issue_planning_run ON issue_planning(execution_process_id);
