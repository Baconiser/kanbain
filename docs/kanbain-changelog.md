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

## Phase 3a: Intake, External ID, description versions, timeline (2026-10-28)

**Goal:** a dropped Jira ticket becomes a structured ticket after review,
dropping it again adds a description version, and every ticket has a
live timeline.

### What changed

- **Headless agent jobs (D34).** Intake and history analysis run an agent
  definition of that role in a throwaway sandbox container with no repo
  and no tools (`SandboxRuntime::run_job`, `local-deployment/src/jobs.rs`).
  The task, with all the text the agent needs, is the first message (a
  tool-less agent can't read `/kanbain/context`, which still gets copies);
  the final message is the result. Claude Code runs as
  `claude -p --output-format json` with the task on stdin, OpenCode as
  `opencode run --format json` with the task attached as a file.
  - The container takes a sandbox slot, reaches only the model proxy and
    the default package registries, and is removed afterwards.
  - Jobs are rows in a new `agent_jobs` table with the same token and cost
    columns as runs. The model proxy books a job's calls there.
  - **Deviation from D41:** runs are still execution processes, but jobs
    have no workspace or session to hang an execution process on, so they
    get their own table instead.
- **Attachment text (D15).** PDF (`pdf-extract`), HTML and Jira's "Word"
  export (HTML saved as `.doc`, `html2text`), DOCX (`zip` + `quick-xml`),
  Jira XML exports (element labels plus the escaped HTML in descriptions
  converted) and plain text. All of these are pure Rust. The text is stored
  in `attachment_texts` for the intake agent and for search (3c). Linking
  a file to a ticket extracts it too.
- **Intake (D15).**
  - **Drop ticket** on the board opens a dialog for pasted text (Jira
    wiki markup included) plus dropped, chosen or pasted files.
  - The intake agent answers with the `enhanced_ticket.json` shape: title,
    clean description, acceptance criteria, open questions, External ID and
    a priority guess.
  - The review screen shows the original text and each attachment's
    extracted text next to the editable result, with the agent, model,
    tokens and cost.
  - If the agent fails, the ticket can still be made from the original,
    or the agent retried.
  - Acceptance criteria and open questions become Markdown sections of the
    description.
  - The intake is kept in `ticket_intakes` (with `intake_attachments`, which
    protects its files from the orphan cleanup).
- **External ID (D16, D17).**
  - Detection uses the project's pattern (default
    `\[?[A-Z][A-Z0-9]*-\d+\]?`) and key, and never matches the project's
    own prefix.
  - An External ID the agent invents is replaced by the detected one.
  - A dropped ticket whose ID exists offers "Update KAN-n (new
    description version)" with a line diff, or "Create a new ticket".
  - The New ticket dialog fills the ID from the title and description and
    warns before creating a second ticket with the same ID.
  - Tickets' workspaces now branch as `kanbain/<ID>-<slug>` (numbered when
    another workspace has the name). Leftover commits are prefixed
    `<ID>: `, and the sandbox's `ticket.md` asks agents to do the same.
- **Description versions (D17).**
  - `issue_descriptions` stores each version's title and description, its
    kind (`original`, `enhanced` or `edited`), the dropped text and the
    agent's version.
  - Existing tickets got version 1 in the migration.
  - Edits in the ticket panel become an `edited` version; edits within ten
    minutes of it update it, so the autosave doesn't make a version per
    pause.
- **Timeline (D19).**
  - `issue_events` is filled by SQLite triggers for created, status,
    title, priority, External ID, mode, description versions, comments,
    attachments, and runs started and finished (agent, duration, tokens,
    cost). Board drags and bulk updates are recorded like every other path.
  - Code adds intake events, and Phase 3b adds history ones.
  - `occurred_at` lets prior commits sort before the ticket's creation.
  - The ticket panel shows the timeline live (a new WebSocket stream).
    Description versions open a diff against the previous version and the
    dropped text.
  - The panel also lists attachments with their extracted text.

### Demo (Linux dev container)

The model upstream was the scripted mock with `mock-script-intake.json`
(new `text_replies`). The intake agent was the seeded `intake` (Claude Code
2.1.119, Haiku) in the real sandbox.

