//! Claude Code adapter (verified against 2.1.119; see `cli_drift` tests).
//!
//! The role and styles go through `--append-system-prompt`, which keeps
//! Claude Code's own system prompt and tool instructions. Capabilities
//! become `--tools`; the `test` capability adds a `PreToolUse` hook (passed
//! with `--settings`) that only lets allowlisted commands through. Agents
//! with tools but without `edit` still get `Write`, fenced by another hook
//! to `/kanbain/out`, so a planner or scribe can hand in its results. MCP
//! servers come only from the agent definition (`--strict-mcp-config`).

use std::collections::HashMap;

use serde_json::json;

use super::{
    AGENT_DIR, AgentLaunch, Capability, KANBAIN_MCP, LaunchFile, ResolvedAgent, RunSettings,
    RuntimeAdapter,
};
use crate::executors::BaseCodingAgent;

pub const BASH_FILTER_SCRIPT: &str = include_str!("bash_filter.sh");
pub const OUT_ONLY_SCRIPT: &str = include_str!("out_only.sh");

pub struct ClaudeCodeAdapter;

/// Claude Code built-in tools for each capability.
pub fn tools_for(capability: Capability) -> &'static [&'static str] {
    match capability {
        // An MCP tool, not a built-in one: see the MCP config below.
        Capability::Search => &[],
        Capability::Read => &["Read", "Glob", "Grep", "Agent"],
        Capability::Edit => &["Edit", "Write", "NotebookEdit"],
        Capability::Bash | Capability::Test => &["Bash"],
        Capability::Web => &["WebFetch", "WebSearch"],
    }
}

