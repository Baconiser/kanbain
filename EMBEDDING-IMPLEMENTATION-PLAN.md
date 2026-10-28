# KanbAIn Embedding and Reranking Plan

This plan adds local semantic search over tickets, ticket information, past runs and processed code. It has two goals, weighted equally:
- **Find earlier work**: before any work starts, KanbAIn (and its agents) can look up whether something similar was already done, and where in the code it lives.
- **Cut tokens and cost**: agents look things up instead of exploring the code with Grep/Glob/Read over whole files, and the saving is measured per run.

It is part of `IMPLEMENTATION_PLAN.md` (decisions D42–D47, Phases 3c–3e, and retrieval steps in Phases 4 and 6–8). This file holds the detailed design; where the two disagree, `IMPLEMENTATION_PLAN.md` wins. Written 2026-10-27, merged into the implementation plan 2026-10-28; nothing here is implemented yet.

The decisions below are numbered E1–E23. Their phases are named after the implementation plan's phases (§4), not E-numbers.

---

## 1. Settled decisions

### Storage
- **E1. Everything stays local.** No cloud vector database and no hosted index. The only thing that may leave the machine is the text sent to an embedding or reranking model, and only if the user points the model URL at a remote service (see E9).
- **E2. Vector store: SQLite with [sqlite-vec](https://github.com/asg017/sqlite-vec).** It runs inside the SQLite we already ship through sqlx, it builds on Windows, Linux and macOS, and it supports KNN with metadata filters and partition keys. libSQL/Turso was considered: it would mean replacing sqlx's SQLite driver across the whole app, which is out of proportion for this feature. Revisit only if sqlite-vec fails the Phase 3c spike.
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
- **E11. Embedding and reranking calls are made by the backend on the host**, not from agent containers. This is the stated exception to D34. Agents reach search only through KanbAIn (E22). Token counts from remote embedders are logged as events, like the model proxy's (D40), so cost shows up in Phase 8 stats.

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
  - one chunk for title plus the first part of the current description;
  - further description chunks split at Markdown headings and paragraphs (about 400 tokens, small overlap);
  - chunks for the original description version when it differs from the current one, and for attachments' extracted text;
  - one chunk per comment, plan, changelog entry, issue found and prior-work summary.
  A finished run becomes one **run summary** chunk (D44): task, agent, outcome, files changed, issues found and cost, built from `execution_process` and its logs, never the raw transcript.
  Each chunk header carries the ticket's IDs (internal and External) and title, so a hit is always attributable.

### Indexing
- **E16. Incremental and content-addressed.** Every chunk stores a hash of its embedded text. A changed source is re-chunked, and only chunks whose hash changed are re-embedded. Code is keyed by git blob SHA, so a file that didn't change between commits is never re-chunked.
- **E17. Code is indexed at the project repos' default branch (`main`), plus a ticket overlay (D45).** The files an in-progress ticket branch changes are indexed under that ticket's scope and searched only by that ticket's runs and views; everything else comes from `main`. When a ticket is merged (D26), its overlay is dropped and the merged files are re-indexed at `main` and linked to the ticket (E19).
- **E18. Triggers:**
  - ticket tables: the SQLite commit hook that already feeds live updates (Phase 0b, `crates/services/src/services/events.rs`) also enqueues index jobs, debounced per source;
  - runs: a finished `execution_process` enqueues its run summary; a commit on a ticket branch refreshes that ticket's overlay;
  - repos: when a repo is linked to a project, when a merge into `main` happens in KanbAIn, on app start (cheap SHA comparison), and on a manual **Reindex** button;
  - embedder change: a full rebuild of the affected space (E4).
- **E19. Ticket ↔ code links.** Commits whose message starts with a ticket's ID (D16) link that ticket to the files and symbols they changed. Search results can then say "KAN-12 changed `fn update` in `issue.rs`". The links also come from the D18 git-history check for tickets from before KanbAIn.
- **E20. Background and polite.** One indexing worker with a bounded queue, batching (default 32 inputs per call), a concurrency limit per endpoint, exponential backoff on 429/5xx, and a pause while agent containers are at their D28 limit if the embedder is local and the machine is busy (setting).

### Using it
- **E21. API.**
  - `POST /api/search`: `{ query, project_id? | all_projects?: bool, issue_id? (adds that ticket's overlay), kinds?: [ticket, comment, attachment, prior_work, run, plan, changelog, issue_found, commit, code], repo_ids?, limit?, rerank?: bool }`. It returns hits with kind, score, a capped excerpt, and a target: ticket id (and comment id), run id, or repo, path, line range and symbol (D43).
  - `GET /api/issues/{id}/related`: similar tickets plus related code locations for one ticket, using its title, description and acceptance criteria as the query.
  - `GET /api/embeddings/status`: per-space progress (queued, embedded, failed), and the active and building spaces.
  - `POST /api/embeddings/reindex`: rebuild a project, repo or space.
- **E22. Agents.**
  - **Search tool (Phase 3d).** `kanbain-mcp` gets a `search_knowledge` tool that calls `/api/search` with the run's token. In sandboxed runs (Phase 1) it is reached through the per-run proxy token, scoped to the run's project and its ticket overlay. It is enabled by a new agent capability `search`; the adapters map it to the MCP server for both runtimes. The default planner, coder and tester agents have it, and their role prompts say to search before exploring and to read only the returned line ranges.
  - **Planner context (Phase 4).** Before a planner run, the backend runs `related` for the ticket and writes `/kanbain/context/related.md`: the top similar tickets with their status, run summaries and merged files, and the top code locations with path, line range and symbol. It is capped (§5) and never put into the system prompt (D43). The task message says to check it before exploring.
  - **Coder context (Phase 5).** Coder runs get the planner's subtask file list, plus the code hits for those files.
  - **Measurement (D46).** Every `search_knowledge` call is logged as a run event with the result count and the excerpt bytes returned, next to the proxy's token events.
- **E23. UI.**
  - A **Related** section in the ticket panel: similar tickets (with status and a "done in KAN-12" hint), similar past runs, and code locations (open in editor, open diff of the linking commit).
  - **Possible duplicates** on the intake review screen (Phase 3a's dialog), next to the exact External-ID match (D17).
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
    kind          TEXT NOT NULL,          -- ticket | comment | attachment | prior_work | run | plan | changelog | issue_found | commit | code
    project_id    BLOB NOT NULL,
    scope         TEXT NOT NULL DEFAULT 'main', -- 'main', or an issue id for a ticket overlay (D45)
    ref_id        BLOB,                   -- issue / comment / attachment / execution_process / plan id in db.v2.sqlite
    repo_id       BLOB,
    path          TEXT,
    blob_sha      TEXT,                   -- code: git blob, for incremental indexing (E16)
    content_hash  TEXT NOT NULL,
    updated_at    TEXT NOT NULL,
    UNIQUE (kind, scope, ref_id, repo_id, path)
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
--     kind TEXT,                         -- metadata column for filtering
--     scope TEXT                         -- 'main' or an issue id (ticket overlay)
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

The phases are part of `IMPLEMENTATION_PLAN.md` §3, which has the authoritative step lists and demos. This section adds the embedding-specific detail. Each phase ends with a demo and a dev-log entry. 3c needs Phase 3a's extracted attachment text; 3d needs Phase 1 (sandbox and proxy) and Phase 2 (agent capabilities); the planner context needs Phase 4.

| Former name | Now | Scope |
|---|---|---|
| E0 Spike | Phase 3c, step 1 | sqlite-vec in sqlx, pinned versions, Windows CI |
| E1 Store, clients, settings | Phase 3c | `crates/embeddings`, clients, Settings → Search |
| E2 Tickets | Phase 3c | text sources incl. run summaries, hybrid search, Related, duplicates |
| E3 Code | Phase 3d | tree-sitter, repo index, ticket overlay |
| E5 Agents (search tool) | Phase 3d | `search_knowledge`, `search` capability, token benchmark |
| E4 Reranking and quality | Phase 3e | reranker, evaluation set, tuning |
| E5 Agents (planner context) | Phase 4 | `related.md` |
| E6 More sources | Phases 4, 6, 7, 8 | plans, issues found, changelogs, commits, stats |

### Phase 3c: Store and ticket search
1. **Spike first (1–2 days).** Load sqlite-vec into the sqlx SQLite (bundled `libsqlite3-sys` 0.30) through `sqlite3_auto_extension`. Create a vec0 table with a partition key and metadata columns (`kind`, `scope`), and run a filtered KNN. Pin sqlite-vec (the newest crates.io release is a pre-release; prefer the newest stable one that passes). Check that the Windows CI build links it statically; that needs a pushed spike branch, so ask first. Test: 1,000 random vectors return the expected nearest neighbours, filtered by project.
2. `crates/embeddings` store, migrations and space lifecycle (E4, §2).
3. Embedder and reranker clients (E6, E7, E10), tested against a mock HTTP server for both API shapes, batching and prefix handling. The reranker client is built here; using it in search is Phase 3e.
4. Settings → Search with text, code and reranker endpoints, Test connection and the remote-endpoint warning (E9).
5. The text chunker (E15) and indexing of tickets, description versions, comments, attachments' extracted text, prior-work summaries and run summaries, triggered as in E18.
6. Hybrid search with RRF (E5), the E21 routes, the Related section, possible duplicates at intake, and the command-bar semantic mode (E23).

**Demo** with the Hugging Face TEI CPU image as the embedder: with 30 seeded tickets, a new ticket "users cannot reset their password" lists the earlier "password reset email never arrives" ticket first, although they share few words. Searching `PROJ-123` finds that ticket by keyword. Dropping a near-duplicate shows it as a possible duplicate. Editing a description re-embeds only the changed chunks. Changing the model creates a building space while search keeps using the active one.

### Phase 3d: Code index and agent search
1. Pin `tree-sitter` and the grammar crates; check grammar/core ABI compatibility and the Windows CI build. Measure on this repo: chunk count, chunking time and embedding time.
2. The tree-sitter chunker for the E13 languages, with tags queries, class splitting, merging small items, context headers and the fallback (E12, E14).
3. Repo indexing at `main`, incremental by blob SHA (E16, E17), the ticket overlay (D45), and the separate code embedder slot (E8) with the per-project remote-code switch (E9).
4. Code hits in search and in **Related** (open in editor), with the Phase 3b ticket ↔ code links extended to symbols (E19).
5. The `search_knowledge` MCP tool and the `search` capability (E22), logged per call.
6. The token benchmark (D46): a fixed set of about 10 tickets on this repo, each run twice with the same agent, with and without `search`. The report lists tokens in/out/cache, cost, tool calls, files read and outcome per run, from the proxy events.

**Demo:** in this repo, the query "where are ticket numbers assigned" returns `Issue::create` in `crates/db/src/models/issue.rs` as a whole-function chunk. Switching the code slot to another model rebuilds only the code space while ticket search keeps working. A second index run after one file change re-embeds only that file's changed functions. A change on a ticket branch is found by that ticket's runs and not by others. The benchmark report shows the token difference.

### Phase 3e: Reranking and quality
1. Rerank the top 50 (E10), with timeout fallback.
2. An evaluation set of about 40 query → expected-hit pairs over this repo's tickets, run summaries and code, and a test that reports recall@10 and MRR with and without rerank.
3. Choose the defaults (candidate count, RRF k, chunk budget, excerpt cap, `related.md` cap) on the evaluation set and the Phase 3d token benchmark together: a setting that raises recall but also raises tokens per run needs a reason.

**Demo:** the evaluation and benchmark reports show the reranker's effect on quality and tokens. Stopping the reranker server falls back to RRF without errors.

### Later phases
- **Phase 4:** `related.md` in the planner's `/kanbain/context` (E22); plans are indexed. Demo: a planner run for a ticket similar to a merged one cites the earlier ticket and its files in `plan.json`'s assumptions or likely files.
- **Phase 5:** coder runs get the code hits for their subtask's files.
- **Phase 6:** issues found are indexed.
- **Phase 7:** changelog entries and merge commit messages are indexed; on merge the overlay is dropped and the merged files are re-indexed and linked (E17, E19).
- **Phase 8:** embedding token use and cost for remote embedders, search calls per run, and tokens per run with and without `search`.

---

## 5. Defaults

| Setting | Default |
|---|---|
| Text embedder | none (search disabled until configured); suggested local defaults: `nomic-embed-text` via Ollama `http://localhost:11434/v1`, or a TEI CPU container |
| Code embedder | same as text |
| Reranker | off; suggested local `bge-reranker-v2-m3` via TEI or llama.cpp |
| Chunk budget | code 512 tokens, text 400 tokens with 40 tokens overlap |
| Candidates | 50 from vectors, 50 from FTS5, RRF k = 60, top 10 returned |
| Batch size | 32 inputs per embedding request |
| File size cap | 500 KB |
| Remote code embedding | off per project |
| Search scope | current project; "all projects" is opt-in |
| Excerpt per hit | about 20 lines / 1,500 characters |
| `related.md` | top 5 tickets and runs, top 10 code locations, at most about 8 KB |

---

## 6. Risks and mitigations
- **sqlite-vec maturity** (pre-1.0, API changes between minor versions). Pin the version, wrap it behind `store.rs`, and keep FTS5 working on its own so search still works if the vec table must be rebuilt.
- **Embedder drift behind the same model name** (E7). Check the dimension on every batch, and sample a canary text on start; a changed vector marks the space stale.
- **Large monorepos.** Initial indexing can take long with a local CPU model. Show progress, index the most recently changed files first, and allow per-project path include/exclude globs.
- **tree-sitter build friction on Windows** (C compilers, grammar ABI versions). Covered by the Phase 3c spike (sqlite-vec) and the Phase 3d pinning step (grammars) in Windows CI; grammars are compiled in, not loaded at runtime.
- **Leaking code to a remote embedder.** Off by default per project (E9), with the target host shown in Settings.
- **Retrieval that costs more tokens than it saves.** Capped excerpts and `related.md` (D43), and defaults chosen on the token benchmark (D46).
- **Stale results after a branch merge outside KanbAIn.** The start-up SHA comparison and the manual Reindex catch it; the index status shows the indexed commit per repo.

## 7. Resolved questions (2026-10-28)
- **Branches in progress:** `main` plus a per-ticket overlay of the files the ticket branch changes (D45). No separate index per run.
- **Across projects:** per project by default, with an opt-in "all projects" (D47).
- **Pin/hide feedback:** not for now (D47).
