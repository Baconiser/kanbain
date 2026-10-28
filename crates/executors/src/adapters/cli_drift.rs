//! Checks the adapters against snapshots of the pinned CLIs' interfaces
//! (`cli_snapshots/`, written by `scripts/snapshot-agent-clis.sh`). Bumping
//! a version in `versions.rs` fails these tests until the snapshots are
//! regenerated, and an adapter can't emit a flag or key the pinned CLI
//! doesn't have.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{
    AgentDefinition, ResolvedAgent, RunSettings, RuntimeAdapter, Skill, SkillFile, StyleBlock,
    claude::ClaudeCodeAdapter, opencode::OpencodeAdapter,
};
use crate::versions::{CLAUDE_CODE_VERSION, OPENCODE_VERSION};

const CLAUDE_SNAPSHOT: &str = include_str!("cli_snapshots/claude-code.txt");
const OPENCODE_SNAPSHOT: &str = include_str!("cli_snapshots/opencode.txt");

fn section(snapshot: &str, title: &str) -> BTreeSet<String> {
    snapshot
        .lines()
        .skip_while(|line| *line != format!("## {title}"))
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect()
}

/// An agent that exercises every adapter feature.
fn full_agent(runtime: &str, model: &str) -> ResolvedAgent {
    let mut definition = AgentDefinition::from_yaml(&format!(
        "name: all\ndescription: d\nrole: tester\nruntime: {runtime}\nmodel: {model}\n\
         role_prompt: p\ncapabilities: [read, edit, test]\n\
         limits: {{ max_steps: 5, budget_usd: 1 }}\n"
    ))
    .unwrap();
    definition.mcp_servers = Some(json!({"m": {"command": "x"}}).as_object().unwrap().clone());
    ResolvedAgent {
        definition,
        styles: vec![StyleBlock {
            name: "s".into(),
            body: "b".into(),
        }],
        skills: vec![Skill {
            name: "k".into(),
            files: vec![SkillFile {
                path: "SKILL.md".into(),
                contents: vec![],
            }],
        }],
    }
}

#[test]
fn claude_code_snapshot_matches_pinned_version() {
    assert_eq!(
        CLAUDE_SNAPSHOT.lines().next(),
        Some(format!("version: {CLAUDE_CODE_VERSION}").as_str()),
        "re-run scripts/snapshot-agent-clis.sh after bumping Claude Code"
    );
}

#[test]
fn claude_code_adapter_flags_exist() {
    let launch = ClaudeCodeAdapter.launch(
        &full_agent("claude-code", "sonnet"),
        &RunSettings {
            budget_usd: Some(1.0),
        },
    );
    // Flags the executor adds around the adapter's.
    let mut flags: Vec<String> = [
        "--model",
        "--permission-mode",
        "--disallowedTools",
        "--resume",
        "--output-format",
        "--input-format",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    flags.extend(
        launch
            .args
            .iter()
            .filter(|a| a.starts_with("--"))
            .map(|a| a.split('=').next().unwrap().to_string()),
    );
    for flag in flags {
        assert!(
            CLAUDE_SNAPSHOT.contains(&format!("{flag} "))
                || CLAUDE_SNAPSHOT.contains(&format!("{flag},")),
            "Claude Code {CLAUDE_CODE_VERSION} has no {flag} flag"
        );
    }
    assert!(section(CLAUDE_SNAPSHOT, "--agents fields").contains("maxTurns"));
}

#[test]
fn opencode_snapshot_matches_pinned_version() {
    assert_eq!(
        OPENCODE_SNAPSHOT.lines().next(),
        Some(format!("version: {OPENCODE_VERSION}").as_str()),
        "re-run scripts/snapshot-agent-clis.sh after bumping OpenCode"
    );
}

#[test]
fn opencode_adapter_keys_exist() {
    for model in ["sonnet", "openai/gpt-4.1-mini"] {
        let launch =
            OpencodeAdapter.launch(&full_agent("opencode", model), &RunSettings::default());
        let config: Value = serde_json::from_str(&launch.env["OPENCODE_CONFIG_CONTENT"]).unwrap();
        assert!(
            section(OPENCODE_SNAPSHOT, "config fields").contains("instructions"),
            "OpenCode {OPENCODE_VERSION} has no `instructions` config"
        );
        let agent_fields = section(OPENCODE_SNAPSHOT, "agent fields");
        let permission_keys = section(OPENCODE_SNAPSHOT, "permission keys");
        let provider_options = section(OPENCODE_SNAPSHOT, "provider options");
        for agent in config["agent"].as_object().unwrap().values() {
            for key in agent.as_object().unwrap().keys() {
                assert!(agent_fields.contains(key), "agent field `{key}` unknown");
            }
            for key in agent["permission"].as_object().unwrap().keys() {
                assert!(permission_keys.contains(key), "permission `{key}` unknown");
            }
        }
        for provider in config["provider"].as_object().unwrap().values() {
            for key in provider["options"].as_object().unwrap().keys() {
                assert!(
                    provider_options.contains(key),
                    "provider option `{key}` unknown"
                );
            }
        }
    }
}
