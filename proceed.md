# Where to proceed (as of 2026-10-31, Phase 6 done)

## State
- **Phase 6 (tester and fix loop) is complete** on branch `kanbain/phase-6`
  (from `kanbain/workspace-ticket-sidebar`, which is 1 commit ahead of
  `main`): `5de9874f`, `119f9dd5`, `91fc63ab`, `ad5bdce5`. **Not merged or
  pushed** — needs your approval.
- Results, bugs found on the way and open items: `docs/kanbain-changelog.md`
  (Phase 6 entry). Demo: `.claude/skills/run-kanbain/driver/testing.mjs`
  (41/41 PASS); `orchestration.mjs` (Phase 5) updated and passing 34/34.
  Docs in the run-kanbain skill §8f.
- `cargo test --workspace` 336 passed; check, lint + clippy,
  generate-types:check, prepare-db:check, check-i18n all pass.
- Backend, Vite and mock are stopped.

## Needs your decision
1. Merge `kanbain/phase-6` (and the sidebar commit under it) into `main`?
2. Windows CI + a Windows run (worktrees, squash, notifications).
3. A real-model run of planner → coders → tester → fixer: all demos use
   the scripted mock.
4. OpenCode `session.prompt returned empty response body` (error.txt):
   still not investigated.

## Next: Phase 7 (scribe and changelogs)
- A scribe run after each coder/tester run; `changelog_entries` with
  `issues_fixed[]` can come from `issues_found` (status fixed, fixed_round).
- Review screen (D26) can reuse `GET /api/issues/{id}/coding`, which now
  carries checkpoints and issues found.

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
