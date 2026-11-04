-- An agent rebasing a workspace branch onto its target and resolving the
-- conflicts on the way. One row per attempt; the newest row per workspace
-- repo is what the UI shows.
CREATE TABLE conflict_resolutions (
    id                   BLOB PRIMARY KEY,
    workspace_id         BLOB NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    repo_id              BLOB NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    -- The resolver's run, once it started
    execution_process_id BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    target_branch        TEXT NOT NULL,
    -- Target head the branch must end up on top of
    target_oid           TEXT NOT NULL,
    -- Branch head before the rebase
    branch_oid           TEXT NOT NULL,
    -- Files `git merge-tree` expects to conflict (JSON array)
    predicted_files      TEXT NOT NULL DEFAULT '[]',
    stage                TEXT NOT NULL DEFAULT 'queued'
                         CHECK (stage IN ('queued', 'rebasing', 'syncing', 'resolved',
                                          'failed')),
    error                TEXT,
    created_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE INDEX idx_conflict_resolutions_workspace
    ON conflict_resolutions (workspace_id, repo_id, created_at);
CREATE INDEX idx_conflict_resolutions_execution
    ON conflict_resolutions (execution_process_id);
