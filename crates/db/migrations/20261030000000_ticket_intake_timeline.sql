-- KanbAIn Phase 3a: ticket intake, description versions, timeline.

-- Headless agent jobs (D34) that run without a repo or a workspace: intake
-- and history analysis. Workspace runs stay execution processes (D41); these
-- have no session to hang off, so they get their own rows with the same
-- usage columns.
CREATE TABLE agent_jobs (
    id                 BLOB PRIMARY KEY,
    project_id         BLOB REFERENCES projects(id) ON DELETE CASCADE,
    issue_id           BLOB REFERENCES issues(id) ON DELETE SET NULL,
    -- `intake` or `history`; later phases add their own
    kind               TEXT NOT NULL,
    agent_def_id       TEXT,
    model              TEXT,
    status             TEXT NOT NULL DEFAULT 'queued'
                       CHECK (status IN ('queued', 'running', 'completed', 'failed')),
    container_id       TEXT,
    -- The agent's final message
    output             TEXT,
    error              TEXT,
    tokens_in          INTEGER NOT NULL DEFAULT 0,
    tokens_out         INTEGER NOT NULL DEFAULT 0,
    tokens_cache_read  INTEGER NOT NULL DEFAULT 0,
    tokens_cache_write INTEGER NOT NULL DEFAULT 0,
    cost_usd           REAL NOT NULL DEFAULT 0,
    model_calls        INTEGER NOT NULL DEFAULT 0,
    budget_usd         REAL,
    started_at         TEXT,
    completed_at       TEXT,
    created_at         TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_agent_jobs_issue_id ON agent_jobs(issue_id);

-- Text extracted from attachments (PDF, HTML, DOCX, XML, plain text), kept
-- for the intake agent and for search indexing (Phase 3c).
CREATE TABLE attachment_texts (
    attachment_id BLOB PRIMARY KEY REFERENCES attachments(id) ON DELETE CASCADE,
    format        TEXT NOT NULL,
    text          TEXT NOT NULL DEFAULT '',
    error         TEXT,
    created_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

-- A dropped ticket waiting for review (D15).
CREATE TABLE ticket_intakes (
    id                   BLOB PRIMARY KEY,
    project_id           BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    raw_text             TEXT NOT NULL DEFAULT '',
    status               TEXT NOT NULL DEFAULT 'enhancing'
                         CHECK (status IN ('enhancing', 'ready', 'failed', 'accepted', 'discarded')),
    job_id               BLOB REFERENCES agent_jobs(id) ON DELETE SET NULL,
    -- EnhancedTicket JSON from the intake agent
    enhanced             TEXT,
    detected_external_id TEXT,
    error                TEXT,
    -- The ticket created or updated from it
    issue_id             BLOB REFERENCES issues(id) ON DELETE SET NULL,
    created_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at           TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE TABLE intake_attachments (
    intake_id     BLOB NOT NULL REFERENCES ticket_intakes(id) ON DELETE CASCADE,
    attachment_id BLOB NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    PRIMARY KEY (intake_id, attachment_id)
);

-- Description versions (D17). `kind` says where the stored description came
-- from: the dropped text as is, the intake agent's version unchanged, or an
-- edit by the user. The raw dropped text and the agent's version are kept
-- next to it.
CREATE TABLE issue_descriptions (
    id            BLOB PRIMARY KEY,
    issue_id      BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    version       INTEGER NOT NULL,
    kind          TEXT NOT NULL CHECK (kind IN ('original', 'enhanced', 'edited')),
    title         TEXT NOT NULL,
    description   TEXT,
    original_text TEXT,
    enhanced_text TEXT,
    intake_id     BLOB REFERENCES ticket_intakes(id) ON DELETE SET NULL,
    created_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, version)
);

-- Existing tickets start with their current description as version 1.
INSERT INTO issue_descriptions (id, issue_id, version, kind, title, description, created_at)
SELECT randomblob(16), id, 1, 'original', title, description, created_at FROM issues;

-- Ticket timeline (D19). `issue_id` is the second column so the pre-update
-- hook can route deletes. `occurred_at` is when it happened, which is before
-- the ticket existed for prior commits (Phase 3b).
CREATE TABLE issue_events (
    id          BLOB PRIMARY KEY,
    issue_id    BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    event_type  TEXT NOT NULL,
    -- `user`, `agent:<job or run id>` or `system`
    actor       TEXT NOT NULL DEFAULT 'user',
    payload     TEXT NOT NULL DEFAULT '{}',
    occurred_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    created_at  TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_issue_events_issue_id ON issue_events(issue_id, occurred_at);

INSERT INTO issue_events (id, issue_id, event_type, actor, payload, occurred_at)
SELECT randomblob(16), i.id, 'created', 'user',
       json_object('number', i.number, 'title', i.title, 'external_id', i.external_id,
                   'status', s.name),
       i.created_at
FROM issues i LEFT JOIN project_statuses s ON s.id = i.status_id;

-- Triggers record what every write path does (board drags, bulk updates,
-- the ticket panel), so no route can forget to.

CREATE TRIGGER issue_events_created AFTER INSERT ON issues
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload, occurred_at)
    VALUES (randomblob(16), NEW.id, 'created',
            json_object('number', NEW.number, 'title', NEW.title,
                        'external_id', NEW.external_id,
                        'status', (SELECT name FROM project_statuses WHERE id = NEW.status_id)),
            NEW.created_at);
END;

CREATE TRIGGER issue_events_status AFTER UPDATE OF status_id ON issues
WHEN OLD.status_id IS NOT NEW.status_id
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload)
    VALUES (randomblob(16), NEW.id, 'status_changed',
            json_object('from', (SELECT name FROM project_statuses WHERE id = OLD.status_id),
                        'to', (SELECT name FROM project_statuses WHERE id = NEW.status_id),
                        'to_kind', (SELECT kind FROM project_statuses WHERE id = NEW.status_id)));
END;

CREATE TRIGGER issue_events_title AFTER UPDATE OF title ON issues
WHEN OLD.title IS NOT NEW.title
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload)
    VALUES (randomblob(16), NEW.id, 'title_changed',
            json_object('from', OLD.title, 'to', NEW.title));
