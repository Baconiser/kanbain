<p align="center">
  <picture>
    <source srcset="packages/public/kanbain-logo-dark.svg" media="(prefers-color-scheme: dark)">
    <source srcset="packages/public/kanbain-logo.svg" media="(prefers-color-scheme: light)">
    <img src="packages/public/kanbain-logo.svg" alt="KanbAIn Logo">
  </picture>
</p>

KanbAIn (from Russian *kombain*, "combine harvester", plus kanban and AI) is a
local desktop app for Windows. You drop in Jira tickets, an LLM enhances them,
and sandboxed AI agents plan, code, test and document the work until it is
ready for your review. Nothing reaches `main` without your approval.

KanbAIn is a hard fork of [Vibe Kanban](https://github.com/BloopAI/vibe-kanban)
(Apache-2.0). It is local only: the cloud, relay and analytics features are
removed, and only the LLM APIs are remote. Supported agent runtimes are
**Claude Code** and **OpenCode**.

> KanbAIn is under construction. See [`IMPLEMENTATION_PLAN.md`](IMPLEMENTATION_PLAN.md)
> for the phases and [`docs/kanbain-changelog.md`](docs/kanbain-changelog.md)
> for what has landed.

## Development

### Prerequisites

- Rust via [rustup](https://rustup.rs/). The toolchain is pinned to a
  **nightly** in `rust-toolchain.toml`; rustup installs it automatically.
- [Node.js](https://nodejs.org/) 20 or newer
- [pnpm](https://pnpm.io/) 10.13.1 (via `corepack enable`)

Additional tools:

```bash
cargo install cargo-watch
cargo install sqlx-cli --no-default-features --features sqlite,rustls
```

Install dependencies:

```bash
pnpm i
```

### Running

```bash
pnpm run dev        # backend + web app in the browser (development only)
pnpm run tauri:dev  # desktop app
```

`pnpm run dev` copies a blank database from `dev_assets_seed`. Users run the
Tauri app; the browser setup is for development and agent testing.

### Checks

```bash
pnpm run check   # TypeScript + cargo check
pnpm run lint    # ESLint + clippy + unused i18n keys
cargo test --workspace
pnpm run format
```

### Building

```bash
pnpm run tauri:build   # desktop installer for the current platform
```

The `Windows build` GitHub Actions workflow builds the Windows installers on
`windows-latest` and uploads them as an artifact.

### Environment variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | auto | **Production**: server port. **Dev**: frontend port (backend uses PORT+1) |
| `BACKEND_PORT` | auto | Backend server port (dev only, overrides PORT+1) |
| `FRONTEND_PORT` | `3000` | Frontend dev server port (dev only) |
| `PREVIEW_PROXY_PORT` | auto | Port of the preview proxy for workspace dev servers |
| `HOST` | `127.0.0.1` | Backend server host |
| `MCP_HOST` | value of `HOST` | Host the MCP server connects to |
| `MCP_PORT` | value of `BACKEND_PORT` | Port the MCP server connects to |
| `KANBAIN_BACKEND_URL` | not set | Full backend URL for the MCP server (overrides host and port) |
| `KANBAIN_ALLOWED_ORIGINS` | not set | Comma-separated extra origins allowed to call the backend API |
| `DISABLE_WORKTREE_CLEANUP` | not set | Disable git worktree cleanup (for debugging) |

## License

Apache-2.0, as inherited from Vibe Kanban. See [LICENSE](LICENSE).
