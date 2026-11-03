# MCP registry: where to proceed (as of 2026-11-03)

Feature: **one global list of MCP servers** in KanbAIn's settings, which
every agent gets by default; each agent can **untick** global servers and
**add its own extras**. Branch: `kanbain/phase-6` (nothing committed yet).

## Why
Today an MCP server for a sandboxed agent has to be set in three places,
and the page called "MCP Servers" is none of them:
- Settings → MCP Servers (upstream) writes the **host's**
  `~/.config/opencode/opencode.json` / `~/.claude.json`. Sandboxed runs
  never read those: opencode gets its whole config via
  `OPENCODE_CONFIG_CONTENT`, Claude via `--strict-mcp-config --mcp-config`.
- The only thing that reaches a run is the agent YAML's `mcp_servers`
  (Agent Library → agent → MCP servers), read in
  `crates/executors/src/adapters/opencode.rs:197` and `adapters/claude.rs:120`.
- Remote servers also need their host on the sandbox egress allowlist, and
  tokens sit in plain text in the agent YAML (they end up readable by the
  agent inside the container).

## Decisions (agreed with the user)
- **Secrets:** headers/env may use `{secret:NAME}`; the value is stored in
  the encrypted `SecretStore` (`crates/services/src/services/secrets.rs`)
  under `mcp_secret:<NAME>` and substituted at launch. Config and agent YAML
  stay token-free.
- **Default:** a global server is **on for every agent**; agents opt out.
- **Remote servers go through the model proxy** (my call; it's the same
  pattern as provider keys and the built-in `kanbain` MCP): the container
  gets `${ANTHROPIC_BASE_URL}/mcp/<name>` + `Bearer ${ANTHROPIC_AUTH_TOKEN}`
  (the run token); the proxy on the host adds the real URL and headers.
  So: the real token never enters the container **and** no egress
  allowlist entry is needed for remote MCP hosts.
- **Local (`command`) servers** run inside the container, so their
  `{secret:…}` env values are substituted into the container config: the
  agent can read them. That's inherent; the UI should say so. Their API
  hosts still need sandbox allowed domains.
- The name `kanbain` is reserved (built-in search MCP).
- Config is extended in place in v8 with `#[serde(default)]` (the repo's
  pattern; no v9).

## Done
- `crates/model-proxy/src/remote_mcp.rs` (**new, not wired yet**: no
  `mod remote_mcp;` in `lib.rs`, so the build is unaffected). Handlers
  `root_handler` (`/mcp/{name}`) and `rest_handler` (`/mcp/{name}/{*rest}`):
  look up the run from the token, find `inner.mcp_upstreams[run_id][name]`,
  forward method/body/query/headers (drops auth via
  `is_dropped_request_header`), add the upstream headers, stream the
  response back unchanged (SSE works).

## Still to do
1. **model-proxy `lib.rs`**
   - `pub mod remote_mcp;` and `pub use remote_mcp::remote_mcp_path;`
   - `Inner.mcp_upstreams: RwLock<HashMap<Uuid, HashMap<String, Upstream>>>`
     (init in `ModelProxy::new`).
   - `pub fn set_mcp_upstreams(&self, run_id, HashMap<String, Upstream>)`;
     clear the run's entry in `revoke_run`.
   - Make `MAX_REQUEST_BYTES`, `is_dropped_request_header`,
     `is_dropped_response_header` `pub(crate)` if needed.
   - Router: `.route("/mcp/{name}", any(remote_mcp::root_handler))` and
     `.route("/mcp/{name}/{*rest}", any(remote_mcp::rest_handler))` next
     to the existing exact `/mcp` route (axum 0.8 `{}` syntax).
   - Tests in `src/tests.rs`: a mock upstream that asserts the real header
     and no run token; unknown token → 401; other run's server → 404;
     revoked run → 401; an SSE response passes through.
