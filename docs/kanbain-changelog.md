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

## Phase 1: Docker sandbox and model proxy (2026-10-28)

**Goal:** one agent (Claude Code, API key) works on one ticket inside a
container, every model call is logged and budgeted, and nothing on the host
is reachable.

### What changed

- **`crates/sandbox`.** A `Sandbox` trait with a Docker implementation on
  `bollard`: CPU, memory and pids limits, no capabilities,
  `no-new-privileges`, no bind mounts, labels per instance, workspace and
  kind. Orphaned containers of this data folder are removed at startup.
- **Base image.** `docker/agent/Dockerfile` (Node 22, git, Python, Rust,
  socat, ripgrep) with the Claude Code and OpenCode versions from
  `executors::versions`, which is now the one place they are pinned. The
  backend builds `kanbain-agent:<app version>-<hash>` in the background on
  first start. A project can name its own Dockerfile, built with
  `KANBAIN_BASE_IMAGE` as a build arg.
- **Network (D38).** Agents sit on an `internal` Docker network with no
  route anywhere. A gateway sidecar on that network and a normal bridge
  forwards two ports to the backend's model proxy and egress proxy. On
  native Linux the proxies listen on the bridge gateway address; on Docker
  Desktop they listen on 127.0.0.1 and the sidecar dials
  `host.docker.internal`.
- **Egress proxy.** `CONNECT host:443` only, for allowlisted hosts (package
  registries, GitHub release downloads, plus global and per-project extras),
  never to private addresses. Credentials are per run. Refusals are stored
  as `egress_block` events.
- **`crates/model-proxy` (D40).** Anthropic Messages and OpenAI-compatible
  pass-through with SSE streaming. Per-run tokens, real credentials added by
  the proxy, usage parsed including cache reads and 5m/1h cache writes,
  cost from an editable `<data>/prices.json`. Budgets are checked before a
  call and during the stream (estimating output from the streamed text); at
  the limit the stream ends with an error event and the token is revoked.
- **Credentials.** API keys for Anthropic and an OpenAI-compatible provider
  are stored AES-256-GCM encrypted in the data folder (Settings → Sandbox),
  falling back to the backend's `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`.
- **Code transfer (D36).** The branch goes into `/workspace/<repo>` as a git
  bundle. After each run, leftover changes are committed in the container
  and the branch is fetched back into the host worktree (fast-forward, or
  `reset --keep`). If the container is ahead (commits made in its terminal),
  those are pulled first instead of being overwritten.
- **Context dirs (D10).** `/kanbain` is an anonymous volume. The ticket goes
  to `/kanbain/context/ticket.md` (read-only for the agent); files the agent
  writes to `/kanbain/out/` are stored as a `sandbox` event at run end.
- **Executor integration.** `ExecutionEnv` carries a `SandboxTarget`, so
  Claude Code, scripts and dev servers become `docker exec` processes and
  the existing log, approval and normalization pipeline is unchanged. Only
  explicit variables reach the container. Stopping a run kills its process
  group inside the container. OpenCode refuses to run sandboxed until its
  adapter lands in Phase 2.
- **Run tracking (D41).** `execution_processes` gains ticket, sub-ticket,
  agent definition, container, branch, token, cost and budget columns.
  `execution_process_events` stores model calls, blocked egress, budget
  stops and sandbox steps.
- **Limits (D28).** Per-run budget (default $2), per-ticket budget (default
  $5; the run gets the smaller of the two remainders), 30-minute time limit,
  and at most 3 agent runs at once, the rest queued. A ticket over budget
  moves to its Blocked status with a comment.
- **Containers.** One container per workspace, kept between runs and
  removed with the workspace's worktree. The workspace terminal opens in it
  (with egress but no model access). A sandboxed dev server's announced
  port is forwarded to the same port on 127.0.0.1 through `docker exec` and
  `socat`, so the preview proxy reaches it on every platform.
- **UI.** Settings → Sandbox (Docker and image status, API keys, enable
  switch, limits, extra hosts), a banner while Docker or a key is missing,
  run cost in the Logs panel and a summary of the selected run (cost against
  budget, tokens, cache, model calls, blocked hosts). Project settings take
  extra hosts and a Dockerfile. Translated into all locales.

### Demo (Linux dev container)

The model upstream was the scripted mock in
`.claude/skills/run-kanbain/driver/mock_anthropic.py`, since no API key is
available here; Claude Code 2.1.119 itself ran unmodified in the container.

- **A run makes a commit that reaches the host branch.** `sandbox-run.mjs`:
  ticket `DEMO-1` → workspace → Claude Code in the container ran four Bash
  steps, including `git commit -m 'DEMO-1: add sandbox.txt'`. After the run
  the host worktree's branch has that commit plus a commit of the leftover
  uncommitted file, and `/kanbain/out/result.json` was harvested. 13 seconds
  end to end.
