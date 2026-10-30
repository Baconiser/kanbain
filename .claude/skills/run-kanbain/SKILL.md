---
name: run-kanbain
description: Launch KanbAIn (Rust backend + Vite frontend) headless in the Linux dev container and drive it with Playwright Chromium to confirm pages render. Use to run, screenshot or smoke-test the app after a change.
---

# Run KanbAIn headless

The Tauri desktop app is for Windows users; agents verify through the dev
setup: the `server` binary plus the Vite frontend, driven by headless
Chromium. Everything below was verified in this dev container.

## 1. Ports

```bash
cd /workspaces/kanbain
export FRONTEND_PORT=$(node scripts/setup-dev-environment.js frontend)
export BACKEND_PORT=$(node scripts/setup-dev-environment.js backend)
export PREVIEW_PROXY_PORT=$(node scripts/setup-dev-environment.js preview_proxy)
# Every /api call needs the backend's launch token; share one between the
# backend and the Vite proxy (both read this variable).
export KANBAIN_LAUNCH_TOKEN=$(node -e "console.log(require('crypto').randomBytes(32).toString('hex'))")
```

Usually 3000 / 3001 / 3002 (stored in `.dev-ports.json`).

## 2. Backend

Build once, then run the binary directly. This is faster to restart than
`pnpm run dev`, which wraps it in `cargo watch`.

```bash
cargo build --bin server
BACKEND_PORT=$BACKEND_PORT PREVIEW_PROXY_PORT=$PREVIEW_PROXY_PORT \
  KANBAIN_ALLOWED_ORIGINS=http://localhost:$FRONTEND_PORT \
  DISABLE_WORKTREE_CLEANUP=1 RUST_LOG=info \
  nohup ./target/debug/server > /tmp/kanbain-backend.log 2>&1 &
timeout 120 bash -c "until curl -sf http://127.0.0.1:$BACKEND_PORT/api/health >/dev/null; do sleep 2; done"
```

Data lives in `dev_assets/` (seeded from `dev_assets_seed/`). A fresh copy
starts on the onboarding page.

## 3. Frontend

```bash
(cd packages/local-web && BACKEND_PORT=$BACKEND_PORT \
  nohup pnpm run dev -- --port $FRONTEND_PORT > /tmp/kanbain-frontend.log 2>&1 &)
timeout 300 bash -c "until curl -sf http://localhost:$FRONTEND_PORT >/dev/null; do sleep 2; done"
```

Vite needs about 2 minutes to become ready on the `/workspaces` mount, and
it listens on `localhost`, not `127.0.0.1`. Poll `localhost`.

## 4. Drive it

One-time setup (pinned Playwright, Chromium into `~/.cache/ms-playwright`):

```bash
cd .claude/skills/run-kanbain/driver
npm install --no-audit --no-fund
npx playwright install chromium
```

Smoke run:

```bash
node .claude/skills/run-kanbain/driver/smoke.mjs http://localhost:$FRONTEND_PORT /tmp/kanbain-shots
```

`smoke.mjs` does the following:

1. Clicks through onboarding if it appears.
2. Screenshots `/workspaces` and the `/tickets` board.
3. Opens Settings → Agents and reports which agents are listed.
4. Prints console errors and failed `/api/*` calls.

For the ticket board, `tickets.mjs` (same arguments) creates a project and a
ticket, edits it, comments, drags it to In progress, checks the change
arrives live in a second window and survives a reload, and prints PASS/FAIL
lines.

**Read the screenshots.** A shell that renders while API calls fail still
shows up in the printed failures. Extend `smoke.mjs`, or copy it, for
feature-specific paths.

## 5. Sandboxed agent runs (Phase 1)

Agents run in Docker (docker-in-docker in this dev container). The first
backend start builds `kanbain-agent:<ver>-<hash>` in the background, which
takes about 8 minutes; `GET /api/sandbox/status` shows `image_building`.

Without a real API key, point the model proxy at the scripted mock
upstream. It answers Claude Code with Bash tool calls from a JSON script,
then an end_turn, and reports usage so costs are recorded:

```bash
D=.claude/skills/run-kanbain/driver
python3 $D/mock_anthropic.py 18555 $D/mock-script-commit.json /tmp/mock.log &
# start the backend with these two extra variables:
#   KANBAIN_ANTHROPIC_BASE_URL=http://127.0.0.1:18555 ANTHROPIC_API_KEY=mock-upstream-key
node $D/sandbox-run.mjs $BACKEND_PORT $KANBAIN_LAUNCH_TOKEN /tmp/repos/demo demo
```

`sandbox-run.mjs` creates a repo, project and ticket, starts a Claude Code
workspace run and prints the run's status, tokens, cost, events (model
calls, blocked egress, sync, harvest) and the host branch. Pass a budget
as the fifth argument and use `mock-script-budget.json` (a long streamed
answer) to see the budget cut-off and the ticket move to Blocked.
`sandbox.mjs` (frontend URL, shots dir, workspace id) checks Settings →
Sandbox and the run summary in the Logs panel.

