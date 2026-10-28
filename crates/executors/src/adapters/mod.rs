//! Runtime adapters (Phase 2): turn a neutral agent definition plus its
//! styles and skills into what one CLI run needs (arguments, environment
//! and files for the sandbox container).
//!
//! Three prompt layers (D31): the role prompt and the style blocks form the
//! system prompt, added through each runtime's "append" mechanism so the
//! runtime's own tool instructions stay intact; the task is the first user
//! message and never part of the system prompt.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub mod claude;
#[cfg(test)]
mod cli_drift;
pub mod definition;
pub mod opencode;

pub use definition::{
    AgentDefinition, AgentLimits, AgentRole, AgentRuntime, Capability, Skill, SkillFile, StyleBlock,
};

use crate::executors::BaseCodingAgent;

/// Folder inside `/kanbain` with the agent's generated files (instructions,
/// hook scripts). Read-only for the agent.
pub const AGENT_DIR: &str = "agent";
/// Folder inside `/kanbain` with the agent's skills. Read-only for the
/// agent; `~/.claude/skills` points here, which both runtimes scan.
pub const SKILLS_DIR: &str = "skills";

/// An agent definition with its style blocks and skills loaded.
#[derive(Debug, Clone)]
pub struct ResolvedAgent {
    pub definition: AgentDefinition,
    pub styles: Vec<StyleBlock>,
    pub skills: Vec<Skill>,
}

impl ResolvedAgent {
    /// Role prompt followed by the style blocks.
    pub fn system_prompt(&self) -> String {
        let mut parts = Vec::new();
        let role = self.definition.role_prompt.trim();
        if !role.is_empty() {
            parts.push(role.to_string());
        }
        for style in &self.styles {
            let body = style.body.trim();
            if !body.is_empty() {
                parts.push(body.to_string());
            }
        }
        parts.join("\n\n")
    }
}

/// Per-run facts the adapter needs.
#[derive(Debug, Clone, Default)]
pub struct RunSettings {
    /// What the model proxy will allow this run to spend, in USD.
    pub budget_usd: Option<f64>,
}

/// A file to write under `/kanbain`, owned by root.
#[derive(Debug, Clone, PartialEq)]
pub struct LaunchFile {
    /// Path relative to `/kanbain`.
    pub path: String,
    pub contents: Vec<u8>,
    pub executable: bool,
}

/// Everything one run of an agent definition needs.
#[derive(Debug, Clone)]
pub struct AgentLaunch {
    /// The agent definition's name.
    pub agent: String,
    pub executor: BaseCodingAgent,
    /// Model id in the runtime's own format.
    pub model: String,
    /// Extra CLI arguments.
    pub args: Vec<String>,
    /// Extra environment variables.
    pub env: HashMap<String, String>,
    /// Files under `/kanbain` (hook scripts, instructions).
    pub files: Vec<LaunchFile>,
    /// Skills to place under `/kanbain/skills`.
    pub skills: Vec<Skill>,
    /// Runtime-level agent to select (OpenCode's `agent` field).
    pub runtime_agent: Option<String>,
}

pub trait RuntimeAdapter {
    fn launch(&self, agent: &ResolvedAgent, run: &RunSettings) -> AgentLaunch;
}

pub fn adapter_for(runtime: AgentRuntime) -> &'static dyn RuntimeAdapter {
    match runtime {
        AgentRuntime::ClaudeCode => &claude::ClaudeCodeAdapter,
        AgentRuntime::Opencode => &opencode::OpencodeAdapter,
    }
}

pub fn executor_for(runtime: AgentRuntime) -> BaseCodingAgent {
    match runtime {
        AgentRuntime::ClaudeCode => BaseCodingAgent::ClaudeCode,
        AgentRuntime::Opencode => BaseCodingAgent::Opencode,
    }
}

/// Full Anthropic model ids behind the short aliases. Claude Code resolves
/// the aliases itself; OpenCode needs ids. Keep these at models the pinned
/// Claude Code version knows, so both runtimes run the same model.
pub const MODEL_ALIASES: &[(&str, &str)] = &[
    ("opus", "claude-opus-4-7"),
    ("sonnet", "claude-sonnet-4-6"),
    ("haiku", "claude-haiku-4-5"),
];

