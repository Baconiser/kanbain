# KanbAIn Self-Improvement Plan

Follow-up to `TODO.md` point 15. KanbAIn should learn from its own runs: read what agents did across a whole ticket, find where they wasted turns or lacked context, and turn that into concrete changes that make the next ticket cheaper and more reliable. Goals, in order:
- **Fewer wasted turns**: denied commands, blocked hosts, re-reads, tester ↔ fixer loops and retries go down over time, per project.
- **Less asking**: planner questions whose answers never change become project knowledge instead of questions.
- **Never silently worse**: every change is reviewed by the user, versioned, measured and can be rolled back.

Depends on `TODO.md` points 13 and 14 (sandbox setup, `setup.sh`, `environment.md`), which are the main things this feature writes to. Written 2026-11-03; nothing here is implemented yet. Decisions are numbered L1–L20.

---

## 1. Settled decisions

### What is learned from
- **L1. Three layers, cheapest first.**
  1. **Signals**: deterministic extraction from data KanbAIn already stores. No LLM.
  2. **Retro**: an LLM agent reviews a finished ticket, using the signals plus compressed transcripts, and writes structured proposals.
  3. **Apply**: proposals become versioned changes to project context, settings or agents, only after user approval.
  Layer 1 is useful on its own (a "what went wrong" panel) and ships first.
- **L2. Sources, all already in the database or on disk:**
  - `execution_process_events`: `model_call` (tokens, cost, latency), `tool_call` (`search_knowledge` with result counts), `egress_block`, `budget_exceeded`, `sandbox`.
  - `execution_process_logs`: the full JSONL transcript per run.
  - `orchestration_subtasks` / `ticket_orchestrations`: failed, stopped and retried subtasks, merge conflicts.
  - `orchestration_checkpoints` / `issues_found`: tester → fixer loop count and the issues behind it.
  - Planner questions (`IssueComment` from `harvest()`) and the user's answers (`on_comment`).
  - Manual interventions: subtasks the user moved or fixed by hand (TODO 2), plan edits before approval, chat messages that correct an agent.
- **L3. Record what is currently invisible.** Two events are added so signals don't depend on parsing transcripts:
  - `command_denied`: `bash_filter.sh` appends `{command, reason}` to `/kanbain/out/denials.jsonl`, and the harvest step turns each line into an event. The OpenCode adapter's permission denials are recorded the same way.
  - `file_read`: path and byte count per Read tool call, from the normalized log entries, so re-reads and "explored the whole repo" are countable.

### Layer 1: signals
- **L4. Signals are computed per run, then rolled up per ticket and per project.** A signal is `{kind, key, count, run_ids[], sample}`. `key` groups repeats, e.g. the denied command pattern or the blocked host. First set:

  | Signal | Key | Meaning |
  |---|---|---|
  | `command_denied` | normalized command (`rustup component add *`) | Missing setup, or the allowlist is too strict |
  | `egress_blocked` | host | Missing allowed domain, or the agent is doing something it shouldn't |
  | `reread` | path | Same file read ≥3 times in one run: context too thin, or the run was too long |
  | `explore_heavy` | role | Many Read/Grep/ls calls before the first edit: missing project context |
  | `search_empty` | query | `search_knowledge` returned nothing: index gap |
  | `fix_loop` | checkpoint | Tester → fixer loop count above 1 |
  | `retry` / `manual_fix` | subtask | The subtask needed a retry or a hand fix |
  | `repeat_answer` | normalized question | The planner asked the same thing on ≥2 tickets and got the same answer |
  | `budget_hit` | role | The run stopped on budget |
  | `cost_outlier` | role | Run cost above 2× that role's project median |

- **L5. Stored as a cache, not a source of truth.** Signals live in `run_signals`, keyed by `execution_process_id` and computed when the run finishes. Re-running the extractor after a parser change rebuilds them. Old runs are backfilled lazily, when a project's insights panel is first opened.
- **L6. UI: an Insights tab on the ticket and on the project.** The ticket tab lists that ticket's signals with links to the runs. The project tab shows recurring signals ranked by count × cost. This is the deliverable of Phase A and needs no LLM.