- Pasting a Jira ticket with `h2. [PROJEKTNAME-123] …` detected
  `PROJEKTNAME-123`. The agent's request carried no tools and reached the
  upstream with the real key, the job container was removed, and the
  review showed the original next to the enhanced ticket with
  "intake · haiku · 1520 tokens · $0.0016".
- Editing the title and creating the ticket gave KAN-1 with version 1
  (`edited`), and its timeline listed created, version 1 and the intake
  with cost.
- Dropping it again with a PDF offered "Update KAN-1". The PDF's text was
  extracted and shown, and the diff showed the new acceptance criterion.
  Updating created version 2 (`enhanced`). The timeline showed the rename,
  version 2 with its diff, and the attachment. The PDF text shows on the
  ticket.
- The New ticket dialog filled `PROJEKTNAME-123` from the title and warned
  that it exists.
- `intake.mjs` passed with no console errors or failed API calls.
- `cargo test --workspace` (extraction, ID detection, branch and commit
  helpers, headless command and output parsing, intake parsing, version and
  trigger tests), `pnpm run check`, `pnpm run lint` and the i18n checks
  pass.

### Not verified here

- **Windows.** The new crates (`pdf-extract`, `html2text`) are pure Rust
  but haven't been through the Windows CI job yet.
- **A real model.** The mock answers with fixed JSON; a real Haiku answer
  may wrap it in prose. Such answers are handled (the first JSON object is
  taken), but no real model was run.
- **OpenCode intake.** The OpenCode command and its JSON event parsing have
  unit tests, but the seeded intake agent runs on Claude Code and no
  OpenCode job ran here.

### Known leftovers

- Ticket pages are not translated (as in 0b).
- Run events on the timeline come from triggers on `execution_processes`.
  They were checked in 3b's run demo, not here (they pass there).

## Phase 3b: History check and ticket ↔ code links (2026-10-28)

**Goal:** a ticket whose id appears in the git history gets a prior-work
summary, its old commits on the timeline, and the files they changed.

### What changed

- **History check (D18, `crates/git/src/history.rs`).**
  - `git log --all` finds commits whose message mentions the ticket's
    External ID (else its internal id). Matching is case-insensitive and
    on whole tokens, so `PROJ-12` doesn't match `PROJ-123`.
  - Branches named after the id add their commits that the checked-out
    branch doesn't contain.
  - Each commit's changed files come from `--numstat`.
- **When it runs.** The check runs in the background when a ticket is
  created (board, intake), when its External ID changes, and when a run on
  it starts and ends (`local-deployment/src/history.rs`).
- **Prior work vs agent commits.** Commits by KanbAIn's agents (git author
  `KanbAIn agent`, now a shared constant tested against the Dockerfile) are
  linked to the ticket, but they are not prior work. They never prompt and
  never trigger a new analysis.
- **Permission.**
  - New prior commits show "Found N commits for X. Allow analysis?" in the
    ticket panel.
  - The answer is stored per ticket (`issue_history.permission`). The
    project setting "always allow" (from 0b) skips the prompt.
  - Allowing later, or new prior commits after an allowed analysis, starts
    the analysis again.
- **Analysis.**
  - The history agent (a headless job, like intake) gets each prior
    commit's message and patch. A patch over 48 KB, or one that doesn't fit
    the 200 KB total, is cut down to message plus file list.
  - It writes Markdown sections (done, left open, what the planner should
    know, files).
  - The summary is stored on the ticket and copied into
    `/kanbain/context/prior_work.md` for every later run on the ticket.
- **Timeline.**
  - Every commit for the id becomes a `prior_commit` entry at its commit
    time; earlier ones show "before KanbAIn".
  - There are also entries for the check ("Found N commits") and the
    analysis (agent, cost, or the error).
- **Ticket ↔ code links (E19).**
  - `ticket_commits` and `ticket_code_links` (main database) record each
    commit and the files it changed.
  - The panel's Prior work section lists the files with +/− and their
    commits.
  - `symbol` is empty until Phase 3d chunks code.

