-- KanbAIn Phase 3d: the line ranges a ticket commit changed in each file,
-- so search can name the symbols (E19).
ALTER TABLE ticket_code_links ADD COLUMN hunks TEXT NOT NULL DEFAULT '[]';