- **Every model call shows tokens and cost.** 5 `model_call` events
  (1500 in / 300 out each, $0.009 each); the run totals 7.5k → 1.5k tokens
  and $0.045 against a $2.00 budget. The upstream received the real key;
  the container only ever had its run token.
- **`curl https://example.com` is blocked and logged** (curl exit 56, an
  `egress_block` event); `registry.npmjs.org` returns 200 through the proxy.
  The agent can read `/kanbain/context/ticket.md` but not write to it.
- **A $0.05 budget cuts the run off.** A ticket budget of $0.05 and a
  50k-token streamed answer: the proxy ended the stream at $0.053, the run
  was killed, and the ticket moved to Blocked with a comment.
- **`docker inspect`** of the workspace container: no binds, one volume
  (`/kanbain`), the internal network, all capabilities dropped, 2 CPUs,
  4 GB, 1024 pids.
- **Terminal and preview:** the terminal opens as `node` in
  `/workspace/<repo>` (npm reachable, example.com blocked); a dev server in
  the container is served through the preview proxy, and stopping it
  removes the forward and its processes.
- `cargo test --workspace` (plus the Docker test with
  `KANBAIN_DOCKER_TESTS=1`), `pnpm run check`, `pnpm run lint` and the i18n
  check pass. `sandbox.mjs` checked Settings → Sandbox and the run summary
  with no console errors or failed API calls.

### Not verified here

- **A real Anthropic API key.** All runs used the mock upstream.
- **Docker Desktop on Windows.** The 127.0.0.1 + `host.docker.internal`
  path, the named pipe connection and `docker exec` piping need the user's
  check on Windows (risk noted in the plan).

### Known leftovers

- Starting a run waits inside the request while the first image build runs
  (about 8 minutes with the classic builder) or while all slots are busy.
- Paths in agent logs are container paths (`/workspace/...`); the commit
  reminder hook still looks at the host worktree.
- The concurrency limit applies after a restart. Run events in the UI poll
  every 3 seconds instead of streaming.
- Run tokens are passed as `docker exec -e` arguments, so they are visible
  in the host process list while a run lasts.
- MCP config is still the host's `~/.claude.json` concept and is not copied
  into containers (Phase 2, per-container MCP).
- Automated runs (and deleting their container at run end) arrive with the
  orchestrator in Phase 5; today every run is a workspace run.

## Phase 2: Agent profiles, styles, skills, both adapters (2026-10-28)

**Goal:** neutral agent definitions drive both runtimes.

### What changed

- **Agent library (D13).** Agent definitions (`agents/<name>.yaml`, the
  D31 format plus an optional test-command allowlist and MCP servers),
  style blocks (`styles/<name>.md`) and skills (`skills/<name>/SKILL.md`
  plus supporting files) live in the data folder. SQLite indexes them
  (`agent_definitions`, `style_blocks`, `skills`, including files that fail
  to parse), and a file watcher re-syncs edits made outside KanbAIn. The
  planner (Opus), coder and tester (Sonnet), scribe, intake and history
  (Haiku) agents are seeded on first start. Import takes zips, files or a
  folder; export gives one agent with its styles and skills, or everything.
- **Adapters (`crates/executors/src/adapters`).** A `RuntimeAdapter` turns
  an agent with its styles and skills into arguments, environment and
  files. Role and styles are appended to each runtime's own system prompt,
  and the task stays the first message.
  - **Claude Code:** `--append-system-prompt`, `--tools` from the
    capabilities, `--max-turns`, `--max-budget-usd` as a second limit, a
    `PreToolUse` hook (passed with `--settings`) that only lets the test
    allowlist through for the `test` capability, and `--strict-mcp-config`
    with the agent's own MCP servers, so the host's MCP config is never used.
  - **OpenCode:** everything goes in `OPENCODE_CONFIG_CONTENT`: providers
    pointing at the model proxy, a primary agent with `steps` and the
    fine-grained `permission` map (deny only, so supervised runs still ask),
    `instructions` with role and styles, and `provider/model-id` models
    (`openai/<model>` uses an OpenAI-compatible provider).
- **CLI drift check.** `--max-turns` is hidden in Claude Code 2.1.119's
  help but exists. Snapshots of both pinned CLIs' interfaces (help text,
  `--agents` fields, OpenCode agent, permission and provider keys) are
  checked in; a test fails when a pinned version changes without a new
  snapshot, or when an adapter emits a flag or key the CLI doesn't have.
  `scripts/snapshot-agent-clis.sh` regenerates them from the agent image.
