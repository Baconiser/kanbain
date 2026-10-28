-- KanbAIn Phase 1: sandboxed runs (D41).
--
-- A run is an execution process with ticket, container, branch, token and
-- cost tracking. Proxy and sandbox events (model calls, blocked egress, ...)
-- are stored next to the existing process logs.

ALTER TABLE execution_processes ADD COLUMN issue_id BLOB REFERENCES issues(id) ON DELETE SET NULL;
-- Sub-ticket the run works on (Phase 5 orchestrator)
ALTER TABLE execution_processes ADD COLUMN subtask_id BLOB REFERENCES issues(id) ON DELETE SET NULL;
-- Agent definition name (Phase 2)
ALTER TABLE execution_processes ADD COLUMN agent_def_id TEXT;
ALTER TABLE execution_processes ADD COLUMN container_id TEXT;
ALTER TABLE execution_processes ADD COLUMN branch TEXT;
ALTER TABLE execution_processes ADD COLUMN tokens_in INTEGER NOT NULL DEFAULT 0;
ALTER TABLE execution_processes ADD COLUMN tokens_out INTEGER NOT NULL DEFAULT 0;
ALTER TABLE execution_processes ADD COLUMN tokens_cache_read INTEGER NOT NULL DEFAULT 0;
ALTER TABLE execution_processes ADD COLUMN tokens_cache_write INTEGER NOT NULL DEFAULT 0;
ALTER TABLE execution_processes ADD COLUMN cost_usd REAL NOT NULL DEFAULT 0;
ALTER TABLE execution_processes ADD COLUMN budget_usd REAL;

CREATE INDEX idx_execution_processes_issue_id ON execution_processes(issue_id);

CREATE TABLE execution_process_events (
    id                   BLOB PRIMARY KEY,
    execution_process_id BLOB NOT NULL REFERENCES execution_processes(id) ON DELETE CASCADE,
    event_type           TEXT NOT NULL
                         CHECK (event_type IN ('model_call', 'tool_call', 'egress_block',
                                               'budget_exceeded', 'sandbox')),
    payload              TEXT NOT NULL DEFAULT '{}',
    created_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_execution_process_events_process
    ON execution_process_events(execution_process_id, created_at);