Start long-running helpers with `setsid nohup … &` so a stopped tool call
does not take them down, and stop them by port (`lsof -ti:<port>`).

## 6. Agent definitions (Phase 2)

Agents, style blocks and skills live in `dev_assets/agents`, `styles` and
`skills` (seeded with the six default agents on first start) and are
served under `/api/agent-library`. Agent runs need the sandbox.

The mock upstream logs, per request, the tool names and which of the
script's `markers` appear in the system prompt (`system_markers`) or
anywhere in the request (`markers`, which includes tool results). It also
answers OpenAI-compatible `/v1/chat/completions`; start the backend with
`KANBAIN_OPENAI_BASE_URL=http://127.0.0.1:18555 OPENAI_API_KEY=mock-openai-key`
as well to route OpenCode's `openai/<model>` agents to it.

- `sandbox-run.mjs … <label> - <agent>` runs one ticket with an agent
  definition (`-` skips the budget argument).
- `agent-demo.mjs <backendPort> <token> <repoDir> <mockLog> <skillZip>`
  with `mock-script-agent-demo.json`: imports the zip's skill, creates
  `demo-coder` and runs one ticket on Claude Code, on OpenCode (Anthropic)
  and on OpenCode with `openai/gpt-4.1-mini`, then edits the style block
  and runs both runtimes again, checking prompts, the skill in the
  container, keys and the host branch. The zip needs a skill whose
  description contains "Imported from a zip in the agent demo".
- `agents.mjs <frontendUrl> <shotsDir> <backendPort> <token> [skillZip]`
  drives Settings → Agents (styles, skills, zip import, test run), the
  agent picker in the create chat box and the Claude account sign-in link
  in Settings → Sandbox.

`scripts/snapshot-agent-clis.sh kanbain-agent:<tag>` regenerates the CLI
interface snapshots the adapter drift tests use; run it after bumping a
version in `crates/executors/src/versions.rs`.

## 7. Ticket intake (Phase 3a)

