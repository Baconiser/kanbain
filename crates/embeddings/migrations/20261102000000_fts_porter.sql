-- Phase 3d: stem keywords (numbers → number, assigned → assign), so ticket
-- wording finds code. The FTS index is rebuilt from `chunks`.
DROP TRIGGER chunks_fts_insert;
DROP TRIGGER chunks_fts_delete;
DROP TRIGGER chunks_fts_update;
DROP TABLE chunks_fts;

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    text, symbol, content='chunks', content_rowid='id', tokenize='porter unicode61'
);
INSERT INTO chunks_fts(chunks_fts) VALUES ('rebuild');

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