END;

CREATE TRIGGER issue_events_priority AFTER UPDATE OF priority ON issues
WHEN OLD.priority IS NOT NEW.priority
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload)
    VALUES (randomblob(16), NEW.id, 'priority_changed',
            json_object('from', OLD.priority, 'to', NEW.priority));
END;

CREATE TRIGGER issue_events_external_id AFTER UPDATE OF external_id ON issues
WHEN OLD.external_id IS NOT NEW.external_id
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload)
    VALUES (randomblob(16), NEW.id, 'external_id_changed',
            json_object('from', OLD.external_id, 'to', NEW.external_id));
END;

CREATE TRIGGER issue_events_mode AFTER UPDATE OF mode ON issues
WHEN OLD.mode IS NOT NEW.mode
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload)
    VALUES (randomblob(16), NEW.id, 'mode_changed',
            json_object('from', OLD.mode, 'to', NEW.mode));
END;

CREATE TRIGGER issue_events_description AFTER INSERT ON issue_descriptions
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload, occurred_at)
    VALUES (randomblob(16), NEW.issue_id, 'description_version',
            json_object('version', NEW.version, 'kind', NEW.kind,
                        'intake_id', CASE WHEN NEW.intake_id IS NULL THEN NULL ELSE lower(substr(hex(NEW.intake_id), 1, 8) || '-' || substr(hex(NEW.intake_id), 9, 4) || '-' || substr(hex(NEW.intake_id), 13, 4) || '-' || substr(hex(NEW.intake_id), 17, 4) || '-' || substr(hex(NEW.intake_id), 21)) END),
            NEW.created_at);
END;

CREATE TRIGGER issue_events_comment AFTER INSERT ON issue_comments
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, actor, payload, occurred_at)
    VALUES (randomblob(16), NEW.issue_id, 'comment', NEW.author,
            json_object('comment_id', lower(substr(hex(NEW.id), 1, 8) || '-' || substr(hex(NEW.id), 9, 4) || '-' || substr(hex(NEW.id), 13, 4) || '-' || substr(hex(NEW.id), 17, 4) || '-' || substr(hex(NEW.id), 21)),
                        'excerpt', substr(NEW.message, 1, 280)),
            NEW.created_at);
