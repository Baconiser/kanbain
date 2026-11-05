-- Things an agent found missing in its sandbox (a browser, a test runner, a
-- CLI) and asked for with the `request_setup` tool. The user hands them to
-- the setup agent, which proposes setup commands for them, or dismisses
-- them.
CREATE TABLE setup_requests (
    id                   BLOB PRIMARY KEY,
    project_id           BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_id             BLOB REFERENCES issues(id) ON DELETE SET NULL,
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    -- Agent role of the run that asked (`tester`, `coder`, ...); NULL when
    -- the run's agent is unknown
    role                 TEXT,
    -- Agent definition name of the run that asked
    agent                TEXT,
    need                 TEXT NOT NULL,
    reason               TEXT NOT NULL,
    -- Commands the agent thinks would do it
    suggestion           TEXT,
    status               TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'resolved', 'dismissed')),
    created_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    resolved_at          TEXT
);

CREATE INDEX idx_setup_requests_project ON setup_requests(project_id, status);

-- Role-only setup commands the scan proposes, as {"tester": "..."}, and the
-- setup requests the scan answered (approval resolves them).
ALTER TABLE project_setup_scans ADD COLUMN role_setup TEXT NOT NULL DEFAULT '{}';
ALTER TABLE project_setup_scans ADD COLUMN request_ids TEXT NOT NULL DEFAULT '[]';