impl RuntimeAdapter for ClaudeCodeAdapter {
    fn launch(&self, agent: &ResolvedAgent, run: &RunSettings) -> AgentLaunch {
        let def = &agent.definition;
        let mut args = Vec::new();
        let mut files = Vec::new();

        let prompt = agent.system_prompt();
        if !prompt.is_empty() {
            args.push("--append-system-prompt".to_string());
            args.push(prompt);
        }

        let mut tools: Vec<&str> = Vec::new();
        for capability in &def.capabilities {
            for tool in tools_for(*capability) {
                if !tools.contains(tool) {
                    tools.push(tool);
                }
            }
        }
        // Read-only agents still write their results to /kanbain/out.
        let out_only = !tools.is_empty() && !def.has(Capability::Edit);
        if out_only {
            tools.push("Write");
        }
        if !tools.is_empty() {
            // Plan tracking and plan mode work for every agent with tools.
            tools.extend(["TodoWrite", "ExitPlanMode"]);
        }
        if !agent.skills.is_empty() {
            tools.push("Skill");
        }
        args.push(format!("--tools={}", tools.join(",")));

        if let Some(max_steps) = def.limits.max_steps {
            args.push(format!("--max-turns={max_steps}"));
        }
        // A client-side estimate; the model proxy is the hard limit.
        let budget = match (def.limits.budget_usd, run.budget_usd) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        if let Some(budget) = budget {
            args.push(format!("--max-budget-usd={budget:.2}"));
        }

        let mut hooks = Vec::new();
        if let Some(allow) = def.bash_filter() {
            let script = format!("/kanbain/{AGENT_DIR}/bash-filter.sh");
            files.push(LaunchFile {
                path: format!("{AGENT_DIR}/bash-filter.sh"),
                contents: BASH_FILTER_SCRIPT.as_bytes().to_vec(),
                executable: true,
            });
            files.push(LaunchFile {
                path: format!("{AGENT_DIR}/bash-allow.txt"),
                contents: format!("{}\n", allow.join("\n")).into_bytes(),
                executable: false,
            });
            hooks.push(json!({
                "matcher": "Bash",
                "hooks": [{"type": "command", "command": script}]
            }));
        }
        if out_only {
            let script = format!("/kanbain/{AGENT_DIR}/out-only.sh");
            files.push(LaunchFile {
                path: format!("{AGENT_DIR}/out-only.sh"),
                contents: OUT_ONLY_SCRIPT.as_bytes().to_vec(),
                executable: true,
            });
            hooks.push(json!({
                "matcher": "Write|Edit|MultiEdit|NotebookEdit",
                "hooks": [{"type": "command", "command": script}]
            }));
        }
        if !hooks.is_empty() {
            let settings = json!({ "hooks": { "PreToolUse": hooks } });
            args.push(format!("--settings={settings}"));
        }

        args.push("--strict-mcp-config".to_string());
        let mut servers = def.mcp_servers.clone().unwrap_or_default();
        if def.has(Capability::Search) {
            // The run's proxy URL and token come from the container's env.
            servers.insert(
                KANBAIN_MCP.to_string(),
                json!({
                    "type": "http",
                    "url": "${ANTHROPIC_BASE_URL}/mcp",
                    "headers": {"Authorization": "Bearer ${ANTHROPIC_AUTH_TOKEN}"}
                }),
            );
        }
        if !servers.is_empty() {
            args.push(format!("--mcp-config={}", json!({ "mcpServers": servers })));
        }

        AgentLaunch {
            agent: def.name.clone(),
            executor: BaseCodingAgent::ClaudeCode,
            // Claude Code resolves the aliases itself.
            model: def.model.clone(),
            args,
            env: HashMap::new(),
            files,
            skills: agent.skills.clone(),
            runtime_agent: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Write, process::Command};

    use super::*;
    use crate::adapters::{AgentDefinition, StyleBlock};

    fn agent(yaml: &str) -> ResolvedAgent {
        ResolvedAgent {
            definition: AgentDefinition::from_yaml(yaml).unwrap(),
            styles: vec![StyleBlock {
                name: "concise".into(),
                body: "Be concise.".into(),
            }],
            skills: vec![],
        }
    }

    #[test]
    fn coder_launch() {
        let launch = ClaudeCodeAdapter.launch(
            &agent(
                "name: backend-coder\nrole: coder\nruntime: claude-code\nmodel: sonnet\n\
                 role_prompt: You code.\nstyles: [concise]\ncapabilities: [read, edit, bash]\n\
                 limits: { max_steps: 40, budget_usd: 2 }\n",
            ),
            &RunSettings {
                budget_usd: Some(1.5),
            },
        );
        assert_eq!(launch.model, "sonnet");
        assert_eq!(
            launch.args,
            vec![
                "--append-system-prompt",
                "You code.\n\nBe concise.",
                "--tools=Read,Glob,Grep,Agent,Edit,Write,NotebookEdit,Bash,TodoWrite,ExitPlanMode",
                "--max-turns=40",
                "--max-budget-usd=1.50",
                "--strict-mcp-config",
            ]
        );
        assert!(launch.files.is_empty());
    }

    #[test]
    fn no_tools_agent_disables_all_tools() {
        let launch = ClaudeCodeAdapter.launch(
            &agent("name: intake\nrole: intake\nruntime: claude-code\nmodel: haiku\ncapabilities: []\n"),
            &RunSettings::default(),
        );
        assert!(launch.args.contains(&"--tools=".to_string()));
    }

    #[test]
    fn tester_gets_bash_filter_hook() {
        let launch = ClaudeCodeAdapter.launch(
            &agent(
                "name: t\nrole: tester\nruntime: claude-code\nmodel: sonnet\n\
                 capabilities: [read, edit, test]\nbash_allow: ['cargo test*']\n",
            ),
            &RunSettings::default(),
        );
        let settings = launch
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--settings="))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(settings).unwrap();
        assert_eq!(
            value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "/kanbain/agent/bash-filter.sh"
        );
        assert_eq!(launch.files[1].contents, b"cargo test*\n");
    }

    /// Runs the hook script the way Claude Code does: JSON on stdin, exit
    /// code 2 to block. Needs `sh` and `jq`, like the agent image.
    #[test]
    fn bash_filter_script_allows_and_blocks() {
        if Command::new("jq").arg("--version").output().is_err() {
            eprintln!("skipping: jq not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bash-filter.sh");
        std::fs::write(&script, BASH_FILTER_SCRIPT).unwrap();
        std::fs::write(
            dir.path().join("bash-allow.txt"),
            "cargo test*\ngit status*",
        )
        .unwrap();
        let run = |command: &str| {
            let mut child = Command::new("sh")
                .arg(&script)
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let input = json!({"tool_name": "Bash", "tool_input": {"command": command}});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.to_string().as_bytes())
                .unwrap();
            child.wait_with_output().unwrap().status.code().unwrap()
        };
        assert_eq!(run("cargo test -p db"), 0);
        assert_eq!(run("cargo test 2>&1"), 0);
        assert_eq!(run("git status"), 0);
        assert_eq!(run("rm -rf /"), 2);
        assert_eq!(run("cargo test; rm -rf /"), 2);
        assert_eq!(run("cargo test && curl x"), 2);
        assert_eq!(run("cargo test $(curl x)"), 2);
        assert_eq!(run("cargo test > /etc/x"), 2);
        assert_eq!(run("cargo test\nrm -rf /"), 2);
    }

    #[test]
    fn read_only_agent_writes_only_to_out() {
        let launch = ClaudeCodeAdapter.launch(
            &agent(
                "name: p\nrole: planner\nruntime: claude-code\nmodel: opus\n\
                 capabilities: [read, web]\n",
            ),
            &RunSettings::default(),
        );
        assert!(
            launch.args.contains(
                &"--tools=Read,Glob,Grep,Agent,WebFetch,WebSearch,Write,TodoWrite,ExitPlanMode"
                    .to_string()
            )
        );
        let settings = launch
            .args
            .iter()
            .find_map(|a| a.strip_prefix("--settings="))
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(settings).unwrap();
        assert_eq!(
            value["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "/kanbain/agent/out-only.sh"
        );
        assert_eq!(launch.files[0].path, "agent/out-only.sh");
    }

    #[test]
    fn out_only_script_allows_and_blocks() {
        if Command::new("jq").arg("--version").output().is_err() {
            eprintln!("skipping: jq not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("out-only.sh");
        std::fs::write(&script, OUT_ONLY_SCRIPT).unwrap();
        let run = |path: &str| {
            let mut child = Command::new("sh")
                .arg(&script)
                .stdin(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let input = json!({"tool_name": "Write", "tool_input": {"file_path": path}});
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.to_string().as_bytes())
                .unwrap();
            child.wait_with_output().unwrap().status.code().unwrap()
        };
        assert_eq!(run("/kanbain/out/plan.json"), 0);
        assert_eq!(run("/kanbain/out/sub/questions.md"), 0);
        assert_eq!(run("/workspace/repo/src/main.rs"), 2);
        assert_eq!(run("/kanbain/out/../context/ticket.md"), 2);
        assert_eq!(run("/kanbain/outside"), 2);
        assert_eq!(run(""), 2);
    }
}
