-- KanbAIn Phase 2: index of the agent library (D13).
--
-- Agent definitions, style blocks and skills live as files in the data
-- folder (agents/<name>.yaml, styles/<name>.md, skills/<name>/SKILL.md).
-- These tables mirror them for listing and lookups; a file watcher keeps
-- them in sync. A file that fails to parse is indexed with its error.

CREATE TABLE agent_definitions (
    name         TEXT PRIMARY KEY,
    -- Parsed definition as JSON; NULL when the file is invalid
    definition   TEXT,
    error        TEXT,
    content_hash TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE TABLE style_blocks (
    name         TEXT PRIMARY KEY,
    body         TEXT NOT NULL,
    content_hash TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);

CREATE TABLE skills (
    name         TEXT PRIMARY KEY,
    description  TEXT,
    file_count   INTEGER NOT NULL DEFAULT 0,
    total_bytes  INTEGER NOT NULL DEFAULT 0,
    error        TEXT,
    content_hash TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
