-- KanbAIn Phase 3b: git-history check (D18) and ticket <-> code links (E19).

-- One row per ticket that was checked.
CREATE TABLE issue_history (
    issue_id      BLOB PRIMARY KEY REFERENCES issues(id) ON DELETE CASCADE,
    -- The id that was searched for (External ID, else internal id)
    searched_id   TEXT NOT NULL,
    -- Commits found, and those the history agent may analyze (not made by
    -- KanbAIn's own agents)
    commit_count  INTEGER NOT NULL DEFAULT 0,
    prior_count   INTEGER NOT NULL DEFAULT 0,
    -- Hash of the analyzable commits' shas, to notice new ones
    prior_hash    TEXT NOT NULL DEFAULT '',
    -- The user's answer to "Allow analysis?"
    permission    TEXT NOT NULL DEFAULT 'pending'
                  CHECK (permission IN ('pending', 'allowed', 'denied')),
    status        TEXT NOT NULL DEFAULT 'idle'
                  CHECK (status IN ('idle', 'analyzing', 'failed')),
    -- The history agent's summary (`prior_work.md`)
    prior_work    TEXT,
    -- `prior_hash` at the time of the summary
    analyzed_hash TEXT,
    job_id        BLOB REFERENCES agent_jobs(id) ON DELETE SET NULL,
    error         TEXT,
    checked_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    analyzed_at   TEXT
);

-- Commits for a ticket's id, per repo.
CREATE TABLE ticket_commits (
    issue_id      BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    repo_id       BLOB NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    sha           TEXT NOT NULL,
    author        TEXT NOT NULL,
    committed_at  TEXT NOT NULL,
    subject       TEXT NOT NULL,
    -- Branches named after the id that contain it (JSON array)
    branches      TEXT NOT NULL DEFAULT '[]',
    -- The message mentions the id
    message_match INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (issue_id, repo_id, sha)
);

-- Files (and, from Phase 3d, symbols) each ticket commit changed. `symbol`
-- is '' until code is chunked, so it can be part of the key.
CREATE TABLE ticket_code_links (
    issue_id   BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    repo_id    BLOB NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    path       TEXT NOT NULL,
    symbol     TEXT NOT NULL DEFAULT '',
    commit_sha TEXT NOT NULL,
    additions  INTEGER,
    deletions  INTEGER,
    PRIMARY KEY (issue_id, repo_id, path, symbol, commit_sha)
);
CREATE INDEX idx_ticket_code_links_path ON ticket_code_links(repo_id, path);
