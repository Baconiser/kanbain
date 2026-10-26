Agent Dashboard: Vibe Kanban Fork Brief
26. Okt. 2026 · @Michael
How to use this brief
This brief describes a personal AI dev dashboard built as a fork of Vibe Kanban. It is written for a coding agent (Claude) to turn into a concrete implementation plan.
Before planning:
1. Read this whole brief, then explore the forked Vibe Kanban codebase. Find how projects, issues, workspaces and executors (agent integrations) are structured.
2. Ask me clarifying questions where the brief is ambiguous or conflicts with how the codebase works. Do not guess on architecture decisions.
3. Produce a phased plan that maps each feature below to specific modules, files and changes in the codebase.
4. Keep the existing app working after every phase. Prefer small, reviewable changes over big rewrites.
5. Never push to the upstream repo or merge to main without my approval.
Goal and vision
One dashboard where I see all my tasks across multiple projects, their progress, the bugs and issues found, and every change made.
I create a ticket and assign it to the AI. A planning agent clarifies and plans it, I approve the plan, and an orchestrator runs coding and testing agents in sandboxes until the work is ready for my review. Everything is recorded as markdown changelogs.
Key principles:
• I stay in control: nothing merges to main without my approval.
• Agents are configurable: runtime, model, system prompt, style and capabilities per agent.
• Every agent runs in a sandbox and cannot affect anything outside it.
• Everything is traceable: what each agent did, why, and what it cost.
Starting point: Vibe Kanban fork
The base is a fork of Vibe Kanban (Rust backend, TypeScript frontend, Apache-2.0). The company behind it, bloop, shut down on April 10, 2026; the project continues as community-maintained open source. Its hosted Cloud features were wound down, so this fork targets local use only.
Already exists in Vibe Kanban
Status for this project
Kanban issues to plan, prioritise and assign work
Reuse as tickets
Workspaces: each gives an agent a branch (git worktree), terminal and dev server
Reuse, wrap in Docker sandbox
10+ agent executors incl. Claude Code and OpenCode
Reuse, add agent profiles on top
Subtasks, optionally split by AI in a plan stage
Extend into full planning flow
Diff review, PRs, merge, rebase
Reuse for the review step
MCP config management per agent
Reuse
Setup: Rust (latest stable), Node.js 18+, pnpm 8+, then pnpm i and pnpm run dev.
Before starting, check the GitHub fork network for an actively maintained community fork. If one exists and is healthy, consider building on it instead. Maintenance is now my responsibility, including bumping pinned agent CLI versions (Claude Code, OpenCode).
Core workflow
Ticket states: Backlog → Planning → Awaiting approval → In progress → Testing → Review → Done, with Blocked possible from any state.
1. Create and assign. I create a ticket in a project and assign it to the AI.
2. Planning (grill-me mode, default). The planning agent reads the ticket and the relevant code, then uses the grill-me skill: it asks me pointed questions in the ticket thread about scope, edge cases, constraints and what "done" means, until requirements are clear.
3. Planning (yolo mode). If I reply "yolo" (or set the ticket to yolo), the agent skips questions, makes its own decisions, and lists every assumption explicitly in the plan.
4. Plan output. Subtasks, files likely affected, acceptance criteria, test strategy, and where test checkpoints happen. Assign subtasks so parallel coders touch different files where possible.
5. Approval gate. I approve, edit, or send the plan back. Nothing is coded before approval.
6. Orchestration. The orchestrator splits the approved plan into subtasks and spawns coding agents, each in its own sandbox on its own branch. Subtasks that touch the same files run sequentially, not in parallel.
7. Testing. At the checkpoints defined in the plan, a testing agent runs tests and checks the work against the acceptance criteria. It is read-only and writes its own tests where useful, independent of the coder.
8. Fix loop. Failures become issues on the ticket and go back to the coders. Maximum 3 rounds, then the ticket moves to Blocked and I get notified.
9. Review. I get a summary, the diff, the issues found and fixed, and the changelog. I merge or request changes.
Agents
Each agent is configured once in the dashboard in a neutral format. An adapter translates it into Claude Code or OpenCode format when a run starts. Start with one runtime, get the full loop working, then add the second adapter.
Roles needed: planner, orchestrator, coder, tester, scribe (writes changelogs).
Each agent has three prompt layers:
• Role (system prompt): who the agent is and how it works. Set once per agent.
• Style: reusable blocks attachable to any agent (e.g. concise commits, project conventions, comment language). Appended to the system prompt; editing one block updates every agent using it.
• Task: generated per ticket or subtask from the approved plan, sent as the first message, never baked into the system prompt.
Neutral agent definition:
name: backend-coder
runtime: claude-code        # or opencode
model: sonnet               # adapter maps per runtime
role_prompt: "You are a backend developer..."
styles: [concise-commits, project-conventions]
capabilities: [read, edit, bash, test]
limits: { max_steps: 40, budget_usd: 2 }
Adapter mapping:
Neutral setting
Claude Code
OpenCode
Agent definition
Markdown with YAML frontmatter, or JSON via claude -p --agents ./agents.json
Markdown in .opencode/agents/ or opencode.json
Model
Alias (sonnet, opus, haiku) or full model ID
provider/model-id
Read-only
tools: Read, Grep, Glob
permission: edit: deny
Limited bash
Bash plus a PreToolUse hook that blocks forbidden commands
Per-command bash permission rules
Max steps
maxTurns
Max steps setting
Store agent configs in the database and also export them as files in the repo so they are version-controlled. Verify exact field names against current Claude Code and OpenCode docs before implementing.
Sandboxing and model proxy
Every agent run happens in its own Docker container. Git worktrees (what Vibe Kanban uses today) isolate branches but are not a security boundary, so the container is required.
• The container gets a clone or worktree of the project and nothing else from the host. No home directory, no SSH keys, no Docker socket.
• CPU, memory and wall-time limits per container.
• Work is preserved by committing to the agent's branch; the container is deleted afterward.
• Docker first. gVisor or Firecracker can replace it later without changing the rest of the design.
Because Claude Code and OpenCode run inside the container, they need model access. Solve this with a model proxy running outside the sandbox:
1. The container's only allowed network destination is the proxy.
2. The proxy injects the real API key and forwards to the model provider. The key never enters the sandbox.
3. The proxy logs every model call, so it provides per-run and per-ticket cost tracking.
4. The proxy can enforce budgets and cut off a runaway agent instantly.
Claude Code supports this via ANTHROPIC_BASE_URL pointing at a gateway. Confirm the equivalent setting for OpenCode.
Changelogs and tracking
All history lives in markdown files in the project repo, so it survives even if the dashboard breaks. The database indexes them for the UI.
• Per ticket: .agents/changelog/<TICKET-ID>.md. The scribe appends an entry after each run: what changed, why, which files, which issues were found and fixed, and the commit reference.
• Per project: a summary CHANGELOG.md updated when a ticket reaches Done.
• Event log: every model call and tool call per run is logged as structured data (JSON). This feeds the dashboard and debugging.
• Structured issues: the tester writes findings as JSON with fields file, line, severity (low/medium/high), category (edge case, integration, logic, error handling, state/timing, requirements), and description. Charts depend on this structure.
Dashboard views
Reuse Vibe Kanban's existing UI where possible and add these views. Updates should be live (WebSockets or SSE), not refresh-based.
View
Shows
Projects overview
All projects with open tickets, active agents, open issues and cost at a glance
Project board
Kanban of tickets by the states in Core workflow
Ticket detail
Plan and its versions, grill-me conversation, live agent activity, issues, diffs, changelog, running cost
Issues and stats
Charts: issues by category, by file, by severity, and over time; cost per ticket
Agents
List, create, edit and test agent configs; manage style blocks
Data model additions
Map these onto Vibe Kanban's existing schema; extend existing entities rather than duplicating them.
Entity
Key fields
Ticket (extend issue)
mode (grill / yolo), state per Core workflow, fix-round count
Plan
ticket, version, subtasks, acceptance criteria, assumptions, test checkpoints, approval status
Agent definition
name, role, runtime, model, role prompt, style refs, capabilities, limits
Style block
name, content
Run
agent, ticket/subtask, container id, branch, status, start/end, tokens, cost
Event
run, timestamp, type (model call / tool call), payload
Issue
ticket, run, file, line, severity, category, description, status
Changelog entry
ticket, run, summary, files changed, commit ref, file path
Build phases
Each phase must leave the app usable. Each phase ends with a short demo I can run and a changelog entry.
1. Phase 0: Fork and learn. Build the fork, run it unmodified, document how projects, issues, workspaces and executors work. Identify exactly where agents are launched.
2. Phase 1: Sandbox and proxy. Wrap agent launches in Docker containers; add the model proxy with logging and budgets. One agent, one ticket, end to end.
3. Phase 2: Agent profiles. Neutral agent definitions, style blocks, adapter for the first runtime. Config files first; UI can come later.
4. Phase 3: Planning flow. Planner agent with grill-me and yolo modes, plan versions, approval gate, new ticket states.
5. Phase 4: Orchestrator. Split approved plans into subtasks, spawn parallel coders on separate branches, sequence overlapping subtasks, merge into the ticket branch.
6. Phase 5: Testing and fix loop. Tester agent at checkpoints, structured issues, max 3 rounds, Blocked state with notification.
7. Phase 6: Changelogs. Scribe agent, per-ticket and per-project markdown files.
8. Phase 7: Stats and charts. Issues and cost dashboards.
9. Phase 8: Agents UI and second runtime. Edit agents and styles in the dashboard; add the second adapter.
Risks, constraints, and open questions
Risks to design for:
• Parallel coders conflicting: the orchestrator assigns non-overlapping files and sequences overlapping subtasks.
• Costs: multiple agents plus retries add up. Per-ticket budget enforced by the proxy, running cost visible on every ticket.
• Runaway agents: max steps, wall-time limits, and budget cutoffs per run.
• Upstream drift: agent CLIs change often. Keep pinned versions and update them deliberately.
• Tests that verify nothing: the tester checks against acceptance criteria, not just whether tests pass.
Open questions to ask me before planning:
[ ] Claude Code or OpenCode as the first runtime?
[ ] Build on the original repo or an active community fork?
[ ] Where does it run: my PC only, or a home server I access remotely?
[ ] Which models per role (e.g. stronger model for planner and orchestrator, cheaper for scribe)?
[ ] Default budget per ticket?
[ ] Where is the grill-me skill defined, and should the planner load it as a skill or have it built into its prompt?
    
    
    The Projects name is now KanbAIn like the russian word Kambain for processing machine and kanban and AI is also included in the word