### Layer 2: the retro agent
- **L7. A new `retro` role** (`AgentRole::Retro`), seeded as default agent `retro` (sonnet, capabilities `[Read, Search]`, no edit, no bash, no network). It is configurable in Settings → Agents like every other role.
- **L8. Triggers:**
  - a main ticket reaches Done, or is Blocked with failed subtasks;
  - a manual **"Run retro"** button on the ticket;
  - a project-level retro over the last N tickets (default 10), from the project Insights tab. This is the one that finds cross-ticket patterns.

  An auto retro after each ticket is a project setting, off by default, because it costs tokens.
- **L9. Input is compressed, never raw transcripts.** KanbAIn builds `/kanbain/context/retro/`:
  - `signals.json`: the ticket's L4 signals.
  - `runs/<role>-<n>.md`, one per run: task, outcome, cost, and the transcript reduced to tool calls with one-line arguments, errors and denials kept in full, assistant text truncated, file contents dropped. Target ≤ 8k tokens per run; longer runs keep their first and last parts plus every error.
  - `ticket.md`, the approved plan, planner Q&A, `issues_found`, and the user's chat corrections.
  - `current.md`: the project's current lessons, `environment.md`, `setup.sh`, allowed domains, and each involved agent's `bash_allow` and role prompt, so the retro proposes diffs against what exists rather than duplicates.
- **L10. Output is structured.** The retro writes `/kanbain/out/proposals.json`, an array of:
  ```json
  {
    "target": "project_lesson | environment | setup_script | allowed_domain | bash_allow | test_paths | role_lesson | role_prompt",
    "scope": { "project_id": "…", "role": "tester" },
    "change": { "op": "add | replace | remove", "path": "…", "value": "…" },
    "rationale": "one or two sentences",
    "evidence": [{ "run_id": "…", "signal": "command_denied", "key": "rustup component add *" }],
    "confidence": 0.0
  }
  ```
  Proposals without evidence that points at a real run or signal are dropped on harvest.

### Layer 3: applying changes
- **L11. Targets, from narrow to wide:**

  | Target | Scope | Where it lands |
  |---|---|---|
  | `project_lesson` | project, optionally role | `lessons` table, written to `/kanbain/context/lessons.md` for that project's runs |
  | `environment` | project | `environment.md` (TODO 13) |
  | `setup_script` | project | `setup.sh` (TODO 13/14); the hash change re-runs setup |
  | `allowed_domain` | project | `ProjectSettings.sandbox_allowed_domains` |
  | `test_paths` | project | `ProjectSettings.test_paths` |
  | `bash_allow` | agent definition | `<data>/agents/<name>.yaml` |
  | `role_lesson` | role, all projects | a generated style block `learned-<role>` in `<data>/styles/`, attached to agents of that role |
  | `role_prompt` | agent definition | `role_prompt` in the agent YAML, shown as a diff |

- **L12. Nothing applies without approval.** Proposals show in the ticket's and the project's Insights tab with accept, edit-then-accept and reject. A rejection is stored with an optional reason and fed into the next retro's `current.md`, so the same proposal isn't made again. Policy-loosening targets (`allowed_domain`, `bash_allow`, `role_prompt`) are never auto-applied, even if a future setting allows auto-apply for project lessons.
- **L13. Project and role lessons before prompt edits.** A lesson is a short imperative line with its evidence, e.g. "Tests for the wasm crate need `cargo component test`; it is installed by setup.sh." Most problems are project-specific and belong in project lessons. `role_lesson` needs the same lesson in ≥3 projects; `role_prompt` edits are offered only from the project-level retro and only as a diff.

