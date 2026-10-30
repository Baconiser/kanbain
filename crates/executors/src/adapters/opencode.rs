//! OpenCode adapter (verified against opencode-ai 1.4.7; see `cli_drift`
//! tests).
//!
//! Everything goes into `OPENCODE_CONFIG_CONTENT`: providers pointing at the
//! KanbAIn model proxy (OpenCode has no base-URL env var), one primary agent
//! with `steps` and the fine-grained `permission` map, and `instructions`
//! naming a file with the role and styles. Instructions are appended to
//! OpenCode's own system prompt, like Claude Code's `--append-system-prompt`.
//!
//! Permissions only deny what the agent may not do. Allowed tools inherit
//! the run's global permission, so supervised runs still ask.

use std::collections::HashMap;

use serde_json::{Map, Value, json};

use super::{
    AGENT_DIR, AgentLaunch, Capability, KANBAIN_MCP, LaunchFile, ResolvedAgent, RunSettings,
    RuntimeAdapter, resolve_alias,
};
use crate::executors::BaseCodingAgent;

/// Provider id for the OpenAI-compatible upstream behind the proxy.
pub const OPENAI_PROVIDER_ID: &str = "kanbain-openai";

pub struct OpencodeAdapter;

/// `provider/model` for OpenCode.
pub fn opencode_model(model: &str) -> String {
    if let Some(rest) = model.strip_prefix("openai/") {
        format!("{OPENAI_PROVIDER_ID}/{rest}")
    } else if model.contains('/') {
        model.to_string()
    } else {
        format!("anthropic/{}", resolve_alias(model))
    }
}

/// Where agents hand in their results (`sandbox::OUT_DIR`; executors
/// doesn't depend on the sandbox crate).
const OUT_DIR: &str = "/kanbain/out";

fn permissions(agent: &ResolvedAgent) -> Map<String, Value> {
    let def = &agent.definition;
    let deny = || Value::String("deny".into());
    let mut permission = Map::new();
    if !def.has(Capability::Read) {
        for key in ["read", "glob", "grep", "list", "lsp", "task"] {
            permission.insert(key.into(), deny());
        }
    }
    if !def.has(Capability::Edit) {
        if def.has(Capability::Read) {
            // Read-only agents still hand in results under /kanbain/out.
            // OpenCode matches edits by path relative to the worktree
            // (`../../kanbain/out/plan.json`), so the pattern is depth-free;
            // the later rule wins.
            let mut patterns = Map::new();
            patterns.insert("*".into(), deny());
            patterns.insert("*kanbain/out/*".into(), Value::String("allow".into()));
            permission.insert("edit".into(), Value::Object(patterns));
            let mut dirs = Map::new();
            dirs.insert(format!("{OUT_DIR}/*"), Value::String("allow".into()));
            permission.insert("external_directory".into(), Value::Object(dirs));
        } else {
            permission.insert("edit".into(), deny());
        }
    }
    if let Some(allow) = def.bash_filter() {
        let mut patterns = Map::new();
        patterns.insert("*".into(), deny());
        for pattern in allow {
            patterns.insert(pattern, Value::String("allow".into()));
        }
        permission.insert("bash".into(), Value::Object(patterns));
    } else if !def.has(Capability::Bash) {
        permission.insert("bash".into(), deny());
    }
    if !def.has(Capability::Web) {
        for key in ["webfetch", "websearch", "codesearch"] {
            permission.insert(key.into(), deny());
        }
    }
    if agent.skills.is_empty() {
        permission.insert("skill".into(), deny());
    }
    permission
}

