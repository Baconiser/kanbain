//! Runtime-neutral agent definitions (D31), style blocks and skills.
//!
//! These are the files users manage in the dashboard: `agents/<name>.yaml`,
//! `styles/<name>.md` and `skills/<name>/SKILL.md` in the data folder.

use serde::{Deserialize, Serialize};
use strum_macros::{AsRefStr, Display, EnumIter, EnumString};
use ts_rs::TS;

/// What an agent is for in the pipeline.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    TS,
    Display,
    EnumString,
    AsRefStr,
    EnumIter,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Coder,
    Tester,
    Scribe,
    Intake,
    History,
}

/// Which CLI runs the agent.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    TS,
    Display,
    EnumString,
    AsRefStr,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
pub enum AgentRuntime {
    ClaudeCode,
    Opencode,
}

/// Tool groups an agent may use. Each adapter maps them to its runtime's
/// tools and permissions.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    TS,
    Display,
    EnumString,
    AsRefStr,
    EnumIter,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum Capability {
    /// Read, search and list files.
    Read,
    /// Create and change files.
    Edit,
    /// Any shell command.
    Bash,
    /// Shell commands matching `bash_allow` only (test runners by default).
    Test,
    /// Fetch web pages and search the web (through the egress allowlist).
    Web,
    /// KanbAIn's `search_knowledge` tool over the project's tickets, runs
    /// and code (E22).
    Search,
}

impl AgentRole {
    pub fn all() -> Vec<Self> {
        <Self as strum::IntoEnumIterator>::iter().collect()
    }
}

impl Capability {
    pub fn all() -> Vec<Self> {
        <Self as strum::IntoEnumIterator>::iter().collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentLimits {
    /// Agentic turns before the runtime stops (`--max-turns` / `steps`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    /// Model spend per run in USD. The model proxy enforces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_usd: Option<f64>,
    /// Minutes of agent activity per run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_minutes: Option<u32>,
}

/// A neutral agent definition, stored as `agents/<name>.yaml`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AgentDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub role: AgentRole,
    pub runtime: AgentRuntime,
    /// `opus`, `sonnet`, `haiku`, a full model id, or `openai/<model>` for
    /// the OpenAI-compatible provider (OpenCode only).
    pub model: String,
    /// The role layer of the system prompt.
    #[serde(default)]
    pub role_prompt: String,
    /// Style blocks appended to the system prompt, in order.
    #[serde(default)]
    pub styles: Vec<String>,
    /// Skills available in the container.
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default = "default_capabilities")]
    pub capabilities: Vec<Capability>,
    /// Shell glob patterns allowed with the `test` capability, e.g.
    /// `cargo test*`. Empty means the default test runners.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bash_allow: Vec<String>,
    #[serde(default)]
    pub limits: AgentLimits,
    /// MCP servers for this agent, in Claude Code's `mcpServers` shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(type = "Record<string, unknown> | null")]
    pub mcp_servers: Option<serde_json::Map<String, serde_json::Value>>,
}

fn default_capabilities() -> Vec<Capability> {
    vec![Capability::Read]
}

/// Commands the `test` capability allows when an agent lists none.
pub const DEFAULT_TEST_COMMANDS: &[&str] = &[
    "cargo test*",
    "cargo nextest*",
    "cargo check*",
    "npm test*",
    "npm run test*",
    "pnpm test*",
    "pnpm run test*",
    "pnpm vitest*",
    "yarn test*",
    "npx vitest*",
    "npx jest*",
    "npx playwright test*",
    "pytest*",
    "python -m pytest*",
    "python3 -m pytest*",
    "go test*",
    "git status*",
    "git diff*",
    "git log*",
    "git show*",
    "ls*",
    "cat *",
];

#[derive(Debug, thiserror::Error)]
pub enum DefinitionError {
    #[error("invalid name `{0}`: use lowercase letters, digits and dashes")]
    InvalidName(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Yaml(#[from] serde_yaml_ng::Error),
}

/// Names are file names and CLI agent names, so keep them simple.
pub fn validate_name(name: &str) -> Result<(), DefinitionError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        && !name.starts_with('-');
    if valid {
        Ok(())
    } else {
        Err(DefinitionError::InvalidName(name.to_string()))
    }
}

impl AgentDefinition {
    pub fn from_yaml(text: &str) -> Result<Self, DefinitionError> {
        let def: Self = serde_yaml_ng::from_str(text)?;
        def.validate()?;
        Ok(def)
    }

