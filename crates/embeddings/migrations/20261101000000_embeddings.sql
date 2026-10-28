-- KanbAIn Phase 3c: the embedding store (embedding plan §2). Derived data:
-- this file can be deleted and rebuilt at any time; backups skip it.

-- One per embedder configuration (E4). Vectors of different spaces are
-- never compared. Each space has its own `vec_space_<id>` vec0 table.
CREATE TABLE embedding_spaces (
    id           INTEGER PRIMARY KEY,
    purpose      TEXT NOT NULL CHECK (purpose IN ('text', 'code')),
    provider     TEXT NOT NULL,
    base_url     TEXT NOT NULL,
    model        TEXT NOT NULL,
    dimension    INTEGER NOT NULL,
    distance     TEXT NOT NULL DEFAULT 'cosine',
    query_prefix TEXT NOT NULL DEFAULT '',
    doc_prefix   TEXT NOT NULL DEFAULT '',
    config_hash  TEXT NOT NULL UNIQUE,
    state        TEXT NOT NULL CHECK (state IN ('building', 'active', 'retired')),
    -- The embedder answered with another dimension (E7): needs a re-index
    stale        INTEGER NOT NULL DEFAULT 0,
    last_error   TEXT,
    created_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    activated_at TEXT
);

-- Something that was chunked. Ids from the main database are text; unused
-- key parts are '' so the UNIQUE constraint holds.
CREATE TABLE sources (
    id           INTEGER PRIMARY KEY,
    kind         TEXT NOT NULL,
    project_id   TEXT NOT NULL,
    -- The ticket the source belongs to, '' for code on main
    issue_id     TEXT NOT NULL DEFAULT '',
    -- 'main', or an issue id for a ticket overlay (D45)
    scope        TEXT NOT NULL DEFAULT 'main',
    ref_id       TEXT NOT NULL DEFAULT '',
    repo_id      TEXT NOT NULL DEFAULT '',
    path         TEXT NOT NULL DEFAULT '',
    blob_sha     TEXT,
    content_hash TEXT NOT NULL,
    updated_at   TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (kind, scope, ref_id, repo_id, path)
);
CREATE INDEX idx_sources_issue ON sources(issue_id);
CREATE INDEX idx_sources_project ON sources(project_id, kind);

CREATE TABLE chunks (
    id          INTEGER PRIMARY KEY,
    source_id   INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    ordinal     INTEGER NOT NULL,
    symbol      TEXT,
    language    TEXT,
    start_line  INTEGER,
    end_line    INTEGER,
    text        TEXT NOT NULL,
    token_count INTEGER NOT NULL,
    text_hash   TEXT NOT NULL,
    UNIQUE (source_id, ordinal)
);

CREATE TABLE chunk_embeddings (
    chunk_id    INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    space_id    INTEGER NOT NULL REFERENCES embedding_spaces(id) ON DELETE CASCADE,
    model       TEXT NOT NULL,
    dimension   INTEGER NOT NULL,
    text_hash   TEXT NOT NULL,
    embedded_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    PRIMARY KEY (chunk_id, space_id)
);
CREATE INDEX idx_chunk_embeddings_space ON chunk_embeddings(space_id);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    text, symbol, content='chunks', content_rowid='id', tokenize='unicode61'
);
CREATE TRIGGER chunks_fts_insert AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text, symbol) VALUES (NEW.id, NEW.text, COALESCE(NEW.symbol, ''));
END;
CREATE TRIGGER chunks_fts_delete AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text, symbol)
    VALUES ('delete', OLD.id, OLD.text, COALESCE(OLD.symbol, ''));
END;
CREATE TRIGGER chunks_fts_update AFTER UPDATE OF text, symbol ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text, symbol)
    VALUES ('delete', OLD.id, OLD.text, COALESCE(OLD.symbol, ''));
    INSERT INTO chunks_fts(rowid, text, symbol) VALUES (NEW.id, NEW.text, COALESCE(NEW.symbol, ''));
END;

-- Sources to (re)index, debounced by `not_before`.
CREATE TABLE index_jobs (
    id          INTEGER PRIMARY KEY,
    source_kind TEXT NOT NULL,
    source_ref  TEXT NOT NULL,
    state       TEXT NOT NULL DEFAULT 'queued' CHECK (state IN ('queued', 'running', 'failed')),
    attempts    INTEGER NOT NULL DEFAULT 0,
    last_error  TEXT,
    not_before  TEXT NOT NULL DEFAULT (datetime('now', 'subsec')),
    UNIQUE (source_kind, source_ref)
);

-- Embedder calls, for token use and cost of remote embedders (E11).
CREATE TABLE embedding_calls (
    id         INTEGER PRIMARY KEY,
    space_id   INTEGER,
    purpose    TEXT NOT NULL,
    kind       TEXT NOT NULL CHECK (kind IN ('document', 'query')),
    inputs     INTEGER NOT NULL,
    tokens     INTEGER,
    latency_ms INTEGER NOT NULL,
    error      TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now', 'subsec'))
);
