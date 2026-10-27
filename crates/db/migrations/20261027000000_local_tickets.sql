-- KanbAIn Phase 0b: local tickets in SQLite.
--
-- Replaces the legacy `tasks` table and the remote-linked `projects` table
-- with a local ticket model. Old Vibe Kanban data is not migrated.

-- Legacy task tables (unused since the cloud board was removed)
DROP TABLE IF EXISTS task_attachments;
DROP TABLE IF EXISTS tasks;
DROP TABLE IF EXISTS project_repos;
DROP TABLE IF EXISTS projects;

CREATE TABLE projects (
    id                  BLOB PRIMARY KEY,
    name                TEXT NOT NULL,
    -- Internal ticket ids are `<id_prefix>-<number>`, e.g. KAN-42
    id_prefix           TEXT NOT NULL DEFAULT 'KAN',
    -- Optional regex override for detecting External IDs
    external_id_pattern TEXT,
    -- Optional key that restricts External IDs, e.g. `PROJEKTNAME-`
    external_id_key     TEXT,
    issue_counter       INTEGER NOT NULL DEFAULT 0,
    sort_order          INTEGER NOT NULL DEFAULT 0,
    settings            TEXT NOT NULL DEFAULT '{}',
    created_at          TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at          TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE TABLE project_repos (
    id         BLOB PRIMARY KEY,
    project_id BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    repo_id    BLOB NOT NULL REFERENCES repos(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (project_id, repo_id)
);
CREATE INDEX idx_project_repos_project_id ON project_repos(project_id);

-- Board tables keep `project_id` as their second column so the SQLite
-- pre-update hook can route delete events to the right project stream.

CREATE TABLE project_statuses (
    id         BLOB PRIMARY KEY,
    project_id BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    color      TEXT NOT NULL,
    sort_order INTEGER NOT NULL DEFAULT 0,
    hidden     INTEGER NOT NULL DEFAULT 0,
    kind       TEXT NOT NULL DEFAULT 'custom'
               CHECK (kind IN ('backlog', 'planning', 'awaiting_approval',
                               'in_progress', 'testing', 'review', 'done',
                               'blocked', 'custom')),
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_project_statuses_project_id ON project_statuses(project_id);

CREATE TABLE issues (
    id                      BLOB PRIMARY KEY,
    project_id              BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    number                  INTEGER NOT NULL,
    status_id               BLOB NOT NULL REFERENCES project_statuses(id),
    title                   TEXT NOT NULL,
    description             TEXT,
    priority                TEXT CHECK (priority IN ('urgent', 'high', 'medium', 'low')),
    sort_order              REAL NOT NULL DEFAULT 0,
    parent_issue_id         BLOB REFERENCES issues(id) ON DELETE SET NULL,
    parent_issue_sort_order REAL,
    external_id             TEXT,
    mode                    TEXT NOT NULL DEFAULT 'ask' CHECK (mode IN ('ask', 'yolo')),
    fix_round               INTEGER NOT NULL DEFAULT 0,
    budget_usd              REAL,
    completed_at            TEXT,
    created_at              TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at              TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (project_id, number)
);
CREATE INDEX idx_issues_project_id ON issues(project_id);
CREATE INDEX idx_issues_status_id ON issues(status_id);
CREATE INDEX idx_issues_parent_issue_id ON issues(parent_issue_id);
CREATE INDEX idx_issues_external_id ON issues(project_id, external_id)
    WHERE external_id IS NOT NULL;

-- Ticket labels. Named `project_tags` because `tags` already holds the
-- reusable prompt snippets used in the chat box.
CREATE TABLE project_tags (
    id         BLOB PRIMARY KEY,
    project_id BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    color      TEXT NOT NULL,
    UNIQUE (project_id, name)
);

CREATE TABLE issue_tags (
    id         BLOB PRIMARY KEY,
    project_id BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_id   BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    tag_id     BLOB NOT NULL REFERENCES project_tags(id) ON DELETE CASCADE,
    UNIQUE (issue_id, tag_id)
);
CREATE INDEX idx_issue_tags_project_id ON issue_tags(project_id);

CREATE TABLE issue_relationships (
    id                BLOB PRIMARY KEY,
    project_id        BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_id          BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    related_issue_id  BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    relationship_type TEXT NOT NULL
                      CHECK (relationship_type IN ('blocking', 'related', 'has_duplicate')),
    created_at        TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, related_issue_id, relationship_type),
    CHECK (issue_id != related_issue_id)
);
CREATE INDEX idx_issue_relationships_project_id ON issue_relationships(project_id);

-- Comments keep `issue_id` as their second column (see the hook note above).
CREATE TABLE issue_comments (
    id         BLOB PRIMARY KEY,
    issue_id   BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    parent_id  BLOB REFERENCES issue_comments(id) ON DELETE SET NULL,
    -- `user` or `agent:<run id>`
    author     TEXT NOT NULL DEFAULT 'user',
    message    TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
CREATE INDEX idx_issue_comments_issue_id ON issue_comments(issue_id);

CREATE TABLE issue_attachments (
    id            BLOB PRIMARY KEY,
    issue_id      BLOB NOT NULL REFERENCES issues(id) ON DELETE CASCADE,
    attachment_id BLOB NOT NULL REFERENCES attachments(id) ON DELETE CASCADE,
    created_at    TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (issue_id, attachment_id)
);
CREATE INDEX idx_issue_attachments_issue_id ON issue_attachments(issue_id);
CREATE INDEX idx_issue_attachments_attachment_id ON issue_attachments(attachment_id);

-- Workspace ↔ ticket link
ALTER TABLE workspaces ADD COLUMN issue_id BLOB REFERENCES issues(id) ON DELETE SET NULL;
CREATE INDEX idx_workspaces_issue_id ON workspaces(issue_id) WHERE issue_id IS NOT NULL;