    pub fn to_yaml(&self) -> Result<String, DefinitionError> {
        Ok(serde_yaml_ng::to_string(self)?)
    }

    pub fn validate(&self) -> Result<(), DefinitionError> {
        validate_name(&self.name)?;
        for style in &self.styles {
            validate_name(style)?;
        }
        for skill in &self.skills {
            validate_name(skill)?;
        }
        if self.model.trim().is_empty() {
            return Err(DefinitionError::Invalid("model is required".into()));
        }
        if self.runtime == AgentRuntime::ClaudeCode && self.model.starts_with("openai/") {
            return Err(DefinitionError::Invalid(
                "Claude Code only runs Anthropic models; use the opencode runtime for openai/ models"
                    .into(),
            ));
        }
        if let Some(budget) = self.limits.budget_usd
            && (budget.is_nan() || budget <= 0.0)
        {
            return Err(DefinitionError::Invalid(
                "budget_usd must be positive".into(),
            ));
        }
        Ok(())
    }

    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Commands the bash filter lets through, or `None` when bash is
    /// unrestricted or unavailable.
    pub fn bash_filter(&self) -> Option<Vec<String>> {
        if self.has(Capability::Bash) || !self.has(Capability::Test) {
            return None;
        }
        Some(if self.bash_allow.is_empty() {
            DEFAULT_TEST_COMMANDS
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            self.bash_allow.clone()
        })
    }
}

/// A reusable block appended to agents' system prompts, stored as
/// `styles/<name>.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct StyleBlock {
    pub name: String,
    pub body: String,
}

/// One file of a skill, relative to the skill folder.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFile {
    pub path: String,
    pub contents: Vec<u8>,
}

/// A skill folder (`skills/<name>/SKILL.md` plus supporting files).
#[derive(Debug, Clone, PartialEq)]
pub struct Skill {
    pub name: String,
    pub files: Vec<SkillFile>,
}

/// `name` and `description` from a `SKILL.md` frontmatter block.
pub fn skill_frontmatter(skill_md: &str) -> (Option<String>, Option<String>) {
    let Some(rest) = skill_md.strip_prefix("---") else {
        return (None, None);
    };
    let Some(end) = rest.find("\n---") else {
        return (None, None);
    };
    let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&rest[..end]) else {
        return (None, None);
    };
    let field = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };
    (field("name"), field("description"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plan_example() {
        let yaml = r#"
name: backend-coder
role: coder
runtime: claude-code
model: sonnet
role_prompt: "You are a backend developer..."
styles: [concise-commits, project-conventions]
skills: [some-skill]
capabilities: [read, edit, bash, test]
limits: { max_steps: 40, budget_usd: 2, wall_minutes: 30 }
"#;
        let def = AgentDefinition::from_yaml(yaml).unwrap();
        assert_eq!(def.role, AgentRole::Coder);
        assert_eq!(def.runtime, AgentRuntime::ClaudeCode);
        assert_eq!(def.limits.max_steps, Some(40));
        assert_eq!(def.limits.budget_usd, Some(2.0));
        assert_eq!(def.bash_filter(), None);
        let round = AgentDefinition::from_yaml(&def.to_yaml().unwrap()).unwrap();
        assert_eq!(round, def);
    }

    #[test]
    fn test_capability_gets_default_filter() {
        let def = AgentDefinition::from_yaml(
            "name: t\nrole: tester\nruntime: opencode\nmodel: sonnet\ncapabilities: [read, edit, test]\n",
        )
        .unwrap();
        let filter = def.bash_filter().unwrap();
        assert!(filter.contains(&"cargo test*".to_string()));
    }

    #[test]
    fn rejects_bad_names_and_openai_on_claude() {
        assert!(validate_name("Backend").is_err());
        assert!(validate_name("../x").is_err());
        assert!(validate_name("backend-coder").is_ok());
        let err = AgentDefinition::from_yaml(
            "name: x\nrole: coder\nruntime: claude-code\nmodel: openai/gpt-5\n",
        );
        assert!(err.is_err());
    }

    #[test]
    fn reads_skill_frontmatter() {
        let md = "---\nname: pdf\ndescription: Work with PDFs\n---\n# PDF\n";
        assert_eq!(
            skill_frontmatter(md),
            (Some("pdf".into()), Some("Work with PDFs".into()))
        );
        assert_eq!(skill_frontmatter("# no frontmatter"), (None, None));
    }
}