impl RuntimeAdapter for OpencodeAdapter {
    fn launch(&self, agent: &ResolvedAgent, run: &RunSettings) -> AgentLaunch {
        let def = &agent.definition;
        let model = opencode_model(&def.model);
        let runtime_agent = format!("kanbain-{}", def.name);
        let mut files = Vec::new();
        let mut config = Map::new();
        config.insert("$schema".into(), json!("https://opencode.ai/config.json"));
        config.insert("share".into(), json!("disabled"));
        config.insert("autoupdate".into(), json!(false));

        let prompt = agent.system_prompt();
        if !prompt.is_empty() {
            let path = format!("{AGENT_DIR}/instructions.md");
            config.insert("instructions".into(), json!([format!("/kanbain/{path}")]));
            files.push(LaunchFile {
                path,
                contents: prompt.into_bytes(),
                executable: false,
            });
        }

        // Real credentials stay in the proxy; these are the run's token and
        // the proxy URLs the sandbox sets for every run.
        let mut providers = Map::new();
        providers.insert(
            "anthropic".into(),
            json!({"options": {
                "baseURL": "{env:ANTHROPIC_BASE_URL}/v1",
                "apiKey": "{env:ANTHROPIC_AUTH_TOKEN}"
            }}),
        );
        if let Some(openai_model) = model.strip_prefix(&format!("{OPENAI_PROVIDER_ID}/")) {
            providers.insert(
                OPENAI_PROVIDER_ID.into(),
                json!({
                    "npm": "@ai-sdk/openai-compatible",
                    "name": "OpenAI-compatible (KanbAIn proxy)",
                    "options": {
                        "baseURL": "{env:OPENAI_BASE_URL}",
                        "apiKey": "{env:OPENAI_API_KEY}"
                    },
                    "models": { openai_model: { "name": openai_model, "tool_call": true } }
                }),
            );
        }
        config.insert("provider".into(), Value::Object(providers));

        let mut agent_config = Map::new();
        agent_config.insert("mode".into(), json!("primary"));
        agent_config.insert("model".into(), json!(model));
        if let Some(description) = &def.description {
            agent_config.insert("description".into(), json!(description));
        }
        if let Some(steps) = def.limits.max_steps {
            agent_config.insert("steps".into(), json!(steps));
        }
        agent_config.insert("permission".into(), Value::Object(permissions(agent)));
        config.insert(
            "agent".into(),
            json!({ runtime_agent.clone(): Value::Object(agent_config) }),
        );

        let mut mcp = Map::new();
        if let Some(servers) = &def.mcp_servers {
            for (name, server) in servers {
                mcp.insert(name.clone(), mcp_server(server));
            }
        }
        if def.has(Capability::Search) {
            mcp.insert(
                KANBAIN_MCP.into(),
                json!({
                    "type": "remote",
                    "url": "{env:ANTHROPIC_BASE_URL}/mcp",
                    "headers": {"Authorization": "Bearer {env:ANTHROPIC_AUTH_TOKEN}"},
                    "enabled": true
                }),
            );
        }
        config.insert("mcp".into(), Value::Object(mcp));

        // The proxy enforces the budget; OpenCode has no client-side limit.
        let _ = run;

        AgentLaunch {
            agent: def.name.clone(),
            executor: BaseCodingAgent::Opencode,
            model,
            args: vec![],
            env: HashMap::from([
                (
                    "OPENCODE_CONFIG_CONTENT".to_string(),
                    Value::Object(config).to_string(),
                ),
                // The bundled model catalogue is enough, and models.dev is
                // not on the egress allowlist.
                ("OPENCODE_DISABLE_MODELS_FETCH".to_string(), "1".to_string()),
                ("OPENCODE_DISABLE_AUTOUPDATE".to_string(), "1".to_string()),
            ]),
            files,
            skills: agent.skills.clone(),
            runtime_agent: Some(runtime_agent),
        }
    }
}