### Keeping it from rotting
- **L14. Lessons have a lifecycle.** Each one has a status (`proposed → active → retired | rejected`), `created_from` (retro id), `last_confirmed_at`, and a `hits` counter: how often the retro or the signals saw the problem again. An active lesson whose problem never shows up again over 20 runs of its scope is offered for retirement. When the retro finds the problem behind an active lesson still happening, it proposes a rewrite instead of a second lesson.
- **L15. Hard caps.** `lessons.md` per project ≤ 40 lines, `learned-<role>` ≤ 20 lines. Above the cap the retro must merge or retire before it can add. This keeps context from growing without bound.
- **L16. Promote only on repetition.** A one-off failure gives at most a project lesson with low confidence. `role_lesson` and `role_prompt` need repeats across tickets (L13). `repeat_answer` needs ≥2 tickets.
- **L17. Everything is versioned.** Every applied change writes a `lesson_changes` row (target, before, after, proposal id, applied_at, user). Agent YAML and style files keep the previous version next to them. One click rolls back any change.
- **L18. Measure before and after.** Every run records which lesson versions it got (`run_lessons`: run id → lesson ids, plus a hash of `setup.sh` and `environment.md`). For each applied change, the Insights tab compares the target signal's rate over the next N runs of that scope with the previous N. If the target signal didn't drop, or cost or retries went up, the change is flagged "no effect" or "worse" and offered for rollback. This is correlation, not proof; it is shown as such.

### Cost and privacy
- **L19. Retro runs go through the model proxy** like every agent, get a budget (default $0.50 per ticket retro, $2 per project retro) and show up in Phase 8 stats as their own role.
- **L20. Nothing leaves the project.** Project lessons are only injected into that project's runs. `role_lesson` content is generalized by the retro (no paths, names or code from the source project) and shown for review before it applies across projects.

---

## 2. Data model

New migration `2026xxxx_self_improvement.sql`:

```sql
CREATE TABLE run_signals (
    id                   BLOB PRIMARY KEY,
    execution_process_id BLOB NOT NULL REFERENCES execution_processes(id) ON DELETE CASCADE,
    kind                 TEXT NOT NULL,
    key                  TEXT NOT NULL,
    count                INTEGER NOT NULL,
    sample               TEXT,
    extractor_version    INTEGER NOT NULL,
    created_at           TEXT NOT NULL
);

CREATE TABLE retros (
    id                   BLOB PRIMARY KEY,
    project_id           BLOB NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
    issue_id             BLOB REFERENCES issues(id) ON DELETE SET NULL, -- NULL = project retro
    execution_process_id BLOB REFERENCES execution_processes(id),
    status               TEXT NOT NULL,  -- running | done | failed
    created_at           TEXT NOT NULL
);

CREATE TABLE lesson_proposals (
    id          BLOB PRIMARY KEY,
    retro_id    BLOB NOT NULL REFERENCES retros(id) ON DELETE CASCADE,
    target      TEXT NOT NULL,
    scope       TEXT NOT NULL,   -- JSON
    change      TEXT NOT NULL,   -- JSON
    rationale   TEXT NOT NULL,
    evidence    TEXT NOT NULL,   -- JSON
    confidence  REAL NOT NULL,
    status      TEXT NOT NULL,   -- proposed | accepted | rejected | superseded
    reject_reason TEXT,
    decided_at  TEXT
);

CREATE TABLE lessons (
    id                BLOB PRIMARY KEY,
    project_id        BLOB REFERENCES projects(id) ON DELETE CASCADE, -- NULL = role-wide
    role              TEXT,
    body              TEXT NOT NULL,
    status            TEXT NOT NULL, -- active | retired
    hits              INTEGER NOT NULL DEFAULT 0,
    created_from      BLOB REFERENCES lesson_proposals(id),
    last_confirmed_at TEXT,
    created_at        TEXT NOT NULL
);

CREATE TABLE lesson_changes (
    id          BLOB PRIMARY KEY,
    proposal_id BLOB REFERENCES lesson_proposals(id),
    target      TEXT NOT NULL,
    scope       TEXT NOT NULL,
    before      TEXT,
    after       TEXT,
    rolled_back_at TEXT,
    applied_at  TEXT NOT NULL
);

CREATE TABLE run_lessons (
    execution_process_id BLOB NOT NULL REFERENCES execution_processes(id) ON DELETE CASCADE,
    lesson_id            BLOB NOT NULL REFERENCES lessons(id) ON DELETE CASCADE,
    PRIMARY KEY (execution_process_id, lesson_id)
);
```

`execution_processes` gets `setup_hash` and `environment_hash` columns for L18.

## 3. Where it plugs in

