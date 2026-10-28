// Phase 2 demo through the backend API: one ticket, one agent definition,
// both runtimes. Needs the mock upstream (mock_anthropic.py) started with
// mock-script-agent-demo.json and its log path.
// Usage: node agent-demo.mjs <backendPort> <token> <repoDir> <mockLog> <skillZip>
//
// 1. Imports a skill zip and creates a style block and `demo-coder`.
// 2. Runs the same ticket with demo-coder on Claude Code, on OpenCode
//    (Anthropic through the proxy) and on OpenCode with an OpenAI-compatible
//    model.
// 3. Edits the style block and runs Claude Code and OpenCode again.
// For every run it prints status, model calls, which prompt markers the
// model saw and whether the imported skill was readable in the container.
import { execSync } from 'node:child_process';
import { readFileSync } from 'node:fs';

const [port, token, repoDir, mockLog, skillZip] = process.argv.slice(2);
const base = `http://127.0.0.1:${port}/api`;
const headers = { 'x-kanbain-token': token };

async function api(method, path, body) {
  const res = await fetch(base + path, {
    method,
    headers: { ...headers, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok || json.success === false) {
    throw new Error(`${method} ${path} -> ${res.status} ${JSON.stringify(json)}`);
  }
  return json.data;
}

let failures = 0;
const check = (ok, label) => {
  console.log(`  ${ok ? 'PASS' : 'FAIL'} ${label}`);
  if (!ok) failures += 1;
};
const sh = (cmd, cwd) => execSync(cmd, { cwd, encoding: 'utf8' }).trim();
const mockLines = () =>
  readFileSync(mockLog, 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l));

// --- Library: skill (zip import), style, agent ----------------------------
const form = new FormData();
form.append('file', new Blob([readFileSync(skillZip)]), 'demo-skill.zip');
const imported = await (
  await fetch(`${base}/agent-library/import`, { method: 'POST', headers, body: form })
).json();
console.log('imported', JSON.stringify(imported.data));

const style = 'demo-style';
await api('POST', '/agent-library/styles', {
  name: style,
  body: 'STYLE-DEMO-V1: write short commit messages.',
}).catch(() =>
  api('PUT', `/agent-library/styles/${style}`, {
    name: style,
    body: 'STYLE-DEMO-V1: write short commit messages.',
  })
);
const agent = {
  name: 'demo-coder',
  description: 'Phase 2 demo agent',
  role: 'coder',
  runtime: 'claude-code',
  model: 'sonnet',
  role_prompt: 'ROLE-DEMO: you are a backend developer.',
  styles: [style],
  skills: ['zipped-skill'],
  capabilities: ['read', 'edit', 'bash'],
  limits: { max_steps: 20, budget_usd: 1, wall_minutes: 10 },
};
await api('POST', '/agent-library/agents', agent).catch(() =>
  api('PUT', `/agent-library/agents/${agent.name}`, agent)
);

// --- One repo, project and ticket ----------------------------------------
sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}`);
sh('git init -q -b main && git config user.email demo@example.com && git config user.name Demo', repoDir);
sh('echo "# demo" > README.md && git add . && git commit -qm init', repoDir);
const label = repoDir.split('/').pop();
const repo = await api('POST', '/repos', { path: repoDir, display_name: label });
const project = await api('POST', '/projects', {
  name: `Agent demo ${label}`,
  id_prefix: 'AGD',
  repo_ids: [repo.id],
});
const issue = await api('POST', `/projects/${project.id}/issues`, {
  title: 'Agent demo',
  description: 'Add a line to demo.txt.',
});
console.log('ticket', issue.id);

async function run(title, patch, expect) {
  console.log(`\n== ${title}`);
  const updated = { ...agent, ...patch };
  await api('PUT', `/agent-library/agents/${agent.name}`, updated);
  const before = mockLines().length;
  const { workspace, execution_process: exec } = await api('POST', '/workspaces/start', {
    name: title,
    repos: [{ repo_id: repo.id, target_branch: 'main' }],
    linked_issue: { issue_id: issue.id },
    executor_config: { executor: 'CLAUDE_CODE', agent_definition: agent.name },
    prompt: 'Follow the scripted steps.',
    attachment_ids: null,
  });
  let process_ = exec;
  const started = Date.now();
  while (process_.status === 'running' && Date.now() - started < 5 * 60_000) {
    await new Promise((r) => setTimeout(r, 2000));
    process_ = await api('GET', `/execution-processes/${exec.id}`);
  }
  await new Promise((r) => setTimeout(r, 3000));
  const events = await api('GET', `/execution-processes/${exec.id}/events`);
  const calls = events.filter((e) => e.event_type === 'model_call');
  const lines = mockLines().slice(before);
  const withTools = lines.filter((l) => l.tools.length > 0);
  const seen = new Set(withTools.flatMap((l) => l.system_markers));
  const anywhere = new Set(withTools.flatMap((l) => l.markers));
  console.log(
    `  status=${process_.status} agent=${process_.agent_def_id} model_calls=${calls.length} ` +
      `models=${[...new Set(calls.map((c) => c.payload.model))].join(',')} ` +
      `provider=${[...new Set(calls.map((c) => c.payload.provider))].join(',')} ` +
      `cost=$${process_.cost_usd.toFixed(4)} budget=$${process_.budget_usd}`
  );
  console.log(`  system prompt markers: ${[...seen].join(', ') || '(none)'}`);
  console.log(`  tools: ${withTools[0]?.tools.join(',')}`);
  check(process_.status === 'completed', 'run completed');
  check(process_.agent_def_id === agent.name, 'run recorded its agent definition');
  check(calls.length > 0, 'model calls went through the proxy');
  for (const marker of expect.system) {
    check(seen.has(marker), `system prompt has ${marker}`);
  }
  for (const marker of expect.absent ?? []) {
    check(!seen.has(marker), `system prompt no longer has ${marker}`);
  }
  check(
    anywhere.has('Imported from a zip in the agent demo'),
    'imported skill readable in the container'
  );
  check(
    lines.every((l) => !l.auth || /mock-(upstream|openai)-key/.test(l.auth)),
    'upstream saw the real key, never the run token'
  );
  const ws = await api('GET', `/workspaces/${workspace.id}`);
  const log = sh(`git log --oneline -1 ${workspace.branch}`, `${ws.container_ref}/${label}`);
  console.log(`  host branch ${workspace.branch}: ${log}`);
  check(log.includes('AGD: agent demo run'), 'commit fetched back to the host branch');
}

const v1 = { system: ['ROLE-DEMO', 'STYLE-DEMO-V1'] };
await run('Claude Code', { runtime: 'claude-code', model: 'sonnet' }, v1);
await run('OpenCode + Anthropic', { runtime: 'opencode', model: 'sonnet' }, v1);
await run('OpenCode + OpenAI-compatible', { runtime: 'opencode', model: 'openai/gpt-4.1-mini' }, v1);

console.log('\n== Edit the style block');
await api('PUT', `/agent-library/styles/${style}`, {
  name: style,
  body: 'STYLE-DEMO-V2: write short commit messages.',
});
const v2 = { system: ['ROLE-DEMO', 'STYLE-DEMO-V2'], absent: ['STYLE-DEMO-V1'] };
await run('Claude Code after style edit', { runtime: 'claude-code', model: 'sonnet' }, v2);
await run('OpenCode after style edit', { runtime: 'opencode', model: 'sonnet' }, v2);

console.log(failures === 0 ? '\nALL PASS' : `\n${failures} FAILED`);
process.exit(failures === 0 ? 0 : 1);
