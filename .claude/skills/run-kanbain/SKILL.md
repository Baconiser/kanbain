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
```

Usually 3000 / 3001 / 3002 (stored in `.dev-ports.json`).

## 2. Backend

Build once, then run the binary directly. This is faster to restart than
`pnpm run dev`, which wraps it in `cargo watch`.

```bash
cargo build --bin server
BACKEND_PORT=$BACKEND_PORT PREVIEW_PROXY_PORT=$PREVIEW_PROXY_PORT \
  VK_ALLOWED_ORIGINS=http://localhost:$FRONTEND_PORT \
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
2. Screenshots `/workspaces` and the `/tickets` placeholder.
3. Opens Settings → Agents and reports which agents are listed.
4. Prints console errors and failed `/api/*` calls.

**Read the screenshots.** A shell that renders while API calls fail still
shows up in the printed failures. Extend `smoke.mjs`, or copy it, for
feature-specific paths.

## 5. Stop

```bash
lsof -ti:$FRONTEND_PORT -sTCP:LISTEN | xargs -r kill
lsof -ti:$BACKEND_PORT -sTCP:LISTEN | xargs -r kill
```

Don't use a broad `pkill -f`; it can match the agent's own shell.
