# FIXING_AGENTS.md — Migrate profile-based runs to sandboxed, configurable agents

Follow-up to `TODO.md` point 10. Audit result: KanbAIn has **two classes of LLM
runs** today:

1. **Agent-library agents** (configurable in Settings → Agents, run in the Docker
   sandbox): `planner`, `coder`, `tester`, `intake`, `history`, `scribe` (scribe is
   defined but never launched). Role → agent resolution lives in
   `AgentJobs::agent_name_for` (`crates/local-deployment/src/jobs.rs:53`).
2. **Raw executor-profile runs** (model from Settings → General profile / global
   default model — *not* visible in Settings → Agents): workspace chat sessions,
   the **Resolve Conflicts** dialog, rebase-conflict flow, **PR auto-description**,
   and **review** runs. These are the "hidden agents" the user couldn't configure
   (e.g. stuck on `gpt-5.3-chat`).

**Goal:** move class 2 onto the same mechanism as class 1 — a named, editable
agent definition with a role, launched in the sandbox — so every LLM run in the
app is configurable in Settings → Agents.

## Why this is mostly plumbing

`ExecutorConfig.agent_definition` already exists and
`crates/local-deployment/src/container.rs:1766-1800` already turns **any**
`ExecutionProcessRunReason::CodingAgent` run that names an agent definition into
a sandboxed, library-driven run (role prompt, styles, skills, model, budget via
`adapter_for(...).launch(...)` + `sandbox.install_agent(...)`). Follow-ups and
review requests inherit the session's last agent definition
(`agent_definition_for`, `container.rs:532-555`). The frontend payload
(`ExecutorConfigPayload`) already has an `agent_definition` field
(`shared/types.ts:1276`), and `CreateChatBoxContainer.tsx:174-187` already shows
the pattern `{ executor: RUNTIME_EXECUTORS[agent.runtime], agent_definition }`.

## Change map

### 1. Backend — new roles + default agents

- `crates/executors/src/adapters/definition.rs:28` — add variants to
  `AgentRole`: `ConflictResolver`, `Reviewer`. Reuse `Scribe` for PR
  auto-description (it currently has no launcher).
  - New roles automatically appear in Settings → Agents via
    `GET /api/agents/meta` (`crates/server/src/routes/agents.rs:97`,
    `AgentRole::all()`).
- `crates/executors/src/adapters/mod.rs:149` (`default_agents()`) — add
  `conflict-resolver`, `reviewer` (and wire `scribe` for PR descriptions).
  Move the PR description prompt out of `DEFAULT_PR_DESCRIPTION_PROMPT` in
  `crates/server/src/routes/workspaces/pr.rs` into the scribe-style role prompt
  (keep the config override working, or deprecate it).
- Run `pnpm run generate-types` — `AgentRole` is exported via
  `crates/server/src/bin/generate_types.rs:254`; `shared/types.ts` must be
  regenerated, not hand-edited.

### 2. Backend — seeding defaults for existing installs

- `crates/services/src/services/agent_library.rs:243-256` (`init`) currently
  seeds `default_agents()` **only when the agents dir doesn't exist**. Existing
  users would never get the new default agents. Change to: on every start, write
  default agent yamls that are **missing** (never overwrite user edits).
  Track intentional deletion (e.g. a `.deleted-defaults` marker file or skip
  names recorded in a manifest) so a user who deletes `conflict-resolver` doesn't
  get it back on restart. Alternatively follow the config-version migration
  pattern (`crates/services/src/services/config/versions/v6..v8.rs`).

### 3. Backend — role → agent resolution for non-orchestrator callers

- Add `GET /api/agents/for-role?role=<role>` in `crates/server/src/routes/agents.rs`
  returning the agent name, reusing the same rule as
  `AgentJobs::agent_name_for` (prefer the agent named exactly after the role,
  else the first valid definition with that role). Extract the shared logic so
  backend and frontend can't drift.
- `crates/server/src/routes/workspaces/pr.rs:129-167` — instead of
  `ExecutorConfig::from(latest_executor_profile)`, set
  `executor_config.agent_definition = <scribe-role agent name>` (executor field
  gets overwritten by the launch anyway, `container.rs:1795-1801`).

### 4. Backend — session executor check

- `crates/server/src/routes/sessions/mod.rs:150-163` rejects a follow-up whose
  payload executor differs from the session's last executor. When an
  `agent_definition` is present, the run's real executor is the agent's runtime
  (overridden in `container.rs`), so the check must be skipped/adjusted when
  `payload.executor_config.agent_definition` is set — and dialogs that create a
  new session for a role must create it with the agent's runtime executor.

### 5. Frontend — resolve role → agent and send `agent_definition`

