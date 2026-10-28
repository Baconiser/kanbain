//! Pinned agent CLI versions. The executors' `npx` commands and the sandbox
//! base image both read these, so a version bump is a one-line change here.

pub const CLAUDE_CODE_PACKAGE: &str = "@anthropic-ai/claude-code";
pub const CLAUDE_CODE_VERSION: &str = "2.1.119";
pub const OPENCODE_PACKAGE: &str = "opencode-ai";
pub const OPENCODE_VERSION: &str = "1.4.7";