pub fn resolve_alias(model: &str) -> &str {
    MODEL_ALIASES
        .iter()
        .find(|(alias, _)| *alias == model)
        .map(|(_, id)| *id)
        .unwrap_or(model)
}

/// The shipped agents (D28 models), seeded into an empty data folder.
pub fn default_agents() -> Vec<AgentDefinition> {
    use AgentRole::*;
    use Capability::*;
    let agent = |name: &str,
                 role: AgentRole,
                 model: &str,
                 description: &str,
                 prompt: &str,
                 capabilities: Vec<Capability>| AgentDefinition {
        name: name.to_string(),
        description: Some(description.to_string()),
        role,
        runtime: AgentRuntime::ClaudeCode,
        model: model.to_string(),
        role_prompt: prompt.to_string(),
        styles: vec![],
        skills: vec![],
        capabilities,
        bash_allow: vec![],
        limits: AgentLimits::default(),
        mcp_servers: None,
    };
    vec![
        agent(
            "planner",
            Planner,
            "opus",
            "Reads the ticket and the code and writes a plan.",
            "You are the planner. Read the ticket in /kanbain/context and the code, then \
             write a structured plan: subtasks with the files they likely touch, acceptance \
             criteria, assumptions, a test strategy and checkpoints. Do not change code.",
            vec![Read, Web],
        ),
        agent(
            "coder",
            Coder,
            "sonnet",
            "Implements one subtask on its branch.",
            "You are a software engineer. Implement the task you are given in the repository, \
             following the project's existing conventions, and commit your work with clear \
             messages.",
            vec![Read, Edit, Bash],
        ),
        agent(
            "tester",
            Tester,
            "sonnet",
            "Writes black-box tests from the acceptance criteria.",
            "You are the tester. Write tests from the ticket's requirements and acceptance \
             criteria, not from the implementation. Only add or change test files. Report \
             problems you find as structured issues.",
            vec![Read, Edit, Test],
        ),
        agent(
            "scribe",
            Scribe,
            "haiku",
            "Writes a changelog entry for a run.",
            "You are the scribe. Summarize what changed and why, which files were touched, \
             which issues were found and fixed, and the commits involved.",
            vec![Read],
        ),
        agent(
            "intake",
            Intake,
            "haiku",
            "Turns a pasted ticket into a structured one.",
            "You turn a raw ticket into a structured one: title, clean description, acceptance \
             criteria, open questions, external ID and a priority guess. Keep the original \
             meaning; do not invent requirements.",
            vec![],
        ),
        agent(
            "history",
            History,
            "haiku",
            "Summarizes prior commits for a ticket.",
            "You summarize earlier work on a ticket from its commit messages and diffs: what \
             was done, what was left open, and what the planner should know.",
            vec![],
        ),
    ]
}

/// Settings shown in the agent editor for a runtime.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct RuntimeModelOption {
    pub id: String,
    pub label: String,
}

pub fn suggested_models(runtime: AgentRuntime) -> Vec<RuntimeModelOption> {
    let mut models: Vec<RuntimeModelOption> = MODEL_ALIASES
        .iter()
        .map(|(alias, id)| RuntimeModelOption {
            id: alias.to_string(),
            label: format!("{alias} ({id})"),
        })
        .collect();
    if runtime == AgentRuntime::Opencode {
        models.push(RuntimeModelOption {
            id: "openai/".to_string(),
            label: "openai/<model> (OpenAI-compatible provider)".to_string(),
        });
    }
    models
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_prompt_layers_role_then_styles() {
        let agent = ResolvedAgent {
            definition: default_agents().remove(1),
            styles: vec![
                StyleBlock {
                    name: "a".into(),
                    body: "Style A".into(),
                },
                StyleBlock {
                    name: "b".into(),
                    body: "  \n".into(),
                },
            ],
            skills: vec![],
        };
        let prompt = agent.system_prompt();
        assert!(prompt.starts_with("You are a software engineer."));
        assert!(prompt.ends_with("\n\nStyle A"));
    }

    #[test]
    fn default_agents_are_valid() {
        for agent in default_agents() {
            agent.validate().unwrap();
        }
    }
}