Intake and history jobs are headless agent runs in throwaway containers
(`kanbain-job-*`); they need the sandbox too. The mock's `text_replies`
answer them (first `when` found in the request's messages wins):

```bash
python3 $D/mock_anthropic.py 18555 $D/mock-script-intake.json /tmp/mock.log &
# backend with KANBAIN_ANTHROPIC_BASE_URL / ANTHROPIC_API_KEY as above
node $D/intake.mjs http://localhost:$FRONTEND_PORT /tmp/shots /path/to/update.pdf
```

`intake.mjs` drops a Jira ticket containing `[PROJEKTNAME-123]`, reviews
and creates it, drops it again with the PDF (whose text must contain
"Update: the reset link must expire after 30 minutes") to add description
version 2, and checks the diff, the extracted PDF text, the timeline and
the New ticket dialog's External ID warning. Any one-page PDF with that
text works (`text_extract.rs` tests show how to write one).

API checks without the UI: `POST /api/projects/{id}/intakes`, poll
`GET /api/intakes/{id}`, then `POST /api/intakes/{id}/accept`;
`GET /api/issues/{id}/descriptions` and `/timeline`.

**History check (Phase 3b):** start the mock with `mock-script-history.json`,
then `node $D/history.mjs http://localhost:$FRONTEND_PORT /tmp/shots
$BACKEND_PORT $KANBAIN_LAUNCH_TOKEN /tmp/repos/shop`. It builds a repo with
backdated commits for `PROJEKTNAME-123`, creates the ticket in the UI,
allows the analysis, checks the summary, files and timeline, then runs a
sandboxed agent on the ticket and checks the agent commit's links, the run
events and that the run read `prior_work.md`. API:
`GET /api/issues/{id}/history`, `POST …/history/check|allow|deny`.

## 8. Search (Phase 3c)

A real embedder runs fine on CPU with TEI (about 1.3 GB RAM per model; the
model downloads from Hugging Face on first start):

```bash
docker run -d --name kb-tei-a -p 8090:80 ghcr.io/huggingface/text-embeddings-inference:cpu-1.7 --model-id BAAI/bge-small-en-v1.5
docker run -d --name kb-tei-b -p 8091:80 ghcr.io/huggingface/text-embeddings-inference:cpu-1.7 --model-id BAAI/bge-base-en-v1.5
node $D/search.mjs http://localhost:$FRONTEND_PORT /tmp/shots $BACKEND_PORT $KANBAIN_LAUNCH_TOKEN \
  dev_assets/embeddings.sqlite http://localhost:8090 BAAI/bge-small-en-v1.5 http://localhost:8091 BAAI/bge-base-en-v1.5
```

It configures the text embedder through `PUT /api/config`, seeds 30
tickets, checks paraphrase and id search, partial re-embedding and the
model switch, then the UI. `UI_PROJECT="Search <n>"` reruns only the UI
part against an earlier run's project (one TEI container is enough then).
Memory is tight with two TEI containers, Vite, Chromium and a Rust build;
build with `CARGO_BUILD_JOBS=2` and stop TEI containers you don't need.

## 8b. Code search and the token benchmark (Phase 3d)

One TEI container is enough: pass it as both the text and the code
embedder. The code slot without the query prefix is a different
configuration, so it still forces a code-space rebuild.

```bash
python3 $D/mock_anthropic.py 18555 $D/mock-script-benchmark.json /tmp/mock.log &
# backend with KANBAIN_ANTHROPIC_BASE_URL / ANTHROPIC_API_KEY as in section 5
node $D/code-search.mjs $BACKEND_PORT $KANBAIN_LAUNCH_TOKEN dev_assets/embeddings.sqlite \
  /path/to/workdir http://localhost:8090 BAAI/bge-small-en-v1.5 http://localhost:8090 BAAI/bge-small-en-v1.5 3
```

Part A clones this repo into `<workdir>/kanbain-src` and indexes it. That
is about 6,400 chunks and **about 30 minutes per pass** on CPU TEI, and the
code-embedder switch adds a second pass. `PART=B` skips Part A and runs
only the sandboxed benchmark and the overlay check (a few minutes).

Every Part A run creates a new full-repo project. Delete old "KanbAIn
code <n>" projects first, or each pass embeds every copy: the index once
grew to 13k chunks this way.

## 8c. Retrieval evaluation (Phase 3e)

`eval.mjs` runs 41 query → expected-hit pairs (31 code, 9 tickets, 1 run)
against the newest "KanbAIn code", "Search " and "Bench " projects. It
reports recall@10 and MRR with and without the reranker, overall and per
group. It needs the embedder and the reranker together:

```bash
docker run -d --name kb-rerank -p 8093:80 ghcr.io/huggingface/text-embeddings-inference:cpu-1.7 --model-id cross-encoder/ms-marco-MiniLM-L-6-v2
# Settings → Search → Reranker: TEI, http://localhost:8093, timeout 10 s
node $D/eval.mjs $BACKEND_PORT $KANBAIN_LAUNCH_TOKEN both 10
```

These are the only two TEI containers you may run together. Do not
build, run Vite or Chromium meanwhile, and watch `free -m`. The
cross-encoder needs about 0.4 s per 2,000-character document on this CPU,
so keep `rerank_candidates` × `rerank_doc_chars` small (the default is
20 × 500, about 1.5 s per search). The reranker's order is fused with the
RRF order at `rerank_weight` 0.5, so a web-trained cross-encoder can't
scramble code results. To check the fallback, `docker stop
kb-rerank` mid-run: every search should still answer, with `reranked
0/41`.

## 8d. Planning flow (Phase 4)

The planner runs in the sandbox like any agent. `mock-script-planning.json`
uses the mock's `phases`: the phase whose `when` text is in the latest
prompt plays its steps, so one session can ask questions (`Mode: ask`),
continue after the answer, revise after a send-back, or plan straight away
(`Mode: yolo`). Steps fill `{related_ticket}`, `{related_changed}` and
`{related_file}` from the `related.md` the planner read.

```bash
python3 $D/mock_anthropic.py 18555 $D/mock-script-planning.json /tmp/mock-plan.log &
# backend with KANBAIN_ANTHROPIC_BASE_URL / ANTHROPIC_API_KEY as in section 5
node $D/planning.mjs http://localhost:$FRONTEND_PORT /tmp/shots $BACKEND_PORT \
  $KANBAIN_LAUNCH_TOKEN /tmp/repos/plan-shop /tmp/mock-plan.log
```

It builds a repo where PLN-1 (Done) changed `src/mailer.rs`, then in the UI
assigns PLN-2 to the planner, answers its questions, edits plan v1 into v2,
sends v2 back (v3), approves, and checks the timeline. PLN-3 (yolo) is moved
to Planning through the bulk API and must get a plan citing PLN-1. It also
checks the mock log: `related.md` was read, a Write outside `/kanbain/out`
was refused, and the planner had no Edit or Bash. The search embedder is
optional; without one, related search falls back to keywords. A full run
takes 10 to 25 minutes, mostly sandbox start-up per planner turn.

The bulk update body is flat: `[{ "id": …, "status_id": … }]`. Unknown
keys such as `changes` are ignored silently.

## 9. Stop

```bash
lsof -ti:$FRONTEND_PORT -sTCP:LISTEN | xargs -r kill
lsof -ti:$BACKEND_PORT -sTCP:LISTEN | xargs -r kill
```

The backend removes its sandbox containers on the next start. To remove
them now: `docker ps -aq --filter label=dev.kanbain.managed=true | xargs -r docker rm -f`.

Don't use a broad `pkill -f`; it can match the agent's own shell.
