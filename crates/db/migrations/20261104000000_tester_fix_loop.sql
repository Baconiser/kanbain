-- KanbAIn Phase 6: the tester and the fix loop (D23, D24).

-- One row per checkpoint of the approved plan, plus the final one when the
-- plan has none covering every subtask. A checkpoint is tested by a tester
-- run on its own branch; open issues go to a coder fix run on another
-- branch, then the tester runs again.
CREATE TABLE orchestration_checkpoints (
    id                   BLOB PRIMARY KEY,
    issue_id             BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    -- Index into the effective checkpoint list (the plan's, then the final)
    position             INTEGER NOT NULL,
    what                 TEXT NOT NULL,
    -- JSON: plan ids of the subtasks it waits for
    after_subtasks       TEXT NOT NULL DEFAULT '[]',
    status               TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'testing', 'fixing', 'passed',
                                           'failed', 'stopped')),
    -- Fix runs done for this checkpoint
    round                INTEGER NOT NULL DEFAULT 0,
    tester_workspace_id  BLOB REFERENCES workspaces(id) ON DELETE SET NULL,
    fixer_workspace_id   BLOB REFERENCES workspaces(id) ON DELETE SET NULL,
    -- The current tester or fixer run
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    -- JSON: the last merge conflict of its tester or fixer branch
    conflict             TEXT,
    -- JSON: files the last tester run changed outside the test paths
    rejected_files       TEXT,
    error                TEXT,
    started_at           TEXT,
    finished_at          TEXT,
    updated_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, position)
);
CREATE INDEX idx_orchestration_checkpoints_run
    ON orchestration_checkpoints(execution_process_id);
CREATE INDEX idx_orchestration_checkpoints_tester
    ON orchestration_checkpoints(tester_workspace_id);
CREATE INDEX idx_orchestration_checkpoints_fixer
    ON orchestration_checkpoints(fixer_workspace_id);

-- Problems a tester run reported in issues.json (D23).
CREATE TABLE issues_found (
    id            BLOB PRIMARY KEY,
    issue_id      BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    -- The tester run that reported it first
    run_id        BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    -- The checkpoint position it was found at
    checkpoint    INTEGER,
    file          TEXT,
    line          INTEGER,
    severity      TEXT NOT NULL DEFAULT 'medium'
                  CHECK (severity IN ('low', 'medium', 'high')),
    category      TEXT NOT NULL DEFAULT 'logic'
                  CHECK (category IN ('edge_case', 'integration', 'logic', 'error_handling',
                                      'state_timing', 'requirements')),
    description   TEXT NOT NULL,
    status        TEXT NOT NULL DEFAULT 'open'
                  CHECK (status IN ('open', 'fixed', 'wontfix')),
    -- The fix round in which a re-test no longer reported it
    fixed_round   INTEGER,
    created_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_issues_found_issue ON issues_found(issue_id, status);
CREATE INDEX idx_issues_found_file ON issues_found(file);