### Demo (Linux dev container)

`history.mjs` with the mock upstream (`mock-script-history.json`) and the
seeded `history` agent in the sandbox:

- A repo with commits from March 2025:
  - `PROJEKTNAME-123: send reset mails over SMTP`;
  - `Fix the reset template for projektname-123`;
  - `PROJEKTNAME-1234: unrelated`;
  - an unmerged `feature/PROJEKTNAME-123-expiry` branch.
- Creating a ticket with External ID `PROJEKTNAME-123` showed "Found 3
  commits for PROJEKTNAME-123. Allow analysis?".
- After Allow, the summary appeared with "history · haiku · 1520 tokens ·
  $0.0016".
- The ticket listed `mailer.rs`, `templates/reset.html` and `token.rs` but
  not `other.rs`.
- The timeline started with the three commits, marked "before KanbAIn",
  then the check and the analysis.
- A sandboxed run on the ticket:
  - branched `kanbain/PROJEKTNAME-123-password-reset-mail-never-arrives`;
  - read `prior_work.md`, and its `ticket.md` asked for the ID prefix;
  - committed `PROJEKTNAME-123: add reset notes`.
- After the run, that commit and `NOTES.md` were linked to the ticket,
  the prior-work count stayed 3, and no new analysis started.
- The timeline got "Run started" and "Run completed" with tokens and cost.
- No console errors or failed API calls.
- `cargo test --workspace`, `pnpm run check`, `pnpm run lint` pass.

### Not verified here

- **Large repos.** Each check runs `git log --all` plus one `git show` per
  commit (at most 200 per repo). That is fast on the demo repo but wasn't
  measured on a large monorepo.
- **Windows** paths for the git calls go through the existing `GitCli`.

### Known leftovers

