-- The setup agent's latest proposal for a project's sandbox setup: a setup
-- script, test and run commands and environment notes. The user reviews it
-- in the project settings; approving copies it into the project settings,
-- where runs pick it up.
CREATE TABLE project_setup_scans (
    project_id        BLOB PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
    status            TEXT NOT NULL DEFAULT 'scanning'
                      CHECK (status IN ('scanning', 'ready', 'failed', 'approved',
                                        'discarded')),
    job_id            BLOB REFERENCES agent_jobs(id) ON DELETE SET NULL,
    -- `setup.sh`: extra commands run after KanbAIn's own lockfile installs
    setup_script      TEXT,
    -- JSON arrays of shell commands
    test_commands     TEXT NOT NULL DEFAULT '[]',
    run_commands      TEXT NOT NULL DEFAULT '[]',
    -- `environment.md`: notes every agent of the project gets
    environment_md    TEXT,
    -- The agent found KanbAIn's lockfile installs wrong for this project
    skip_auto_install INTEGER NOT NULL DEFAULT 0,
    -- The agent's final message
    summary           TEXT,
    -- Fingerprint of the repos' stack files when the scan started
    stack_hash        TEXT NOT NULL DEFAULT '',
    error             TEXT,
    started_at        TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    finished_at       TEXT,
    decided_at        TEXT
);