- New helper hook in `packages/web-core/src/shared/hooks/useAgentLibrary.ts`:
  `useAgentForRole(role)` → agent name + definition (calls the new endpoint or
  computes from `useAgents()` with the same rule).
- `packages/web-core/src/shared/dialogs/tasks/ResolveConflictsDialog.tsx`
  (lines ~120-190): replace profile-based `executor_config` with
  `{ executor: RUNTIME_EXECUTORS[agent.runtime], agent_definition: agentName }`
  for role `conflict_resolver`; create new sessions with that executor. The
  profile pickers in the dialog become the agent (keep a "custom profile"
  escape hatch or drop it — decision below).
- `packages/web-core/src/shared/dialogs/command-bar/RebaseDialog.tsx` — only
  opens `ResolveConflictsDialog`, so it inherits the change.
- `packages/web-core/src/shared/dialogs/command-bar/StartReviewDialog.tsx:112-119`
  — send `agent_definition` for role `reviewer`.
- `packages/web-core/src/features/workspace-chat/...` (`useSessionSend.ts`,
  `SessionChatBoxContainer.tsx:347`) — conflict detection in chat opens the
  same dialog; verify no other hidden follow-up paths remain (audit:
  `grep -rn "followUp\|startReview" packages/web-core/src`).
- Plain workspace chat sessions stay profile-based (that's the user's own
  interactive session, already configurable via the per-session model picker) —
  only the *system-initiated* runs (conflict, rebase, review, PR description)
  migrate.
- i18n: new labels for the agent pickers in the dialogs
  (`packages/web-core/src/i18n/locales/*/tasks.json` etc.).

### 6. Tests / checks

- `crates/executors/src/adapters/mod.rs` test `default_agents_are_valid` covers
  the new defaults automatically; add role-uniqueness assertions.
- `crates/services/src/services/agent_library.rs:1056` test hardcodes the
  default agent name list — update it.
- New tests: `for-role` endpoint resolution (named-after-role preferred, fallback
  to first-with-role, error when none); seeding migration (missing defaults
  written, existing files untouched, deleted-defaults respected).
- `crates/executors/src/adapters/cli_drift.rs` is snapshot-based on CLI flags,
  not roles — should be unaffected, but run it.
- `pnpm run generate-types:check` in CI will catch a stale `shared/types.ts`.
- `pnpm run check`, `pnpm run lint`, `cargo test --workspace`, `pnpm run format`.

## Behavior changes to accept (or decide on)

1. **Sandbox becomes mandatory for these flows.** Agent runs hard-fail without
   Docker (`container.rs:1770-1775`). Conflict resolution / rebase / review / PR
   description currently work unsandboxed. Options: (a) accept — sandbox is
   where egress allowlist + budget enforcement live; (b) add an unsandboxed
   fallback — *not recommended*, it silently drops the security model.
2. **Budgets and concurrency slots apply**: agent `limits` (budget_usd,
   max_steps, wall_minutes) and `sandbox.max_concurrent_runs` now cap these runs
   (`container.rs:430-480`, `prepare_sandbox`).
3. **Model comes from the agent definition**, not the profile — that's the point
   (fixes "stuck on gpt-5.3-chat"), but users must learn the model lives in
   Settings → Agents for these roles.

## Rollout checklist

- [ ] `AgentRole` + `default_agents()` + `generate-types`
- [ ] seeding migration for existing installs (+ deleted-defaults handling)
- [ ] `GET /api/agents/for-role` + shared resolution logic with `agent_name_for`
- [ ] `pr.rs` uses scribe-role agent
- [ ] sessions executor-mismatch check aware of `agent_definition`
- [ ] `useAgentForRole` hook + dialogs (ResolveConflicts, StartReview; Rebase inherits)
- [ ] i18n strings
- [ ] tests (roles, seeding, endpoint) + update hardcoded name-list test
- [ ] `pnpm run format`, `pnpm run check`, `pnpm run lint`, `cargo test --workspace`
- [ ] manual smoke: merge conflict → resolve runs as `conflict-resolver` agent
      in sandbox; PR opened → description written by `scribe` agent; review run;
      each visible/editable in Settings → Agents

## Open questions

- Keep the profile pickers in the Resolve Conflicts / Review dialogs as an
  override, or remove them so the agent is the single source of truth?
- Should ticket-panel merge-conflict re-runs (`conflict.md` flow in
  `orchestrator/checkpoints.rs`) also switch from `coder` to the new
  `conflict_resolver` role, or stay on `coder`? (Currently they reuse coder/tester —
  configurable already, but a dedicated role would be clearer.)
- PR description prompt: keep the `pr_auto_description_prompt` config override
  layered on top of the scribe agent, or move fully into the agent definition?
