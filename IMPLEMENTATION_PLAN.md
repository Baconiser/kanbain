# KanbAIn Implementation Plan

This plan comes from the brief in `PLAN.md` and a full Q&A session on 2026-10-26. It is self-contained: a new session should be able to implement from this file alone. Where this file and `PLAN.md` disagree, **this file wins**.

KanbAIn (from Russian *kombain*, "combine harvester", plus kanban and AI) is a **local Tauri desktop app** for Windows. In it you drop in Jira tickets, an LLM enhances them, and sandboxed AI agents plan, code, test and document the work until it is ready for your review. Nothing reaches `main` without your approval.

---

## 1. Settled decisions

### Product and platform
- **D1.** Target: a local **Tauri desktop app** on **Windows**. Linux and macOS may come later. Only the user tests on Windows.
- **D2.** "No server" means no separately installed or hosted server. The Rust axum backend keeps running **inside the Tauri process** on `127.0.0.1`, as it already does (`crates/tauri-app/src/main.rs:206`, `server::startup::start()`). The frontend is not rewritten as Tauri IPC.
- **D3.** Single user, **no login**. The local API requires a **random token generated on each launch**, which only the Tauri webview knows. All other callers are rejected: other local programs, browser pages, sandbox containers.
- **D4.** `pnpm run dev` (backend plus Vite in a browser) stays, **for development and agent testing only**. Users run the Tauri app.
- **D5.** This is a **hard fork** (the user's repo `Baconiser/kanbain`). No upstream syncing and no PR workflow for the fork itself.
- **D6.** **Full rename** from Vibe Kanban to **KanbAIn**:
  - Crate names, `@vibe/*` → `@kanbain/*`, binary and npm package `kanbain`.
  - Env vars `VK_*` → `KANBAIN_*`.
  - Data folder, UI strings, i18n, docs, logo, Tauri identifier.
  - Old Vibe Kanban data is **not** migrated.
- **D7.** **Only the LLM APIs are remote.** Delete:
  - `crates/remote` and `packages/remote-web`
  - all `relay-*` crates, `ws-bridge`, `trusted-key-auth`, `remote-info`, `embedded-ssh` (if only relay uses it), `host_relay`, `relay_auth`, `webrtc`
  - the ElectricSQL client, billing, organizations and OAuth (cloud login)
  - **Keep `crates/tauri-app`.**
- **D8.** Executors: keep **only Claude Code and OpenCode**. Delete amp, codex, copilot, cursor, droid, gemini, qwen and acp. Keep `qa_mock` if tests need it.

### Data
- **D9.** **Everything in SQLite** (`crates/db`): projects, statuses, tickets, comments, tags, relationships, attachments metadata, plans, runs, events, issues, changelog entries, skills/agents/styles index, settings.
- **D10.** **Nothing is written to project repos** by KanbAIn itself. Agents get context files **inside their sandbox only**, outside the code checkout:
  - `/kanbain/context/` is read-only: ticket, plan, prior-work summary, previous changelog entries, open issues.
  - `/kanbain/out/` is writable: `plan.json`, `issues.json`, `changelog.md`, `questions.md`, `enhanced_ticket.json`, and so on.
  - The backend reads `/kanbain/out/` back into SQLite when the run ends.
- **D11.** No automatic `CHANGELOG.md` in repos. Instead, an **"Export changelog as Markdown"** action per ticket and per project.
- **D12.** **Automatic SQLite backups**: daily, and before every migration, keeping the last N (setting). There is also a **full export** (JSON plus Markdown).
- **D13.** **Skills, agent definitions and style blocks** are managed in the dashboard (create, edit, import from file, folder or zip, export). They are stored in the KanbAIn data folder (files on disk, indexed in SQLite), global across projects, and **never in project repos**. Skills are mounted read-only into sandboxes. Grill-me is **not** part of KanbAIn. Users add whatever skills they want.

### Tickets
- **D14.** Ticket states are per-project **status rows**, not an enum. Default set: Backlog → Planning → Awaiting approval → In progress → Testing → Review → Done, plus **Blocked**. The pipeline references statuses by a stable `kind` column, never by name. The upstream code matched status names, so this is a known pitfall.
- **D15.** **Ticket intake**: you drop or paste a ticket (pasted text including Jira wiki markup; `.txt`/`.md`/`.html`/`.pdf`/Jira Word/XML export; screenshots). **No Jira API.**
  - An **intake agent** produces a structured ticket: title, clean description, acceptance criteria, open questions, External ID, priority guess.
  - The **original text stays unchanged** next to it. The user accepts or edits before the ticket enters Backlog.
  - Intake looks at text only, never code.
- **D16.** **External ID**: an optional field, auto-filled with the default regex `\[?[A-Z][A-Z0-9]*-\d+\]?` (Jira keys such as `PROJEKTNAME-123`, with or without brackets).
  - A per-project key can restrict it, for example to `PROJEKTNAME-`.
  - Every ticket also has an internal ID `<PREFIX>-<n>` (for example `KAN-42`, prefix set per project).
  - Branches and commits use the External ID when it is set, otherwise the internal ID: `kanbain/<ID>-<slug>`, and commit messages `<ID>: …`.
- **D17.** Dropping a ticket whose External ID **already exists** updates that ticket with a new **description version** (diff shown). The dialog also offers "create new".
- **D18.** **Git-history check**: when a ticket is created and when a run starts, run `git log --all` over all the project's repos, searching commit messages and branch names for the ID.
  - On hits, ask: *"Found N commits for X. Allow analysis?"*. Permission is per ticket. A project setting "always allow" skips the prompt.
  - The analysis reads commit messages plus diffs, capped at about 200 KB total; oversized diffs are reduced to file lists.
  - It produces a **"Prior work" summary** on the ticket, which goes into the planner's context.
- **D19.** Every ticket has a **timeline**:
  - Created/updated (with description versions), intake enhancement, history analysis.
  - State changes, plan versions and approvals, questions and answers.
  - Every run (agent, duration, tokens, cost), issues found and fixed, commits, merge.
  - Prior git commits for the ID appear as entries from before KanbAIn.

### Pipeline
- **D20.** **Planner**: reads the ticket, the prior-work summary and the code, then writes a structured **`plan.json`**: subtasks with likely files, acceptance criteria, assumptions, test strategy and checkpoints.
  - It can post **questions** to the ticket thread. The ticket shows that it is waiting for an answer, and your reply **resumes the same agent session** (`claude --resume`, or the OpenCode equivalent).
  - The ticket-level **ask/yolo** flag goes into the task prompt. How the planner questions you is defined only by its role prompt and skills.
- **D21.** **Approval gate**: the plan is shown as an editable form. You approve, edit (which creates a new version) or send it back with feedback (also a new version). **No coding before approval.**
- **D22.** **Orchestrator is deterministic Rust code**, not an LLM. It:
  - schedules subtasks, running those with disjoint file sets in parallel and overlapping ones in sequence;
  - spawns one coder per subtask, each on its own branch and in its own container;
  - merges subtask branches into the ticket branch.
  - On a **merge conflict**, it re-runs that subtask's coder on top of the updated ticket branch. That counts as a fix round.
- **D23.** **Tester**: runs at the plan's checkpoints and writes **black-box tests from the ticket's requirements and acceptance criteria**, not from the coder's implementation.
  - It must not modify the coder's code (enforced: only new or changed test files under the test paths are accepted). Its tests are a separate commit on the ticket branch.
  - It reports structured issues in `issues.json`: `file, line, severity (low|medium|high), category (edge case|integration|logic|error handling|state/timing|requirements), description`.
- **D24.** **Fix loop**: issues go back to the coders. **At most 3 rounds**; after that the ticket goes to Blocked and you get a notification.
- **D25.** **Scribe**: writes a changelog entry per run (what, why, files, issues found and fixed, commit refs) into `/kanbain/out/changelog.md`, which is stored in SQLite.
- **D26.** **Review and merge**: you see the summary, the diff, issues, the changelog and the cost. "Merge" does a **local git merge into main** of the project repo, with an optional push afterwards. The existing GitHub PR feature stays as an option, but the pipeline doesn't depend on it.
- **D27.** The existing **manual workspace** flow (you chatting with an agent in a workspace) stays, and is sandboxed too.
- **D28.** Defaults (all editable):
  - Models: planner **Opus**, coder and tester **Sonnet**, scribe and intake **Haiku** (or Sonnet).
  - Budgets: **$5 per ticket** (on overrun: Blocked) and **$2 per run**.
  - **30 min of agent activity** per run.
  - **At most 3 agent containers at once**; the rest are queued.
- **D29.** Notifications: in-app plus **native OS notifications through Tauri** (`crates/tauri-app/src/windows_notifications.rs` exists) for Blocked, Awaiting approval, "planner has questions", and Review ready.

### Agents, runtimes, auth
- **D30.** Runtimes: **Claude Code and OpenCode**. The runtime-neutral layer comes first, then the Claude Code adapter end-to-end, then the OpenCode adapter right after.
- **D31.** Neutral agent definition:
  ```yaml
  name: backend-coder
  role: coder            # planner | coder | tester | scribe | intake | history
  runtime: claude-code   # or opencode
  model: sonnet          # adapter maps per runtime/provider
  role_prompt: "You are a backend developer..."
  styles: [concise-commits, project-conventions]
  skills: [some-skill]
  capabilities: [read, edit, bash, test]
  limits: { max_steps: 40, budget_usd: 2, wall_minutes: 30 }
  ```
  Three prompt layers:
  - **role**: the system prompt;
  - **styles**: reusable blocks appended to the system prompt; editing one updates every agent using it;
  - **task**: generated per ticket or subtask and sent as the first message, never baked into the system prompt.
- **D32.** Adapter facts, as verified 2026-10-26. Re-check against the pinned CLI version before implementing.
  - **Claude Code**
    - `--agents` takes JSON (`prompt`, `description`, `tools`, `disallowedTools`, `model`, `permissionMode`, `mcpServers`, `hooks`, `maxTurns`, `skills`, `effort`, …).
    - Other flags: `--max-turns`, `--max-budget-usd` (a client-side estimate, so the proxy stays the hard limit), and `ANTHROPIC_BASE_URL`, which is the server root without `/v1`.
    - Limited bash is done with `Bash` plus a `PreToolUse` hook.
    - Read-only means `tools: Read, Grep, Glob`.
  - **OpenCode**
    - Agents live in `.opencode/agents/<name>.md`; the frontmatter holds `description, mode, model (provider/model-id), temperature, steps, permission`.
    - `maxSteps` and the `tools` booleans are **deprecated**; use `steps` and `permission` (`edit`, `bash` with patterns, `webfetch`, `skill`, …).
    - The gateway is set with `provider.<id>.options.baseURL` in `opencode.json` (no env var). Config can also be injected through `OPENCODE_CONFIG_CONTENT`, which the executor already uses.
    - Check whether the pinned version uses the v1 or the v2 permission format.
- **D33.** **Auth**: either an **API key** or a **Claude login**.
  - For login, the dashboard runs `claude setup-token` in a throwaway container and you complete the browser step. The token is stored **encrypted** in the data folder.
  - The **proxy injects** the real credential. Containers only get a dummy credential plus a per-run proxy token.
  - Subscription runs show an **API-equivalent cost estimate**, and budgets are enforced on that estimate.
  - OpenCode can use any provider through the proxy.
- **D34.** Every LLM-using job, **including intake and history analysis**, runs as a **headless Claude Code or OpenCode run in a container**. Intake and history runs have no repo and no tools. This is what lets subscription login work everywhere and keeps logging uniform.

### Sandbox and proxy
- **D35.** Each run gets its **own Docker container**. Docker Desktop with WSL2 is required on Windows; the app checks for it at startup and shows a setup screen if it's missing. The container sees **no** host home folder, SSH keys or Docker socket. CPU and memory limits apply.
- **D36.** Code access: a **fresh clone of the ticket or subtask branch inside the container**, made from the host repo through a host-side git transfer (git bundle or a short-lived `git daemon`/`upload-pack` on the proxy network). After the run (or at each commit), the host **fetches the agent's branch back**. The host-side worktree remains your review copy.
- **D37.** **Container lifetime**: an automated run's container is deleted when the run ends. A manual workspace's container lives until the workspace is closed. The workspace **terminal** (`docker exec`) and **dev server** run inside it, and the preview is routed through `crates/preview-proxy`. The wall-time limit counts agent activity, not container uptime.
- **D38.** **Network**: containers reach only an egress allowlist:
  - the KanbAIn model proxy (always);
  - package registries by default (npm, crates.io/static.crates.io, PyPI, Go proxy, Docker Hub, GitHub release downloads);
  - extra domains per project.
  Everything else is blocked and logged. Implementation: an internal Docker network plus an egress HTTP(S) proxy (CONNECT allowlist) built into the backend.
- **D39.** **Images**: one **KanbAIn base image** with Claude Code and OpenCode at **pinned versions**, git, Node, Rust, Python and common build tools. Projects may provide a Dockerfile `FROM kanbain-agent:<ver>`.
- **D40.** The **model proxy is built into the backend** (Rust, separate port, reachable only from the sandbox network).
  - It speaks **Anthropic Messages** and **OpenAI-compatible** APIs, and streams responses through.
  - Each run gets a short-lived token, which maps every call to run → ticket → budget.
  - It logs every call (model, tokens in/out/cache, cost, latency) as Events.
  - Revoking the token kills a runaway run instantly.
- **D41.** A run is the existing **`execution_process`, extended**: ticket/subtask, agent definition, container id, branch, tokens, cost, status. Events are stored alongside the existing process logs. No parallel `runs` table.

---

## 2. Codebase facts (verified 2026-10-26)

- **Toolchain**: `rust-toolchain.toml` pins **`nightly-2025-12-04`** (README and PLAN.md wrongly say "stable"). Node ≥20, pnpm 10.13.1 (`packageManager`).
- **Tickets today live only in the remote server**:
  - Postgres schema in `crates/remote/migrations/20260112000000_remote-projects.sql`: `projects`, `project_statuses` (L30), `issues` (L53, with `parent_issue_id` for subtasks at L78), `issue_assignees`, `issue_followers`, `issue_relationships`, `tags`, `issue_tags`, `issue_comments` (L172), `issue_comment_reactions`, `notifications`, `workspaces` (L242).
  - Default statuses are in `crates/remote/src/db/project_statuses.rs:11-18`.
  - Name-based status logic is in `crates/remote/src/db/issues.rs` around L602-655.
- **Local backend issue routes are proxies**: `crates/server/src/routes/remote/*.rs` call `deployment.remote_client()?`. They are disabled unless `VK_SHARED_API_BASE` is set (`crates/local-deployment/src/lib.rs:171-201`).
- **Frontend board** reads remote data through ElectricSQL:
  - `packages/web-core/src/shared/providers/remote/ProjectProvider.tsx:41-78` (`useShape(PROJECT_ISSUES_SHAPE …)`);
  - `packages/web-core/src/shared/lib/electric/collections.ts` (shape URLs at L348, mutations at L633+).
- **Local SQLite** (`crates/db`) has a legacy `tasks` table (`crates/db/src/models/task.rs`, fixed `TaskStatus` enum) that is unused outside `crates/db`. `workspaces.task_id` no longer has an FK.
- **Workspaces**:
  - Models: `crates/db/src/models/workspace.rs:42`, `session.rs:23`, `execution_process.rs:62` (`RunReason`: SetupScript/CleanupScript/ArchiveScript/CodingAgent/DevServer).
  - Creation: `crates/server/src/routes/workspaces/create.rs:212`, then `container().start_workspace` (`crates/services/src/services/container.rs:1047`).
  - Worktree: `crates/worktree-manager/src/worktree_manager.rs:60`.
  - The workspace↔issue link currently exists only remotely (`routes/workspaces/links.rs:24`).
- **Agent spawn site**: `LocalContainerService::start_execution_inner` (`crates/local-deployment/src/container.rs:1316`) calls `executor_action.spawn(&current_dir, approvals, &env)` (L1372). **"Container" in this codebase means the worktree path; there is no Docker abstraction yet.**
- **Claude executor** (`crates/executors/src/executors/claude.rs`):
  - `npx -y @anthropic-ai/claude-code@2.1.119` (L61-65);
  - flags in `build_command_builder` (L149), spawn in `spawn_internal` (L620);
  - plan mode and `ExitPlanMode` approvals at L221-231 and L1141;
  - token usage from `MessageDelta.usage` (about L1772), emitted at L2058.
- **OpenCode executor** (`opencode.rs`): `npx -y opencode-ai@1.4.7 serve --hostname 127.0.0.1 --port 0` (L91/105); sets `OPENCODE_PERMISSION` and `OPENCODE_CONFIG_CONTENT` (L779-805).
- **Executor env/config**:
  - `ExecutionEnv` in `crates/executors/src/env.rs:79`;
  - `CmdOverrides` in `command.rs:44`;
  - profiles in `crates/executors/default_profiles.json` plus the user `profiles.json` (`profile.rs:59/278`);
  - MCP config in `mcp_config.rs` and `default_mcp.json` (currently writes the global `~/.claude.json`, which has to change to per-container).
- **Approvals**: `crates/services/src/services/approvals.rs` (in-memory), routes in `routes/approvals.rs`, UI in `packages/web-core/src/shared/hooks/ApprovalForm.tsx`. These are per tool call; there is no ticket-level gate.
- **Live updates**:
  - SQLite update hooks (`crates/services/src/services/events.rs:77,119`) produce JSON-Patch streams (`events/patches.rs`, `events/streams.rs`);
  - SSE at `/events`;
  - WebSockets for workspaces, execution processes, logs and approvals.
- **Cost tracking**: none. There is only `TokenUsageInfo{total_tokens, model_context_window}` (`crates/executors/src/logs/mod.rs:109`), and it isn't persisted.
- **Types**: Rust → TS through ts-rs. Regenerate with `pnpm run generate-types`; never edit `shared/types.ts` by hand. SQLx offline data: `pnpm run prepare-db`.

---

## 3. Phases

Rules for every phase:
- The app stays usable.
- Small, reviewable commits.
- The phase ends with a **demo** and a **changelog entry**. Until Phase 7 exists, the changelog entry goes in `docs/kanbain-changelog.md`; this is the fork's own dev log, not a project-repo artifact.
- Verify on Linux first: `cargo test --workspace`, `pnpm run check`, `pnpm run lint`, and a headless-browser demo against `pnpm run dev`.
- Then run the Windows CI job (Phase 0a sets it up) for the user's Windows check.
- **Never push or merge to `main` without the user's approval.**

### Phase 0a: Rename and delete
**Goal:** a KanbAIn-branded app with only the local feature set, which builds and runs.

1. **Environment** (this Linux dev container is missing tools): install rustup (the pinned nightly comes from `rust-toolchain.toml`), pnpm 10.13.1 (corepack), `cargo-watch`, Tauri Linux deps (webkit2gtk etc.), Playwright/Chromium. Build and run **unmodified** first, to get a baseline.
2. **Write the run skill**: `.claude/skills/run-kanbain/SKILL.md` and a driver (from the paused `/run-skill-generator` task). Do this after the baseline works.
3. **Delete cloud code**:
   - Crates: `crates/remote`, `relay-client`, `relay-control`, `relay-hosts`, `relay-protocol`, `relay-tunnel`, `relay-tunnel-core`, `relay-types`, `relay-webrtc`, `relay-ws`, `ws-bridge`, `trusted-key-auth`, `remote-info`. Check `embedded-ssh`, `desktop-bridge` and `client-info` for remaining users first.
   - Server routes: `host_relay`, `relay_auth`, `webrtc`, `oauth`, `organizations`.
   - `services/remote_client.rs`, `remote_sync.rs`, `auth.rs`/`oauth_credentials.rs` (cloud login), billing and analytics if it reports to the cloud.
   - Frontend: `packages/remote-web`.
   - CI: the `remote-*` and `relay-*` workflows.
   - **Keep** `crates/tauri-app` and `crates/preview-proxy`.
   - **Important:** the board still depends on `routes/remote/*` and Electric until 0b. To keep the app usable, **stub** them (an empty board with a "local tickets coming in 0b" notice), or swap 0a and 0b for the board code.
4. **Delete executors** other than Claude Code and OpenCode (`crates/executors/src/executors/*`, `default_profiles.json`, frontend executor lists and icons, i18n keys). Run `node scripts/check-unused-i18n-keys.mjs`.
5. **Rename** as one mechanical commit:
   - Crates/packages (`vibe-kanban*` → `kanbain*`, `@vibe/*` → `@kanbain/*`), the binary, `npx-cli`, env vars `VK_*` → `KANBAIN_*`, the data-folder name (see `crates/utils` asset/dir helpers), Tauri `productName`/`identifier`.
   - UI strings, i18n, README, logo/assets.
   - Fix `README.md` (toolchain is nightly, not stable).
6. **Windows CI**: a GitHub Actions `windows-latest` job that builds the Tauri installer (`pnpm tauri:build`) and uploads it as an artifact. It only runs once the user pushes.

**Demo:** the app starts as "KanbAIn", only Claude Code and OpenCode can be selected, a manual workspace with Claude Code still works, and `cargo test`/`pnpm check` pass.

### Phase 0b: Local tickets in SQLite
**Goal:** the kanban board works fully offline on SQLite.

1. **Migrations** in `crates/db/migrations/`, ported from the remote schema minus multi-user tables:
   - `projects`: `name`, `id_prefix`, `external_id_pattern`, `external_id_key`, `settings` JSON.
   - `project_repos`: project ↔ local repo (1:N).
   - `project_statuses`: `name, color, sort_order, hidden, kind`, where `kind` ∈ `backlog|planning|awaiting_approval|in_progress|testing|review|done|blocked|custom`.
   - `issues`: `number` (per project), `title`, `description`, `status_id`, `priority`, `parent_issue_id`, `parent_issue_sort_order`, `external_id`, `mode` (ask|yolo), `fix_round`, `budget_usd`, timestamps.
   - `tags`, `issue_tags`, `issue_relationships`, `issue_comments` (author = `user|agent:<run>`), `attachments` (local files in the data folder).
   - Drop: assignees, followers, reactions, organizations, notifications-as-remote.
   - Drop the legacy `tasks` table and `TaskStatus`.
2. **Models** in `crates/db/src/models/` (`project.rs`, `issue.rs`, `project_status.rs`, `issue_comment.rs`, `tag.rs`, …) with ts-rs derives. Default statuses are seeded per project with their `kind`.
3. **Routes**: replace `crates/server/src/routes/remote/*` with local CRUD routes (`routes/projects.rs`, `routes/issues.rs`, …). Keep request/response shapes close to the old ones so the frontend changes stay small.
4. **Live updates**: add `projects`/`issues`/`statuses`/`comments` to the SQLite update-hook JSON-Patch streams (`services/events.rs`, `events/streams.rs`) and expose them over WS/SSE.
5. **Frontend**: swap the data layer. Replace the `useShape(...)` calls in `providers/remote/ProjectProvider.tsx` and the `lib/electric/collections.ts` mutations with hooks over the local stream plus REST mutations. Remove `@electric-sql/*` dependencies. Move providers out of `remote/`.
6. **Workspace↔issue link**: a local `workspaces.issue_id` FK (replaces `routes/workspaces/links.rs` remote call).
7. **Attachments**: local storage in `<data>/attachments/` (replaces the Azurite/remote path; `useAzureAttachments.ts` → local).
8. **Backups (D12)**: daily backup plus pre-migration backup of the SQLite file, keeping the last N; an export endpoint.
9. **Local API launch token (D3)**: the backend generates a token at start, Tauri hands it to the webview, every route checks it, and dev mode reads it from an env/file.

**Demo:** create a project, add statuses and tickets with subtasks, tags and comments, drag on the board with live updates across two windows, start a manual workspace from a ticket, restart the app and everything is still there.

### Phase 1: Docker sandbox and model proxy
**Goal:** one agent (Claude Code, API key) works on one ticket inside a container, every model call is logged and budgeted, and nothing on the host is reachable.

1. **New crate `crates/sandbox`**:
   - A `Sandbox` trait (`create`, `exec`, `stream_logs`, `copy_in/out`, `stop`, `remove`) and a Docker implementation using `bollard`.
   - Limits: CPU, memory, pids, wall/activity timer. Labels link containers to runs.
   - A startup check that Docker is available and its version is supported; a setup screen in the UI if not.
2. **Base image** `docker/agent/Dockerfile`: pinned Claude Code and OpenCode versions (taken from `claude.rs`/`opencode.rs` constants, which become **one** source of truth), git, Node, Rust, Python. Built locally on first use and tagged `kanbain-agent:<ver>`. A per-project Dockerfile override is a project setting.
3. **Network (D38)**:
   - An internal Docker network `kanbain-sandbox` with no default route.
   - An egress proxy (CONNECT allowlist) inside the backend at `crates/sandbox/src/egress.rs`; containers get `HTTPS_PROXY`.
   - Model proxy and egress proxy are the only reachable endpoints. Blocked requests are logged as Events.
4. **Code transfer (D36)**:
   - In: `git bundle` (or `git upload-pack` over the internal network) of the ticket branch, cloned to `/workspace` in the container.
   - Out: the host fetches the agent branch back through a bundle created in the container (`git bundle create`), on a commit hook or at run end.
   - The host worktree stays for review and diffs.
5. **Context dirs (D10)**: `/kanbain/context` (ro) and `/kanbain/out` (rw) are mounted as an anonymous volume and harvested at run end.
6. **Model proxy, new crate `crates/model-proxy`**:
   - An axum server on its own port, bound to the sandbox network gateway.
   - Anthropic Messages pass-through with SSE streaming, plus OpenAI-compatible `/v1/chat/completions` and `/v1/responses`.
   - Per-run tokens (create, revoke). Credentials are injected from encrypted storage.
   - Usage is parsed from responses (including cache tokens). Cost comes from a price table (`model-proxy/prices.json`, editable).
   - Budget check before and during a stream; at the limit, the stream is cut and the token revoked.
7. **Executor integration**:
   - In `crates/local-deployment/src/container.rs::start_execution_inner`, route the spawn through the sandbox: `docker exec` of the built command instead of `tokio::process::Command` on the host.
   - Set `ANTHROPIC_BASE_URL=http://<proxy>:<port>` and `ANTHROPIC_AUTH_TOKEN=<run token>`, and never the real key.
   - Stream stdout/stderr back into the existing log pipeline so the normalized logs, approvals and UI keep working.
   - The workspace `terminal.rs` uses `docker exec -it`; dev servers run in the container and go through `preview-proxy`.
8. **Run tracking (D41)**: extend `execution_process` (migration) with `issue_id`, `subtask_id`, `agent_def_id`, `container_id`, `branch`, `tokens_in/out/cache`, `cost_usd`, `budget_usd`. Add an `execution_process_events` table (`ts`, `type` = model_call|tool_call|egress_block|…, JSON payload).
9. **Concurrency (D28)**: a global queue with at most 3 running containers (setting).
10. **Cleanup**: remove the container at run end for automated runs, and when the workspace closes for manual ones. A janitor removes orphaned containers by label at startup.

**Demo:**
- A Claude Code run on a ticket in a container makes a commit that appears on the host branch.
- Every model call shows tokens and cost.
- `curl https://example.com` from the agent is blocked and logged.
- A $0.05 budget cuts the run off.
- `docker inspect` shows no host mounts besides the run volumes.

### Phase 2: Agent profiles, styles, skills, both adapters
**Goal:** neutral agent definitions drive both runtimes.

1. **Storage (D13)**:
   - `<data>/agents/*.yaml`, `<data>/styles/*.md`, `<data>/skills/<name>/SKILL.md` (plus supporting files).
   - SQLite index tables `agent_definitions`, `style_blocks`, `skills`, with a file watcher to resync.
   - Import/export as file, folder or zip.
   - Seed default agents: `planner`, `coder`, `tester`, `scribe`, `intake`, `history`, with the D28 models.
2. **New crate or module `crates/executors/src/adapters/`**: a `RuntimeAdapter` trait that takes (agent def, styles, skills, task, run ctx) and returns (command, env, files to write into the container).
   - **Claude Code adapter**:
     - `--agents` JSON or `--append-system-prompt` for role plus styles;
     - `--model` mapping, `--max-turns`, `--max-budget-usd` (as a second safety net);
     - `tools`/`disallowedTools` from capabilities, and a `PreToolUse` bash-filter hook script copied into the container;
     - skills mounted read-only to `~/.claude/skills/` in the container;
     - per-container MCP config, no longer the host `~/.claude.json`.
   - **OpenCode adapter**: `OPENCODE_CONFIG_CONTENT` with `provider.<id>.options.baseURL` = proxy, an agent markdown with `steps` and `permission`, skills mounted into OpenCode's skills dir, model `provider/model-id`.
   - Verify the field names against the pinned CLI versions (`claude --help`, `opencode` docs) and pin a test that fails on drift.
3. **Login mode (D33)**: a "Connect Claude account" button runs `claude setup-token` in a throwaway container and shows the URL/code in the UI. The token is stored encrypted (OS keychain through Tauri if available, otherwise a file encrypted with a locally generated key) and injected by the proxy. Other providers: an API-key form per provider.
4. **Agents UI**: list, create, edit and test agents (a "test run" against a scratch repo), plus a styles editor and a skills manager (import, edit `SKILL.md`, delete). Replace the old executor profile UI; `default_profiles.json` goes away or becomes internal.

**Demo:** the same ticket runs with `backend-coder` on Claude Code, then on OpenCode (Anthropic through the proxy, and one OpenAI-compatible model). Editing a style block changes both runs' system prompts. An imported skill is available in the container.

### Phase 3: Ticket intake, External ID, history, timeline
1. **Intake (D15)**:
   - A "Drop ticket" dialog with paste, file drop and images; attachments are stored locally.
   - Text extraction for PDF/HTML/DOCX/XML happens in the backend (pure-Rust crates where possible).
   - The intake agent runs in a **no-repo, no-tools** container (D34) and writes `enhanced_ticket.json`.
   - A review screen shows the original next to the enhanced version, editable, with "Create ticket".
   - Store `issue_descriptions` versions (`original`, `enhanced`, `edited`).
2. **External ID (D16, D17)**: regex auto-fill with a per-project key; a lookup on create, and if the ID exists, "update existing (new version) / create new". The branch-name and commit-prefix helper lives in `crates/git` or `services`.
3. **History check (D18)**:
   - `git log --all --grep=<ID>` plus a branch-name scan over the project repos.
   - A UI prompt, with a per-ticket permission record and a project "always allow" setting.
   - The history agent (no tools) gets the capped commits and diffs in `/kanbain/context` and writes `prior_work.md`, which is stored on the ticket.
4. **Timeline (D19)**: an `issue_events` table (or a view over existing tables plus events) and a `Timeline` component on the ticket detail page, live-updated.

**Demo:** paste a Jira ticket containing `[PROJEKTNAME-123]`. It is enhanced, its ID is detected, and commits for it are found; after "Allow" a prior-work summary appears. Dropping it again creates version 2. The timeline shows all of it.

### Phase 4: Planning flow and approval gate
1. Assigning a ticket to the AI (or dragging it to Planning) starts a **planner run** with ticket, prior work and the ask/yolo flag in the task message.
2. **Questions (D20)**: the planner writes `questions.md`, which becomes an agent comment on the ticket; the ticket gets a "waiting for answer" marker and you get a notification. Your reply comment starts a resumed run (`--resume <session>`; OpenCode session continue). Session ids are stored on the run.
3. **`plan.json` schema**, defined in Rust with ts-rs and validated on harvest:
   `{subtasks:[{id,title,description,files[],depends_on[]}], acceptance_criteria[], assumptions[], test_strategy, checkpoints:[{after_subtasks[], what}]}`.
   Tables: `plans` (`issue_id`, `version`, JSON, `status` = draft|approved|rejected, `feedback`).
4. **Approval UI (D21)**: a plan form editor; Approve, Save as new version, or Send back with feedback (starts a planner run with the feedback). The ticket moves to Awaiting approval, then In progress on approve.

**Demo:** in ask mode the planner asks questions in the thread, you answer, and it produces a plan. Editing it creates v2. Send back produces v3. Approving moves the ticket to In progress. In yolo mode the plan lists its assumptions.

### Phase 5: Orchestrator
1. **New module `crates/services/src/services/orchestrator/`**: a state machine per ticket.
   - Subtasks become sub-issues (`parent_issue_id`). Build a DAG from `depends_on` plus file overlap; disjoint file sets run in parallel (respecting the global limit of 3), overlapping ones in sequence.
   - Branches: ticket branch `kanbain/<ID>-<slug>`, subtask branches `…/<n>`.
2. Each coder run gets its subtask plus the plan in `/kanbain/context`. When it finishes, the branch is fetched back and merged into the ticket branch on the host (`crates/git`).
3. **Conflict (D22)**: re-run that subtask's coder on the updated ticket branch with the conflict info, incrementing `fix_round`.
4. Resumable after an app restart (state lives in SQLite). There are Stop and Retry controls per subtask and per ticket.

**Demo:** a 3-subtask plan where two subtasks touch disjoint files runs them in parallel and the third after them. The ticket branch contains all three merges. A forced conflict triggers a re-run.

### Phase 6: Tester and fix loop
1. At each checkpoint, a **tester run** works on the ticket branch with the acceptance criteria and requirements in its context, and writes tests plus `issues.json`.
2. **Enforcement (D23)**: after the run, reject the tester's changes outside the test paths. Test paths come from a project setting with sensible defaults per language (`**/tests/**`, `**/*.test.*`, `**/*_test.*`, …). Tester tests are a separate commit.
3. Issues are stored in an `issues_found` table (`issue_id`, `run_id`, `file`, `line`, `severity`, `category`, `description`, `status` = open|fixed|wontfix). Open issues go to coder fix runs. After 3 rounds the ticket goes to Blocked and you get a notification (D24). A ticket over budget also goes to Blocked.

**Demo:** a seeded bug is found by the tester, fixed by the coder in round 1, and the ticket moves to Review. An unfixable issue goes to Blocked after round 3, with a notification.

### Phase 7: Scribe and changelogs
1. A scribe run after each coder/tester run (Haiku, no edit tools) writes `changelog.md`, stored in `changelog_entries` (`issue_id`, `run_id`, `summary`, `files[]`, `commit_refs[]`, `issues_fixed[]`).
2. A **review screen (D26)**: summary, full ticket diff (existing diff view), issues found/fixed, changelog, cost. Merge does a local `git merge` into main (with conflict handling and an optional push); Request changes sends you back to coders or the planner.
3. **Export (D11)**: Markdown per ticket and per project.

**Demo:** a ticket from Review to Done through Merge; main contains the work; the ticket and project changelogs export as Markdown.

### Phase 8: Stats and charts
1. **Projects overview**: open tickets, active agents, open issues and cost per project, live.
2. **Issues and stats views**: issues by category, file, severity and over time, and cost per ticket/run/agent/model (from `execution_process` and the proxy events). Use the existing UI kit; pick one chart library.

**Demo:** after several tickets, the charts reflect the stored data and update live when a run finishes.

---

## 4. Risks and mitigations
- **Phase 0b is the largest change** (replacing the Electric data layer). Keep API shapes close to the remote ones, do it one entity at a time, and keep the board working after each step.
- **Docker Desktop on Windows**: WSL2 memory limits, slow file IO, and differences in how `host.docker.internal` and networks behave. The design uses git transfer instead of bind mounts, and I test the network rules on Linux. Windows network behaviour must be checked by the user early, at the end of Phase 1.
- **CLI drift**: keep CLI versions pinned in one place, test the adapter output against `--help`/schema, and bump versions deliberately.
- **Subscription login through the proxy**: depends on Claude Code sending the OAuth capability header through the gateway. Verify in Phase 2; the API key path is the fallback. Before offering KanbAIn to others with claude.ai login, check Anthropic's terms.
- **Cost estimates**: the client-side `--max-budget-usd` can overshoot, so the proxy is the hard limit. Keep the price table editable.
- **Tests that check nothing**: the tester is black-box from the acceptance criteria, and tester and coder are separate runs.

## 5. Open items for later (not decided, not in scope)
- macOS/Linux packaging.
- A Jira API import source.
- gVisor/Firecracker instead of Docker.
- External notification channels.