- The check runs at every run start and end. It is cheap and never
  re-analyzes agent commits, but a run doesn't wait for a summary that is
  still being written (Phase 4's planner should).
- Commits that other people make for the id after the ticket exists count
  as prior work and prompt again. That is intended, but might be noisy.

## Phase 3c: Embedding store and ticket search (2026-10-28)

**Goal:** tickets and their history are searchable by meaning and by
keyword, locally, with an embedder configured by URL.

### What changed

- **Spike (E2).**
  - sqlite-vec **0.1.9** (the newest stable; 0.1.10 is an alpha) is
    compiled in and registered with `sqlite3_auto_extension` into sqlx's
    bundled SQLite (`libsqlite3-sys` 0.30). FTS5 is compiled in too.
  - A vec0 table with a `project_id` partition key and `kind`/`scope`
    metadata columns answers a filtered KNN (`kind IN (…)`) over 1,000
    vectors exactly like brute force (`filtered_knn_matches_brute_force`).
  - **Not checked on Windows yet:** that needs a pushed branch for the
    Windows CI job, which needs your approval.
- **`crates/embeddings` (E1–E7, E10 client).**
  - `embeddings.sqlite` sits next to the database (backups don't include
    it). If it can't be opened, it is deleted and rebuilt.
  - Tables follow the embedding plan's §2: spaces, sources, chunks,
    chunk_embeddings, FTS5 kept in sync by triggers, and index_jobs, plus
    `embedding_calls` for tokens and latency (E11).
  - Queries are built at runtime, since sqlx's compile-time checks can't
    see vec0.
  - **Spaces (E4):** each embedder configuration plus its probed dimension
    is one space with its own vec table.
    - A new configuration builds while search uses the active space, then
      takes over; the old one is retired and its vectors dropped.
    - Chunks and FTS are shared.
    - A dimension change behind the same model name marks the space stale
      (E7).
  - **Clients:**
    - OpenAI-compatible `/embeddings` and TEI `/embed`, with prefixes,
      batching, input cap and retries on 429/5xx.
    - The dimension probe, "Test connection" and the E9 remote-URL check.
    - TEI and Jina/Cohere reranker clients (used in 3e).
    - All tested against a mock HTTP server.
- **Indexing (E15, E16, E18, D44).**
  - Any change to a ticket, its comments or its timeline (from the existing
    SQLite commit-hook patch stream) queues the ticket, debounced.
  - One job syncs all of a ticket's text sources:
    - the ticket itself;
    - the dropped original, if it differs from the description;
    - comments;
    - attachments' extracted text;
    - the prior-work summary;
    - a summary of each finished run (agent, outcome, minutes, tokens,
      cost, task, files changed, final message; never the transcript).
  - Text is chunked at Markdown headings and paragraphs (about 400 tokens,
    40 overlap) under a `KAN-12 · PROJ-123 · Title` header.
  - Only chunks whose text changed are embedded again. Deleted tickets and
    comments leave the index.
  - All tickets are queued at start, so changes made while the app was
    closed are caught (unchanged content costs nothing).
- **Search (E5, E21).**
  - Vector KNN and FTS5 bm25 (stopwords dropped, each term a quoted phrase,
    so `PROJ-123` matches) are merged with RRF (k = 60, 50 candidates each).
  - Hits are locations with capped excerpts (D43).
  - Without an embedder, or while one is unreachable, search falls back to
    keywords and says so.
  - API:
    - `POST /api/search`;
    - `GET /api/issues/{id}/related` (tickets ranked by their best chunk);
    - `GET /api/intakes/{id}/duplicates`;
    - `GET /api/embeddings/status`;
    - `POST /api/embeddings/reindex` (optionally rebuilding vectors);
    - test and key routes for the settings.
  - The file search route moved to `/api/file-search`.
- **UI (E23).**
  - Settings → Search, in all seven locales:
    - text and code embedders and the reranker, each with Test connection
      and an encrypted API key;
    - the remote-URL warning;
    - retrieval defaults;
    - index status per space, with Re-index and Rebuild vectors.
  - The ticket panel's **Related** section, with status, "Done in KAN-n"
    and run hits.
  - **Possible duplicates** in the intake review, next to the exact
    External-ID match. Picking one turns the intake into a new version of
    it, with a diff.
  - The command bar offers "Search tickets for “…”" for whatever is typed.
    It opens a search dialog (this project or all projects) whose results
    open the ticket.

### Demo (Linux dev container)

`search.mjs`, with TEI CPU 1.7 serving `BAAI/bge-small-en-v1.5` (384
dimensions), then `BAAI/bge-base-en-v1.5` (768):

- 30 seeded tickets were indexed: 105 chunks, all embedded, in an active
  space.
- "users cannot reset their password" listed "Password reset email never
  arrives" first, ahead of two-factor authentication.
- `PROJ-123` found its ticket as keyword rank 1.
- Editing one section of a four-chunk ticket sent exactly one input to the
  embedder.
- Switching to `bge-base` created a building 768-dimension space. Search
  kept answering from the active space during the build, then the new
  space became active, the old one was retired, and the paraphrase still
  ranked first.
- Related for the reset ticket lists email change and Google login.
- In the UI:
  - the Related section;
  - the command bar's ticket search, whose result opens the ticket;
  - the near-duplicate "Forgot-password mail is not delivered" showing
    SRC-1 first under Possible duplicates, and "Update SRC-1" after
    picking it;
  - Settings → Search: "bge-base-en-v1.5 · 768 dimensions · 96 ms" and the
    active space.
- No console errors or failed API calls.
- `cargo test --workspace`, `pnpm run check`, `pnpm run lint` and clippy
  pass.

### Not verified here

- **Windows:** sqlite-vec's static link in the Windows CI build (the spike
  branch needs your OK to push).
- **Big indexes:** query latency and indexing speed past a few hundred
  chunks. sqlite-vec's KNN is brute force per partition.

### Known leftovers

- Related scores aren't comparable across queries; only the order is
  meaningful.
- Code kinds, the ticket overlay and the reranker are wired into the types
  and settings, but are used only from 3d and 3e.
- The dev container ran out of memory once while running two TEI
  containers, the IDE's rust-analyzer and leftover OpenCode servers from
  earlier sessions, all at the same time. The UI half of the demo ran
  afterwards (`UI_PROJECT=…`) against the data the API half seeded.
