-- Changelog entries (Phase 7, D25): one per coder, tester or fix run the
-- scribe summarized, plus one per merge into the target branch. Files and
-- commits come from git, not from the scribe.
CREATE TABLE changelog_entries (
    id           BLOB PRIMARY KEY,
    issue_id     BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    run_id       BLOB REFERENCES execution_processes(id) ON DELETE SET NULL,
    job_id       BLOB REFERENCES agent_jobs(id) ON DELETE SET NULL,
    kind         TEXT NOT NULL CHECK (kind IN ('coder', 'tester', 'fix', 'merge')),
    title        TEXT NOT NULL,
    -- The scribe's Markdown, or a plain fallback when it failed
    summary      TEXT NOT NULL,
    -- JSON arrays: ["repo/path"], [{"repo","sha"}], [issue_found id]
    files        TEXT NOT NULL DEFAULT '[]',
    commit_refs  TEXT NOT NULL DEFAULT '[]',
    issues_fixed TEXT NOT NULL DEFAULT '[]',
    issues_found TEXT NOT NULL DEFAULT '[]',
    -- Why the scribe's summary is missing, when it is
    scribe_error TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE INDEX idx_changelog_entries_issue ON changelog_entries (issue_id, created_at);
-- A retried end-of-run handler must not write the same entry twice.
CREATE UNIQUE INDEX idx_changelog_entries_run ON changelog_entries (run_id, kind)
    WHERE run_id IS NOT NULL;
