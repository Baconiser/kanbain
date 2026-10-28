# KanbAIn Embedding and Reranking Plan

This plan adds local semantic search over tickets, ticket information and processed code. The goal: before any work starts, KanbAIn (and its agents) can look up whether something similar was already done, and where in the code it lives, so the work can reach the right place fast.

It builds on `IMPLEMENTATION_PLAN.md` (Phases 0b–8). Where the two disagree on ticket, pipeline or sandbox behaviour, `IMPLEMENTATION_PLAN.md` wins. Written 2026-10-27; nothing here is implemented yet.

---

## 1. Settled decisions

### Storage
- **E1. Everything stays local.** No cloud vector database and no hosted index. The only thing that may leave the machine is the text sent to an embedding or reranking model, and only if the user points the model URL at a remote service (see E9).
- **E2. Vector store: SQLite with [sqlite-vec](https://github.com/asg017/sqlite-vec).** It runs inside the SQLite we already ship through sqlx, it builds on Windows, Linux and macOS, and it supports KNN with metadata filters and partition keys. libSQL/Turso was considered: it would mean replacing sqlx's SQLite driver across the whole app, which is out of proportion for this feature. Revisit only if sqlite-vec fails the E0 spike.
- **E3. A separate database file, `<data>/embeddings.sqlite`.** The index is derived data: it can be deleted and rebuilt at any time. Keeping it apart keeps `db.v2.sqlite` small, keeps the daily backups (D12) small, and lets a re-index run without locking the main database. Backups skip it.
- **E4. Every vector records its embedder.** Each vector row belongs to an *embedding space* that stores the provider, base URL, model name, **dimension**, distance metric and the query/document prefixes. A space is identified by a hash of that configuration. Changing any of it creates a new space and forces a full re-embed of that space's content; vectors from different spaces are never compared.
- **E5. Hybrid retrieval.** Every query runs both a vector KNN search and an FTS5 keyword search (bm25), merged by reciprocal rank fusion (RRF). Ticket IDs, error messages and identifiers are found reliably by keywords; paraphrases by vectors.

### Models
- **E6. Embedders are configured by URL.** Supported APIs:
  - OpenAI-compatible `POST /v1/embeddings`. This covers OpenAI, Ollama, LM Studio, llama.cpp `server --embedding`, vLLM and Hugging Face TEI.
  - TEI's native `POST /embed`, as a fallback.
  Each embedder has: `base_url`, `model`, optional API key (stored encrypted like the D33 credentials), optional `dimensions` (for models with Matryoshka truncation), `query_prefix` / `document_prefix` (for models such as nomic-embed, e5 or bge that expect `search_query:` / `search_document:`), `batch_size`, `max_input_tokens` and a request timeout.
- **E7. Dimension is probed, not typed.** "Test connection" embeds a short string and records the returned dimension in the space. A mismatch later (the server swapped the model behind the same name) marks the space stale and asks for a re-index.
- **E8. Code may use its own embedder.** There are two embedder slots, **text** (tickets, comments, plans, changelogs, commits) and **code**. The code slot defaults to "use the text embedder"; the user can point it at a code-specific model instead (for example `nomic-embed-code`, `jina-embeddings-v2-base-code` or `voyage-code-3`). Each slot gets its own embedding space. A query searching both kinds embeds the query once per space.
- **E9. Remote endpoints are explicit.** If an embedder or reranker URL is not loopback or a private address, Settings shows that ticket text, or code, will be sent to that host. A per-project setting **"Never send code to a remote embedder"** (default on) skips code indexing for that project unless the code slot is local.
- **E10. Reranking is optional.** A reranker URL (TEI `POST /rerank`, Jina/Cohere-compatible `POST /v1/rerank`, llama.cpp `--reranking`) rescores the top candidates (default 50) with a cross-encoder. Without a reranker the RRF order is used. A reranker failure or timeout falls back to RRF and is logged, never surfaced as a search error.
- **E11. Embedding and reranking calls are made by the backend on the host**, not from agent containers. Agents reach search only through KanbAIn (E22). Token counts from remote embedders are logged as events, like the model proxy's (D40), so cost shows up in Phase 8 stats.

### Chunking
- **E12. Code is chunked by syntax with tree-sitter, not by fixed token windows.**
  - One chunk per function, method, class/struct/impl/interface, enum and top-level type alias.
  - Definitions are located with each grammar's tags query (`tags.scm`), which makes the chunker mostly language-agnostic.
  - A class larger than the token budget is split into its methods. Each method chunk keeps a short header with the enclosing class/impl signature.
  - Small adjacent top-level items (constants, imports, tiny functions) are merged up to the budget.
  - Files in languages without a grammar, and code outside any definition, fall back to blank-line-separated blocks with a line-window cap.
  - Chunk text sent to the embedder starts with a context header: repo, path, language and symbol path (for example `crates/db/src/models/issue.rs › impl Issue › fn update`). The stored chunk keeps start/end lines for navigation.
- **E13. First languages:** Rust, TypeScript/TSX, JavaScript, Python, Go, Java, C#, C/C++. Others can be added by dropping in a grammar crate and its tags query.
- **E14. What is not indexed:** anything git ignores, lockfiles, vendored and generated code (`node_modules`, `target`, `dist`, `*.min.js`, `shared/types.ts`-style generated files via a per-project glob list), binaries, and files above a size cap (default 500 KB).
- **E15. Text is chunked by structure.** A ticket becomes:
  - one chunk for title plus the first part of the description;
  - further description chunks split at Markdown headings and paragraphs (about 400 tokens, small overlap);
  - one chunk per comment, plan, changelog entry, issue found and prior-work summary.
  Each chunk header carries the ticket's IDs (internal and External) and title, so a hit is always attributable.

### Indexing
- **E16. Incremental and content-addressed.** Every chunk stores a hash of its embedded text. A changed source is re-chunked, and only chunks whose hash changed are re-embedded. Code is keyed by git blob SHA, so a file that didn't change between commits is never re-chunked.
- **E17. Code is indexed at the project repos' default branch (`main`).** Ticket branches are not indexed while in progress. When a ticket is merged (D26), the merged files are re-indexed and linked to the ticket (E19).
- **E18. Triggers:**
  - ticket tables: the SQLite commit hook that already feeds live updates (Phase 0b) also enqueues index jobs, debounced per source;
  - repos: when a repo is linked to a project, when a merge into `main` happens in KanbAIn, on app start (cheap SHA comparison), and on a manual **Reindex** button;
  - embedder change: a full rebuild of the affected space (E4).
- **E19. Ticket ↔ code links.** Commits whose message starts with a ticket's ID (D16) link that ticket to the files and symbols they changed. Search results can then say "KAN-12 changed `fn update` in `issue.rs`". The links also come from the D18 git-history check for tickets from before KanbAIn.
- **E20. Background and polite.** One indexing worker with a bounded queue, batching (default 32 inputs per call), a concurrency limit per endpoint, exponential backoff on 429/5xx, and a pause while agent containers are at their D28 limit if the embedder is local and the machine is busy (setting).

### Using it
- **E21. API.**
  - `POST /api/search`: `{ query, project_id?, kinds?: [ticket, comment, plan, changelog, issue_found, commit, code], repo_ids?, limit?, rerank?: bool }`. It returns hits with kind, score, snippet, and a target: ticket id (and comment id), or repo, path, line range and symbol.
  - `GET /api/issues/{id}/related`: similar tickets plus related code locations for one ticket, using its title, description and acceptance criteria as the query.
  - `GET /api/embeddings/status`: per-space progress (queued, embedded, failed), and the active and building spaces.
  - `POST /api/embeddings/reindex`: rebuild a project, repo or space.
- **E22. Agents.**
  - **Planner context.** Before a planner run (Phase 4), the backend runs `related` for the ticket and writes `/kanbain/context/related.md`: the top similar tickets with their status, changelog summary and merged files, and the top code locations with path, line range and symbol. The planner prompt says to check it before exploring.
  - **Search tool.** `kanbain-mcp` gets a `search_knowledge` tool that calls `/api/search` with the run's token. In sandboxed runs (Phase 1) it is reached through the per-run proxy token, scoped to the run's project.
  - **Coder context.** Coder runs get the planner's subtask file list, plus the code hits for those files.
- **E23. UI.**
  - A **Related** section in the ticket panel: similar tickets (with status and a "done in KAN-12" hint) and code locations (open in editor, open diff of the linking commit).
  - A semantic mode in the command bar search.
  - In Settings → **Search**: the text, code and reranker endpoints, each with Test connection (which shows model and dimension), the per-project remote-code switch, index status and Reindex.

---

## 2. Data model (`embeddings.sqlite`)

```sql
-- One per embedder configuration (E4). Vectors of different spaces are never compared.
CREATE TABLE embedding_spaces (
    id            INTEGER PRIMARY KEY,
    purpose       TEXT NOT NULL CHECK (purpose IN ('text', 'code')),
    provider      TEXT NOT NULL,          -- openai | tei
    base_url      TEXT NOT NULL,
    model         TEXT NOT NULL,
    dimension     INTEGER NOT NULL,
    distance      TEXT NOT NULL DEFAULT 'cosine',
    query_prefix  TEXT NOT NULL DEFAULT '',
    doc_prefix    TEXT NOT NULL DEFAULT '',
    config_hash   TEXT NOT NULL UNIQUE,
    state         TEXT NOT NULL CHECK (state IN ('building', 'active', 'retired')),
    created_at    TEXT NOT NULL,
    activated_at  TEXT
);

-- A thing that was chunked: a ticket field, comment, plan, changelog entry, commit or file.
CREATE TABLE sources (
    id            INTEGER PRIMARY KEY,
    kind          TEXT NOT NULL,          -- ticket | comment | plan | changelog | issue_found | commit | code
    project_id    BLOB NOT NULL,
    ref_id        BLOB,                   -- issue / comment / plan id in db.v2.sqlite
    repo_id       BLOB,
    path          TEXT,
    blob_sha      TEXT,                   -- code: git blob, for incremental indexing (E16)
    content_hash  TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (kind, ref_id, repo_id, path)
);

CREATE TABLE chunks (
    id            INTEGER PRIMARY KEY,
    source_id     INTEGER NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    ordinal       INTEGER NOT NULL,
    symbol        TEXT,                   -- code: `impl Issue › fn update`
    language      TEXT,
    start_line    INTEGER,
    end_line      INTEGER,
    text          TEXT NOT NULL,          -- what was embedded, header included
    token_count   INTEGER NOT NULL,
    text_hash     TEXT NOT NULL
);

-- Which chunk is embedded in which space, with the model and dimension that produced it (E4).
CREATE TABLE chunk_embeddings (
    chunk_id      INTEGER NOT NULL REFERENCES chunks(id) ON DELETE CASCADE,
    space_id      INTEGER NOT NULL REFERENCES embedding_spaces(id) ON DELETE CASCADE,
    model         TEXT NOT NULL,
    dimension     INTEGER NOT NULL,
    text_hash     TEXT NOT NULL,          -- hash at embed time; differs from chunks.text_hash => stale
    embedded_at   TEXT NOT NULL,
    PRIMARY KEY (chunk_id, space_id)
);

-- One vec0 table per space, created with that space's dimension:
-- CREATE VIRTUAL TABLE vec_space_<id> USING vec0(
--     chunk_id INTEGER PRIMARY KEY,
--     embedding float[<dimension>] distance_metric=cosine,
--     project_id TEXT PARTITION KEY,
--     kind TEXT                          -- metadata column for filtering
-- );

CREATE VIRTUAL TABLE chunks_fts USING fts5(text, symbol, content='chunks', content_rowid='id');

-- Ticket ↔ code links from commits (E19).
CREATE TABLE ticket_code_links (
    issue_id      BLOB NOT NULL,
    repo_id       BLOB NOT NULL,
    path          TEXT NOT NULL,
    symbol        TEXT,
    commit_sha    TEXT NOT NULL,
    PRIMARY KEY (issue_id, repo_id, path, symbol, commit_sha)
);

CREATE TABLE index_jobs (
    id            INTEGER PRIMARY KEY,
    space_id      INTEGER,
    source_kind   TEXT NOT NULL,
    source_ref    TEXT NOT NULL,
    state         TEXT NOT NULL,          -- queued | running | failed
    attempts      INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    not_before    TEXT
);
```

**Switching embedders (blue/green):** a new configuration creates a `building` space and a new `vec_space_<id>` table. The worker re-embeds every chunk of that purpose into it while queries keep using the `active` space. When it is complete, the new space becomes `active`, and the old one becomes `retired` and its vec table is dropped. Chunks and FTS are reused, because chunking does not depend on the model.

---

## 3. Code layout

- **`crates/embeddings`** (new):
  - `store.rs`: opens `embeddings.sqlite` with sqlite-vec registered (`sqlite3_auto_extension` with the `sqlite-vec` crate's static init, so no DLL ships on Windows), migrations, the space lifecycle, and vec0 table management.
  - `client.rs`: `Embedder` and `Reranker` traits, with OpenAI-compatible, TEI and Jina/Cohere-rerank implementations; batching, retries, prefix handling and the dimension probe.
  - `chunk/text.rs`: Markdown-aware ticket chunking (E15).
  - `chunk/code.rs`: the tree-sitter chunker driven by tags queries (E12–E14), with a fallback splitter.
  - `index.rs`: the job queue and worker (E16–E20).
  - `search.rs`: hybrid KNN plus FTS5, RRF, the optional rerank, and filters.
- **`crates/services`**: wiring into the ticket commit hook (enqueue), merge and repo-link events, config, and the encrypted API keys (shared with D33).
- **`crates/server/src/routes/search.rs`** (E21) — the existing file search route moves or is renamed to avoid confusion.
- **`crates/mcp`**: the `search_knowledge` tool (E22).
- **`packages/web-core`**: the Related section, semantic command-bar search, and Settings → Search (E23).

---

## 4. Phases

Each phase ends with a demo and a dev-log entry, as in `IMPLEMENTATION_PLAN.md`. Phases E1–E3 only need Phase 0b. E5 needs Phase 1 (sandbox and proxy) and Phase 4 (planner).

### E0: Spike (1–2 days)
1. Load sqlite-vec into the sqlx SQLite (bundled `libsqlite3-sys`) through `sqlite3_auto_extension`. Create a vec0 table with a partition key and a metadata column, and run a filtered KNN. Check that the Windows CI build links it statically.
2. Pin the versions of sqlite-vec, `tree-sitter` and the grammar crates. Check grammar/core version compatibility; this is a common source of build breaks.
3. Measure on this repo: chunk count, chunking time, and embedding time with a local Ollama `nomic-embed-text` and with a code model.

**Demo:** a test inserts 1,000 random vectors and gets the expected nearest neighbours, filtered by project, on Linux and in the Windows CI job.

### E1: Store, embedder clients, settings
1. `crates/embeddings` store, migrations, and space lifecycle (E4, §2).
2. Embedder and reranker clients (E6, E7, E10), with tests against a mock HTTP server for both API shapes, batching and prefix handling.
3. Settings → Search with text, code and reranker endpoints, Test connection, and the remote-endpoint warning (E9).

**Demo:** pointing Settings at a local Ollama shows model and dimension; changing the model creates a new building space.

### E2: Tickets
1. The text chunker (E15) and indexing of tickets, comments and attachments' extracted text (once Phase 3 extracts it), triggered from the commit hook (E18).
2. Hybrid search with RRF (E5), `POST /api/search`, and `GET /api/issues/{id}/related`.
3. The **Related tickets** part of the ticket panel.

**Demo:** with 30 seeded tickets, a new ticket "users cannot reset their password" lists the earlier "password reset email never arrives" ticket first, although they share few words. Searching `PROJ-123` finds that ticket by keyword. Editing a description re-embeds only the changed chunks.

### E3: Code
1. The tree-sitter chunker for the E13 languages, with tags queries, class splitting, merging small items, context headers and the fallback (E12, E14).
2. Repo indexing at `main`, incremental by blob SHA (E16, E17), and the separate code embedder slot (E8) with the per-project remote-code switch (E9).
3. Code hits in search and in **Related** (open in editor).

**Demo:** in this repo, the query "where are ticket numbers assigned" returns `Issue::create` in `crates/db/src/models/issue.rs` as a whole-function chunk. Switching the code slot to another model rebuilds only the code space while ticket search keeps working. A second index run after one file change re-embeds only that file's changed functions.

### E4: Reranking and quality
1. Rerank the top 50 (E10), with timeout fallback.
2. A small evaluation set (about 40 query → expected hit pairs over tickets and code of this repo) and a test that reports recall@10 and MRR with and without rerank, used to choose defaults (candidate count, RRF k, chunk budget).

**Demo:** the evaluation report shows the reranker's effect. Stopping the reranker server falls back to RRF without errors.

### E5: Agents
1. `related.md` in the planner's `/kanbain/context` (E22).
2. The `search_knowledge` MCP tool, reachable from sandboxed runs through the run token.
3. Ticket ↔ code links from merge commits and from the D18 history check (E19), shown in search hits.

**Demo:** a planner run for a ticket similar to a merged one cites the earlier ticket and its files in `plan.json`'s assumptions or likely files.

### E6: More sources
As their phases land, index plans (Phase 4), issues found (Phase 6), changelog entries (Phase 7) and commit messages. The Phase 8 stats show embedding token use and cost for remote embedders.

---

## 5. Defaults

| Setting | Default |
|---|---|
| Text embedder | none (search disabled until configured); suggested local default `nomic-embed-text` via Ollama `http://localhost:11434/v1` |
| Code embedder | same as text |
| Reranker | off; suggested local `bge-reranker-v2-m3` via TEI or llama.cpp |
| Chunk budget | code 512 tokens, text 400 tokens with 40 tokens overlap |
| Candidates | 50 from vectors, 50 from FTS5, RRF k = 60, top 10 returned |
| Batch size | 32 inputs per embedding request |
| File size cap | 500 KB |
| Remote code embedding | off per project |

---

## 6. Risks and mitigations
- **sqlite-vec maturity** (pre-1.0, API changes between minor versions). Pin the version, wrap it behind `store.rs`, and keep FTS5 working on its own so search still works if the vec table must be rebuilt.
- **Embedder drift behind the same model name** (E7). Check the dimension on every batch, and sample a canary text on start; a changed vector marks the space stale.
- **Large monorepos.** Initial indexing can take long with a local CPU model. Show progress, index the most recently changed files first, and allow per-project path include/exclude globs.
- **tree-sitter build friction on Windows** (C compilers, grammar ABI versions). Covered by the E0 spike in Windows CI; grammars are compiled in, not loaded at runtime.
- **Leaking code to a remote embedder.** Off by default per project (E9), with the target host shown in Settings.
- **Stale results after a branch merge outside KanbAIn.** The start-up SHA comparison and the manual Reindex catch it; the index status shows the indexed commit per repo.

## 7. Open questions
- Should ticket branches in progress be searchable for the agents working on them (a per-run overlay index), or is `main` plus the ticket's own diff enough?
- Is one global index across projects useful ("did we solve this in another project?"), or should search stay per project by default with an opt-in "all projects"?
- Should the user be able to pin or hide related results to give the ranking feedback?
