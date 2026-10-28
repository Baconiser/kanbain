// End-to-end sandbox run through the backend API.
// Usage: node sandbox-run.mjs <backendPort> <token> <repoDir> <label> [budgetUsd|-] [agent]
// Creates a scratch repo, project and ticket, starts a Claude Code workspace
// run (or a run of the named agent definition) and prints the run record,
// its events and the host branch afterwards.
import { execSync } from 'node:child_process';

const [port, token, repoDir, label, budgetArg, agent] = process.argv.slice(2);
const budget = budgetArg && budgetArg !== '-' ? budgetArg : undefined;
const base = `http://127.0.0.1:${port}/api`;

async function api(method, path, body) {
  const res = await fetch(base + path, {
    method,
    headers: { 'content-type': 'application/json', 'x-kanbain-token': token },
    body: body ? JSON.stringify(body) : undefined,
  });
  const json = await res.json().catch(() => ({}));
  if (!res.ok || json.success === false) {
    throw new Error(`${method} ${path} -> ${res.status} ${JSON.stringify(json)}`);
  }
  return json.data;
}

const sh = (cmd, cwd) => execSync(cmd, { cwd, encoding: 'utf8' }).trim();

sh(`rm -rf ${repoDir} && mkdir -p ${repoDir}`);
sh('git init -q -b main && git config user.email demo@example.com && git config user.name Demo', repoDir);
sh('echo "# demo" > README.md && git add . && git commit -qm init', repoDir);

const repo = await api('POST', '/repos', { path: repoDir, display_name: label });
const project = await api('POST', '/projects', {
  name: `Sandbox ${label}`,
  id_prefix: 'DEMO',
  repo_ids: [repo.id],
});
const issue = await api('POST', `/projects/${project.id}/issues`, {
  title: `Sandbox ${label}`,
  description: 'Add sandbox.txt from inside the container.',
  ...(budget ? { budget_usd: Number(budget) } : {}),
});
console.log('ticket', issue.id, 'budget', issue.budget_usd);

const started = Date.now();
const { workspace, execution_process: exec } = await api('POST', '/workspaces/start', {
  name: `sandbox-${label}`,
  repos: [{ repo_id: repo.id, target_branch: 'main' }],
  linked_issue: { issue_id: issue.id },
  executor_config: {
    executor: 'CLAUDE_CODE',
    ...(agent ? { agent_definition: agent } : {}),
  },
  prompt: 'Follow the scripted steps.',
  attachment_ids: null,
});
console.log('workspace', workspace.id, 'branch', workspace.branch, 'exec', exec.id);

let process_ = exec;
while (process_.status === 'running' && Date.now() - started < 10 * 60_000) {
  await new Promise((r) => setTimeout(r, 2000));
  process_ = await api('GET', `/execution-processes/${exec.id}`);
}
// Exit monitor finishes sync-out after the status flips; give it a moment.
await new Promise((r) => setTimeout(r, 4000));
process_ = await api('GET', `/execution-processes/${exec.id}`);
const events = await api('GET', `/execution-processes/${exec.id}/events`);

console.log(
  JSON.stringify(
    {
      status: process_.status,
      agent_def_id: process_.agent_def_id,
      exit_code: process_.exit_code,
      issue_id: process_.issue_id,
      container_id: process_.container_id?.slice(0, 12),
      branch: process_.branch,
      tokens_in: process_.tokens_in,
      tokens_out: process_.tokens_out,
      cost_usd: process_.cost_usd,
      budget_usd: process_.budget_usd,
      seconds: Math.round((Date.now() - started) / 1000),
    },
    null,
    2,
  ),
);
const counts = {};
for (const e of events) counts[e.event_type] = (counts[e.event_type] ?? 0) + 1;
console.log('events', JSON.stringify(counts));
for (const e of events.filter((e) => e.event_type !== 'model_call')) {
  console.log(' ', e.event_type, JSON.stringify(e.payload).slice(0, 300));
}
const call = events.find((e) => e.event_type === 'model_call');
if (call) console.log('  first model_call', JSON.stringify(call.payload));

const ws = await api('GET', `/workspaces/${workspace.id}`);
const worktree = `${ws.container_ref}/${label}`;
console.log('host worktree log:\n' + sh(`git log --oneline -5 ${workspace.branch}`, worktree));
console.log('host files:', sh('ls', worktree).split('\n').join(' '));
const issueAfter = await api('GET', `/issues/${issue.id}`);
console.log('ticket status id', issueAfter.status_id);
console.log(JSON.stringify({ workspace_id: workspace.id, issue_id: issue.id, project_id: project.id }));
