# KanbAIn development log

This is the fork's own dev log, one entry per phase of
[`IMPLEMENTATION_PLAN.md`](../IMPLEMENTATION_PLAN.md). It is not a
project-repo artifact.

## Phase 0a: Rename and delete (2026-10-27)

**Goal:** a KanbAIn-branded app with only the local feature set, which builds
and runs.

### What changed

- **Cloud removed (backend).** The remote server, relay crates, trusted-key
  auth, embedded SSH, desktop bridge, api-types, OAuth/organization routes,
  remote client/sync, PostHog analytics, Sentry, the Tauri auto-updater and
  the `review` CLI (it uploaded code to upstream servers). The preview proxy
  no longer routes `<port>--<host>` relay subdomains.
- **Cloud removed (frontend).** `packages/remote-web` and, in `web-core` and
  `local-web`, the Electric-backed kanban board and issue panels,
  organizations, sign-in, remote host pairing, cloud notifications, export,
  release notes, remote host scoping (`/api/host/<id>`) and the `remote` app
  runtime.
- **Board.** The cloud board was deleted rather than stubbed (decision
  recorded in the plan). A `/tickets` placeholder holds its place until the
  local board in Phase 0b. The presentational kanban components in
  `packages/ui` remain for 0b to reuse.
- **Executors.** Only Claude Code and OpenCode remain, in the backend,
  profiles, icons, i18n and docs.
- **Rename.** Vibe Kanban became KanbAIn across crates, `@kanbain/*` packages,
  binaries (`kanbain-mcp`, `kanbain-tauri`), `KANBAIN_*` env vars, data and
  temp folders, the Tauri identifier `dev.kanbain.app`, UI strings, i18n, docs,
  logo and icons. The README was rewritten; it now states the pinned nightly
  toolchain.
- **CI.** Remote/relay workflows and jobs removed. New `Windows build`
  workflow builds the Tauri installers on `windows-latest` and uploads them.
- **Run skill.** `.claude/skills/run-kanbain` launches the app headless and
  drives it with pinned Playwright Chromium.

### Demo (Linux dev container)

- `cargo test --workspace`, `pnpm run check` and `pnpm run lint` (ESLint,
  clippy `-D warnings`, unused i18n keys) pass.
- The `run-kanbain` smoke driver against `server` plus Vite:
  - onboarding;
  - the workspaces page;
  - the `/tickets` placeholder;
  - Settings → Agents lists only Claude Code and OpenCode.

  The page title is "KanbAIn". No console errors and no failed API calls.

### Not verified here

- **A manual workspace run with Claude Code.** It needs an authenticated
  Claude account, so it is left to the user.
- **The Windows installer.** The `Windows build` workflow runs once the branch
  is pushed.

### Known leftovers (follow-ups)

- **Config fields.** `relay_enabled`, `analytics_enabled`,
  `remote_onboarding_acknowledged` and `host_nickname` still exist in the
  config. The v1–v8 config migration chain, the legacy `.vibe-images`
  attachment migration and the unused credential/relay key paths in
  `utils::assets` are dead weight now that old data is not migrated.
- **Leftover kanban UI state.** The UI preference scratch still stores
  `selected_org_id`, and `shared/keyboard/registry.ts` still lists the kanban
  shortcuts. Phase 0b replaces them.
- **Distribution.** `npx-cli` is renamed, but it still downloads release
  bundles from upstream's storage, which KanbAIn doesn't publish to.
- **Docs.** The remaining docs pages describe upstream screens in places and
  need a pass once the KanbAIn flows exist.

## Phase 0b: Local tickets in SQLite (2026-10-27)

**Goal:** the kanban board works fully offline on SQLite.

### What changed

- **Schema.** One migration replaces the legacy `tasks` table and the
  remote-linked `projects` table with local `projects` (ticket prefix,
  External ID key/pattern, settings JSON, issue counter), `project_repos`,
  `project_statuses` (with a stable `kind`), `issues` (per-project number,
  priority, parent, External ID, ask/yolo `mode`, `fix_round`, budget),
  `project_tags`/`issue_tags`, `issue_relationships`, `issue_comments`
  (author `user` or `agent:<run>`) and `issue_attachments`. Workspaces link to
  a ticket through `workspaces.issue_id`. Ticket labels live in
  `project_tags` because `tags` already holds the chat prompt snippets.
- **Default statuses** per project: Backlog, Planning, Awaiting approval,
  In progress, Testing, Review, Done and Blocked, each with its `kind`.
  Moving a ticket to a `done` status sets `completed_at`.