/// Claude Code's `mcpServers` entry in OpenCode's `mcp` shape.
fn mcp_server(server: &Value) -> Value {
    if let Some(url) = server.get("url").and_then(Value::as_str) {
        let mut remote = json!({"type": "remote", "url": url, "enabled": true});
        if let Some(headers) = server.get("headers") {
            remote["headers"] = headers.clone();
        }
        return remote;
    }
    let mut command = vec![server.get("command").cloned().unwrap_or(json!(""))];
    if let Some(args) = server.get("args").and_then(Value::as_array) {
        command.extend(args.iter().cloned());
    }
    let mut local = json!({"type": "local", "command": command, "enabled": true});
    if let Some(env) = server.get("env") {
        local["environment"] = env.clone();
    }
    local
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::{AgentDefinition, StyleBlock};

    fn launch(yaml: &str) -> (AgentLaunch, Value) {
        let agent = ResolvedAgent {
            definition: AgentDefinition::from_yaml(yaml).unwrap(),
            styles: vec![StyleBlock {
                name: "concise".into(),
                body: "Be concise.".into(),
            }],
            skills: vec![],
        };
        let launch = OpencodeAdapter.launch(&agent, &RunSettings::default());
        let config = serde_json::from_str(&launch.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
        (launch, config)
    }

    #[test]
    fn coder_config() {
        let (launch, config) = launch(
            "name: backend-coder\nrole: coder\nruntime: opencode\nmodel: sonnet\n\
             role_prompt: You code.\nstyles: [concise]\ncapabilities: [read, edit, bash]\n\
             limits: { max_steps: 40 }\n",
        );
        assert_eq!(launch.model, "anthropic/claude-sonnet-4-6");
        assert_eq!(
            launch.runtime_agent.as_deref(),
            Some("kanbain-backend-coder")
        );
        let agent = &config["agent"]["kanbain-backend-coder"];
        assert_eq!(agent["mode"], "primary");
        assert_eq!(agent["model"], "anthropic/claude-sonnet-4-6");
        assert_eq!(agent["steps"], 40);
        assert_eq!(
            agent["permission"],
            json!({"webfetch": "deny", "websearch": "deny", "codesearch": "deny", "skill": "deny"})
        );
        assert_eq!(
            config["provider"]["anthropic"]["options"]["baseURL"],
            "{env:ANTHROPIC_BASE_URL}/v1"
        );
        assert_eq!(
            config["instructions"],
            json!(["/kanbain/agent/instructions.md"])
        );
        assert_eq!(launch.files[0].contents, b"You code.\n\nBe concise.");
    }

    #[test]
    fn tester_bash_patterns_and_openai_provider() {
        let (launch, config) = launch(
            "name: t\nrole: tester\nruntime: opencode\nmodel: openai/gpt-4.1-mini\n\
             capabilities: [read, edit, test]\nbash_allow: ['npm test*']\n",
        );
        assert_eq!(launch.model, "kanbain-openai/gpt-4.1-mini");
        let agent = &config["agent"]["kanbain-t"];
        assert_eq!(
            agent["permission"]["bash"],
            json!({"*": "deny", "npm test*": "allow"})
        );
        let provider = &config["provider"]["kanbain-openai"];
        assert_eq!(provider["npm"], "@ai-sdk/openai-compatible");
        assert_eq!(provider["options"]["baseURL"], "{env:OPENAI_BASE_URL}");
        assert!(provider["models"]["gpt-4.1-mini"].is_object());
    }

    #[test]
    fn read_only_agent_edits_only_out() {
        let (_, config) = launch(
            "name: p\nrole: planner\nruntime: opencode\nmodel: opus\ncapabilities: [read]\n",
        );
        let permission = &config["agent"]["kanbain-p"]["permission"];
        assert_eq!(
            permission["edit"],
            json!({"*": "deny", "*kanbain/out/*": "allow"})
        );
        assert_eq!(
            permission["external_directory"],
            json!({"/kanbain/out/*": "allow"})
        );
        // Agents without any tools stay without them.
        let (_, config) =
            launch("name: i\nrole: intake\nruntime: opencode\nmodel: haiku\ncapabilities: []\n");
        assert_eq!(config["agent"]["kanbain-i"]["permission"]["edit"], "deny");
    }

    #[test]
    fn maps_mcp_servers() {
        assert_eq!(
            mcp_server(&json!({"command": "npx", "args": ["-y", "x"], "env": {"A": "1"}})),
            json!({"type": "local", "command": ["npx", "-y", "x"], "enabled": true, "environment": {"A": "1"}})
        );
        assert_eq!(
            mcp_server(&json!({"type": "http", "url": "https://m"})),
            json!({"type": "remote", "url": "https://m", "enabled": true})
        );
    }
}