2. **executors**
   - `AgentDefinition.mcp_exclude: Vec<String>` (`#[serde(default,
     skip_serializing_if = "Vec::is_empty")]`) in `adapters/definition.rs`.
   - `ResolvedAgent.proxied_mcp: Vec<String>` (names served via the
     proxy) in `adapters/mod.rs`; update every `ResolvedAgent { … }`
     literal (`agent_library.rs:717`, `cli_drift.rs:40`, tests in
     `opencode.rs`, `claude.rs`, `mod.rs`).
   - Pure fn `merge_mcp_servers(global, def) -> Map`: global minus
     `mcp_exclude`, plus `def.mcp_servers` (agent wins on a name clash),
     with unit tests.
   - Both adapters render each `proxied_mcp` name like the `kanbain`
     entry, but at `/mcp/<name>` (Claude: `${ANTHROPIC_BASE_URL}`,
     opencode: `{env:ANTHROPIC_BASE_URL}`).
3. **services**
   - `Config.mcp_servers: serde_json::Map<String, Value>` in
     `config/versions/v8.rs` (`#[serde(default)]`, TS
     `Record<string, unknown>`; also in `from_v7_config` and `Default`).
   - `SecretStore::names_with_prefix(prefix)`; `pub fn mcp_secret(name)`
     → `mcp_secret:<NAME>`.
   - `resolve_secret_refs(&mut Value, lookup)` replacing `{secret:NAME}`
     in strings; missing secret → error naming it. Unit tests.
4. **local-deployment `sandbox.rs`**: `prepare_mcp(&self, agent: &mut
   ResolvedAgent, run_id)`: merge (config + agent), resolve secrets,
   remote ones → `proxy.set_mcp_upstreams(run_id, …)` + push to
   `proxied_mcp` + remove from `definition.mcp_servers`; local ones keep
   resolved env/args. Call it before `adapter_for(..).launch(..)` in
   `container.rs:~1789` (run id = `execution_process.id`) and `jobs.rs:~96`
   (run id = `job.id`). Run notes could list the MCP servers.
5. **server routes** (`routes/sandbox.rs` pattern): `GET /mcp/secrets` →
   names; `PUT /mcp/secrets` `{name, value | null}`. Register types in
   `crates/server/src/bin/generate_types.rs`, then `pnpm run
   generate-types`. Decide what to do with the old `/config/mcp-config`
   routes (now unused by the UI; Phase 9 reuses Settings → MCP for host
   AI clients, D51).
6. **Frontend** (`packages/web-core/src/shared/dialogs/settings/settings/`)
   - Rewrite `McpSettingsSection.tsx`: JSON editor for `config.mcp_servers`
     (save via `updateAndSaveConfig`, copy `SandboxSettingsSection.tsx`),
     plus an "MCP secrets" card (list names, set/remove, password input).
     Explain `{secret:NAME}`, that remote servers go through the proxy,
     that local servers' secrets are visible to the agent.
   - `agent-library/AgentEditor.tsx`: checklist of global servers (ticked
     = not in `mcp_exclude`) above the existing JSON field, relabelled
     "Extra servers for this agent".
   - i18n keys in `i18n/locales/*/settings.json` (`pnpm run check-i18n`).
7. **Docs/plan**: a D-entry in `IMPLEMENTATION_PLAN.md`, a changelog entry
   in `docs/kanbain-changelog.md`, `docs/settings/mcp-servers.mdx`.
8. **Verify** (stop backend and `kb-*` containers before cargo, see the
   memory budget note): `cargo test --workspace`, `pnpm run check`,
   `pnpm run lint`, `pnpm run generate-types:check`, `pnpm run format`.
   Smoke test: global `fal-ai` server
   (`{"url":"https://mcp.fal.ai/mcp","headers":{"Authorization":"Bearer {secret:FAL_KEY}"}}`),
   one opencode agent, check that its run lists the fal tools; untick it
   on another agent and check it's absent.

## The user's original fal config (for the test)
```json
"fal-ai": {
  "type": "http",
  "url": "https://mcp.fal.ai/mcp",
  "headers": { "Authorization": "Bearer token" }
}
```