- **API.** Local CRUD for projects, repos, statuses, tags, tickets (including
  a bulk move for drag and drop), ticket tags, relationships, comments and
  attachments. Parent cycles, foreign statuses and cross-project links are
  rejected. A missing row is now a 404.
- **Live updates.** Row changes in the ticket tables are buffered per SQLite
  connection and pushed as JSON-Patch once their transaction commits, so
  streams never show uncommitted or rolled-back state. WebSocket streams:
  all projects, one project's board, one ticket's comments.
- **Board.** `/tickets` replaces the placeholder: project picker and
  settings (repos, statuses with pipeline role, hide/reorder, tags, External
  ID key and pattern, history "always allow"), drag and drop, filter, and a
  ticket panel (status, priority, mode, External ID, budget, parent, tags,
  description with pasted attachments, sub-tickets, relationships,
  workspaces, comments). "Start workspace" opens create mode linked to the
  ticket, seeded with the project's repos; the branch is named after the
  ticket's ID.
- **Attachments** are stored locally and `attachment://` references in the
  editor resolve to `/api/attachments/<id>/file`.
- **Backups (D12).** A `VACUUM INTO` copy before any pending migration (last
  5 kept) and a daily copy (checked hourly, count set in Settings → General
  → Data, default 7) in `<data>/backups/`. `GET /api/export?format=json|
  markdown[&project_id=]` exports everything; Settings links to it.
- **Launch token (D3).** Every `/api` route except `/health` requires the
  per-launch token as the `x-kanbain-token` header or an HttpOnly,
  SameSite=Strict cookie. Tauri opens `/api/auth/session?token=…`, which sets
  the cookie and redirects into the app. The token is written to
  `<data>/launch-token` (mode 0600) for `kanbain-mcp`. `pnpm run dev`
  generates one token and hands it to both the backend and the Vite proxy
  through `KANBAIN_LAUNCH_TOKEN`, so a second backend instance rewriting the
  file cannot break the dev session.
- **Cleanup.** `shared/remote-types.ts`, dead selection builders and the
  issue/kanban keyboard shortcuts are gone.
- **Fixes found while verifying.** "Start workspace" picks each repo's
  default target branch (or its checked-out branch) so Create isn't blocked
  on "Select branch". The default branch prefix is `kanbain` instead of
  upstream's `vk`; configs still on `vk` move over. The Data settings card is
  translated into all locales, which the CI i18n check requires.

### Demo (Linux dev container)

- `cargo test --workspace` passes, including new model, backup, export and
  token tests. `pnpm run check` and `pnpm run lint` pass.
- `.claude/skills/run-kanbain/driver/tickets.mjs` (new) against the dev
  setup: creates a project and a ticket with an External ID, adds a tag, a
  sub-ticket, a comment and a priority, sees a rename live in a second
  window, drags the ticket to In progress (seen live in the second window),
  reloads and finds everything persisted. No console or API errors.
- Without the token the API answers 401; with the header or cookie, 200.
  The sign-in route refuses bad tokens and off-site redirects.
- The Tauri path, simulated in Chromium against the backend serving the
  built frontend: `/api/auth/session` sets the cookie and lands on
  `/tickets`; the board, panel and all 7 WebSockets work with no failed
  API calls.
- A daily backup is written at startup when none is younger than a day.
- **Manual workspace from a ticket with Claude Code**, through the UI: a
  ticket with External ID `DEMO-7` in a project linked to a scratch repo →
  Start workspace → the create screen comes up with the prompt, the repo on
  `main` and the linked-ticket badge → Create. Claude Code (Opus) ran in the
  new worktree, wrote `hello.txt` and committed it. The workspace has the
  ticket's `issue_id`, its branch is named after `DEMO-7`, and the ticket
  panel lists it.
- **Restart:** after adding a comment, the backend process was killed,
  rebuilt and started again. All projects, the ticket, the comment and the
  linked workspace were still there, with no failed API calls.
- **Windows CI:** the `Windows build` workflow built the installers for the
  pushed Phase 0b commits.

### Not verified here

- **Windows at runtime.** The installer builds in CI, but the Tauri
  sign-in flow, export downloads from the WebView2 window and backups under
  `%APPDATA%` need the user's check on a Windows machine.

### Known leftovers

- **i18n.** The new board uses English strings; only the Settings → Data
  card has (English) translation keys.
- **VS Code embed.** The `/workspaces/<id>/vscode` page opened by the
  upstream VS Code extension has no way to get the launch token and now
  gets 401s.
- **No frontend unit tests.** Vitest is not installed in this repo; the
  board logic is covered by the Playwright driver instead.