- **Signal extraction:** a new `crates/services/src/services/signals.rs`, pure functions over events and normalized log entries, unit tested with fixture transcripts. Called from the run-finished path in `crates/local-deployment/src/container.rs`.
- **Denial capture (L3):** `crates/executors/src/adapters/bash_filter.sh` (append to `/kanbain/out/denials.jsonl`), OpenCode permission denials in `adapters/opencode.rs`, harvested next to the existing `/kanbain/out` reads.
- **Retro role:** `AgentRole::Retro` in `crates/executors/src/adapters/definition.rs`, a default in `default_agents()` (`adapters/mod.rs`), a launcher `crates/local-deployment/src/retro.rs` modelled on `history.rs` and `intake.rs` (build context files → run → harvest `proposals.json`).
- **Lesson injection:** `sandbox.rs` `prepare()` writes `lessons.md` next to `ticket.md` in `/kanbain/context` and records `run_lessons`. Role lessons go through the existing style-block mechanism (`ResolvedAgent::system_prompt`).
- **Applying changes:** one service, `lessons::apply(proposal)`, that switches on `target`, writes a `lesson_changes` row and calls the existing writers (project settings update, `AgentLibrary` save, style save, the setup files from TODO 13).
- **UI:** an Insights tab in `TicketPanel.tsx` and on the project page; proposal cards with accept, edit and reject; a lessons list with retire and rollback.
- **Types:** new structs derive `TS`; regenerate with `pnpm run generate-types`.

## 4. Phases

### Phase A: Signals and the Insights panel
1. `command_denied` and `file_read` capture (L3).
2. `signals.rs` with the L4 set, `run_signals` table, run-finished hook, lazy backfill.
3. Ticket and project Insights tabs, read-only.

**Demo:** the `rustup component add` denial and its count show on the ticket, and the project tab ranks it as a recurring problem.

### Phase B: Ticket retro and project lessons
1. `retro` role, context builder (L9), harvest of `proposals.json` (L10).
2. `lessons`, `lesson_proposals`, `lesson_changes` tables; the accept/edit/reject UI.
3. Targets `project_lesson`, `environment`, `setup_script`, `test_paths`.
4. `lessons.md` injection and `run_lessons`.

**Demo:** after a ticket where the tester couldn't run `cargo component`, the retro proposes a `setup.sh` line and a lesson. Once accepted, the next ticket's tester runs the tests with no denial.

### Phase C: Policy targets and the project retro
1. Targets `allowed_domain` and `bash_allow`, always manual (L12).
2. Project retro over the last N tickets, with `repeat_answer` detection.
3. Lesson lifecycle: hits, retirement offers, caps (L14, L15).

### Phase D: Measurement and cross-project learning
1. Before/after comparison and the "no effect / worse" flags (L18), rollback.
2. `role_lesson` via `learned-<role>` style blocks, then `role_prompt` diffs (L13, L16, L20).

## 5. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Context bloat: every lesson is appended forever | Caps, merge-before-add, retirement on no hits (L14, L15) |
| Overfitting to one ticket | Promotion thresholds; one-off failures stay project-scoped and low-confidence (L13, L16) |
| A bad retro teaches every agent something wrong | User approval on everything; wide targets need repetition and a diff review (L12, L13) |
| Loosening security through "lessons" (allow everything) | Policy targets are never auto-applied; the proposal card shows the exact widened pattern or host (L12) |
| Retro cost exceeds what it saves | Auto retro is off by default, budgets, and the Insights tab shows the retro role's total cost next to the savings it claims (L8, L19) |
| Before/after numbers mislead on small samples | Show N and say "correlation"; flag only after a minimum run count (L18) |
| Transcript parsing breaks when a runtime changes its log format | Signals depend on events where possible (L3); the parser is versioned and signals are rebuildable (L5) |

## 6. Open questions

- Should project lessons be stored in the repo (e.g. `.kanbain/lessons.md`) so they are shared with teammates and reviewed in PRs, or stay in the local database? Repo storage would make them part of the code but adds commits.
- Should the user's own chat corrections ("no, use pnpm") be read as a strong signal on their own, without a retro?
- Is a separate retro model (cheap model for ticket retros, strong model for project retros) worth two roles, or is one configurable role enough?
- Should lessons that worked be offered for export as a skill, so they can be shared between KanbAIn installs?