- **Runs.** A coding run whose executor config names an agent definition
  resolves it and runs its adapter. Follow-ups and reviews inherit the
  session's agent. The generated files and skills go read-only under
  `/kanbain` in the container (`~/.claude/skills` links there, and both
  runtimes scan it). The run records `agent_def_id`. The agent's budget
  caps the proxy budget and its `wall_minutes` the time limit. Agent
  definitions require the sandbox.
- **OpenCode in the sandbox.** `opencode serve` runs in the container and
  the backend reaches it through a tunnelled host port; it sees container
  paths. models.dev fetching and update checks are off.
- **Claude account (D33).** Settings → Sandbox → "Connect Claude account"
  runs `claude setup-token` in a throwaway container that can reach only
  the Anthropic login hosts. It shows the sign-in link, takes the pasted
  code, and stores the token AES-256-GCM encrypted. When connected, the
  proxy sends it as a Bearer token with the `oauth-2025-04-20` beta
  (merged with the client's betas) instead of the API key. Costs are still
  API-equivalent estimates and budgets apply to them.
- **UI.**
  - Settings → Agents replaces the executor profile editor: an agent list
    and editor, a test run that opens a workspace on a scratch repo,
    style blocks showing their users, a skills manager with a file list
    and editor, and import/export.
  - The create-workspace chat box can pick an agent definition, which then
    decides executor and model.
  - Translated into all locales.

### Demo (Linux dev container)

The model upstream was the scripted mock again (no API key or Claude
account here). Claude Code 2.1.119 and OpenCode 1.4.7 ran unmodified in
the container.

- **One ticket, one agent, both runtimes** (`agent-demo.mjs`):
  `demo-coder` with a style block and a skill imported from a zip ran the
  same ticket on Claude Code, on OpenCode with Anthropic through the
  proxy, and on OpenCode with `openai/gpt-4.1-mini` through the
  OpenAI-compatible proxy route.
  - Every run completed and recorded `demo-coder`.
  - The upstream saw the real keys, never the run token.
  - The role and style text were in the system prompt, and the skill was
    listed and readable inside the container.
  - The commit came back to the host branch.
- **Editing the style block** changed the next Claude Code and OpenCode
  runs' system prompts (V1 gone, V2 present).
- **Tools follow the capabilities.**
  - The coder got Read/Glob/Grep/Agent/Edit/Write/NotebookEdit/Bash plus
    Skill on Claude Code, and no web tools on OpenCode.
  - A tester limited to `git status*` and `ls*` had `rm -rf /tmp/x` and
    `ls && curl …` refused by the hook while the allowed commands ran.
  - The skills folder is root-owned and not writable by the agent.
- **Claude account:** the sign-in link appears in the UI. A wrong code
  reaches `platform.claude.com` through the egress proxy and comes back
  as "Claude sign-in failed: Request failed with status code 400", and
  the login container is removed.
- `agents.mjs` (UI) passed:
  - Settings → Agents: the defaults are listed; a style created,
    attached to `coder` and edited; a skill created and its `SKILL.md`
    edited; a skill zip imported.
  - Test run opens a workspace named after the agent.
  - The chat-box picker switches the executor.
  - The Claude sign-in link is shown and cancelling works.
  - No console errors or failed API calls.
- `cargo test --workspace` (new library, adapter, drift, proxy OAuth and
  login parser tests), `pnpm run check`, `pnpm run lint` and the i18n
  checks pass; `smoke.mjs` and `tickets.mjs` still pass.

### Not verified here

- **A real Claude account.** Neither the token exchange with a real code
  nor subscription runs through the proxy could be checked. OpenCode's
  requests may be rejected for subscription tokens, because they lack
  Claude Code's system prompt; the API key path is the fallback (plan
  risk).
- **Windows.** The login PTY (ConPTY + `docker exec -it`) and the
  OpenCode tunnel need the user's check on Windows.

### Known leftovers

- **OS keychain.** The encryption key still lives in `<data>/secret.key`.
  Moving it to the OS keychain through Tauri was left out because it
  can't be tested here and could lock out existing secrets.
- **Model prices.** `prices.json` has no OpenAI-compatible models, so
  their runs cost $0 and their budgets aren't enforced until prices are
  added there.
- **Follow-ups.** The follow-up chat box still shows the model selector,
  although an agent session's follow-ups run the session's agent, and a
  follow-up can't drop the agent.
- **MCP settings.** Settings → MCP Servers edits the host config, which
  sandboxed agent runs no longer use. MCP servers for agents come from
  their definitions only.
- **Profiles.** `default_profiles.json` and a user `profiles.json` still
  configure runs without an agent definition, but have no UI any more.
  The `@rjsf` packages are unused and can be removed from `package.json`.