END;

CREATE TRIGGER issue_events_attachment AFTER INSERT ON issue_attachments
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, payload, occurred_at)
    VALUES (randomblob(16), NEW.issue_id, 'attachment_added',
            json_object('attachment_id', lower(substr(hex(NEW.attachment_id), 1, 8) || '-' || substr(hex(NEW.attachment_id), 9, 4) || '-' || substr(hex(NEW.attachment_id), 13, 4) || '-' || substr(hex(NEW.attachment_id), 17, 4) || '-' || substr(hex(NEW.attachment_id), 21)),
                        'name', (SELECT original_name FROM attachments
                                 WHERE id = NEW.attachment_id)),
            NEW.created_at);
END;

-- Runs on a ticket: the run's ticket is set when the sandbox prepares it.
CREATE TRIGGER issue_events_run_started AFTER UPDATE OF issue_id ON execution_processes
WHEN OLD.issue_id IS NULL AND NEW.issue_id IS NOT NULL
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, actor, payload)
    VALUES (randomblob(16), NEW.issue_id, 'run_started',
            'agent:' || lower(substr(hex(NEW.id), 1, 8) || '-' || substr(hex(NEW.id), 9, 4) || '-' || substr(hex(NEW.id), 13, 4) || '-' || substr(hex(NEW.id), 17, 4) || '-' || substr(hex(NEW.id), 21)),
            json_object('execution_process_id', lower(substr(hex(NEW.id), 1, 8) || '-' || substr(hex(NEW.id), 9, 4) || '-' || substr(hex(NEW.id), 13, 4) || '-' || substr(hex(NEW.id), 17, 4) || '-' || substr(hex(NEW.id), 21)),
                        'run_reason', NEW.run_reason,
                        'agent', NEW.agent_def_id,
                        'session_id', lower(substr(hex(NEW.session_id), 1, 8) || '-' || substr(hex(NEW.session_id), 9, 4) || '-' || substr(hex(NEW.session_id), 13, 4) || '-' || substr(hex(NEW.session_id), 17, 4) || '-' || substr(hex(NEW.session_id), 21))));
END;

CREATE TRIGGER issue_events_run_finished AFTER UPDATE OF status ON execution_processes
WHEN NEW.issue_id IS NOT NULL AND OLD.status = 'running' AND NEW.status != 'running'
BEGIN
    INSERT INTO issue_events (id, issue_id, event_type, actor, payload)
    VALUES (randomblob(16), NEW.issue_id, 'run_finished',
            'agent:' || lower(substr(hex(NEW.id), 1, 8) || '-' || substr(hex(NEW.id), 9, 4) || '-' || substr(hex(NEW.id), 13, 4) || '-' || substr(hex(NEW.id), 17, 4) || '-' || substr(hex(NEW.id), 21)),
            json_object('execution_process_id', lower(substr(hex(NEW.id), 1, 8) || '-' || substr(hex(NEW.id), 9, 4) || '-' || substr(hex(NEW.id), 13, 4) || '-' || substr(hex(NEW.id), 17, 4) || '-' || substr(hex(NEW.id), 21)),
                        'run_reason', NEW.run_reason,
                        'agent', NEW.agent_def_id,
                        'session_id', lower(substr(hex(NEW.session_id), 1, 8) || '-' || substr(hex(NEW.session_id), 9, 4) || '-' || substr(hex(NEW.session_id), 13, 4) || '-' || substr(hex(NEW.session_id), 17, 4) || '-' || substr(hex(NEW.session_id), 21)),
                        'status', NEW.status,
                        'tokens_in', NEW.tokens_in,
                        'tokens_out', NEW.tokens_out,
                        'tokens_cache_read', NEW.tokens_cache_read,
                        'tokens_cache_write', NEW.tokens_cache_write,
                        'cost_usd', NEW.cost_usd,
                        'duration_seconds',
                        CAST(round((julianday(COALESCE(NEW.completed_at, datetime('now')))
                                    - julianday(NEW.started_at)) * 86400) AS INTEGER)));
END;
