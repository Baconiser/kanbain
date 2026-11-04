# Where to proceed (as of 2026-11-04, Phase 7 done)

## State
- **Phase 7 (scribe, review and merge, changelog export)** is on branch
  `kanbain/phase-7`, from `main` at `71c195aa` (the conflict-resolver
  rebase). **Not merged or pushed** — needs your approval.
- Results, bugs found on the way and open items: `docs/kanbain-changelog.md`
  (Phase 7 entry). Demo: `.claude/skills/run-kanbain/driver/review.mjs`
  with `mock-script-review.json`; docs in the run-kanbain skill §8h.
- Two fixes to tests that were already red on `main`: the agent library
  test now expects the `setup` agent (added in 8982b26f).
  `embeddings::client::tests::rerank_batches_once_within_one_deadline` is
  timing-sensitive and can fail under a parallel `cargo test` (passes
  alone); not changed.

## Needs your decision
1. Merge `kanbain/phase-7` into `main`?
2. Windows CI + a Windows run (worktrees, squash merge, push from Merge,
   notifications).
3. A real-model run of planner → coders → tester → fixer → scribe: every
   demo uses the scripted mock, so the scribe's prose quality is untested.
4. OpenCode `session.prompt returned empty response body` (error.txt):
   still not investigated.
5. The agent rebase from Review ("Rebase with agent") has not run against a
   real model either.

## Next: Phase 8 (stats and charts)
- Projects overview, issues and cost views, retrieval stats.
- `GET /api/issues/{id}/review` already sums run and job cost per ticket;
  `changelog_entries` and `issues_found` have what the issue charts need.

## Memory rules (16 GB container), unchanged
- At most the embedder plus the reranker next to the backend, and only with
  no build, Vite or Chromium running. Otherwise one TEI container.
- Stop the backend and `kb-*` containers before any `cargo build`/`cargo test`.
- `CARGO_BUILD_JOBS=2 … -j 2`; watch `free -m`.
- Never edit a migration that has already run.
- Building tauri-app rewrites `crates/tauri-app/gen/schemas/*` (key order).
  Don't commit that noise; `git checkout crates/tauri-app/gen/schemas`.
- Restart scripts: kill only the listening process
  (`lsof -ti:<port> -sTCP:LISTEN`), or Vite dies with the backend.
