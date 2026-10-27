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
