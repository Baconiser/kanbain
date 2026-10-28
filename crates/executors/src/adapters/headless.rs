//! Headless one-shot runs (D34): intake and history jobs run an agent
//! definition without a repo or tools, send the task as the first message
//! and take the agent's final message as the result.

use serde_json::Value;

use super::AgentLaunch;
use crate::{
    executors::BaseCodingAgent,
    versions::{CLAUDE_CODE_PACKAGE, CLAUDE_CODE_VERSION, OPENCODE_PACKAGE, OPENCODE_VERSION},
};

/// The command for a headless run whose task is in `task_file`, as argv.
/// Claude Code reads the task from stdin, so the caller runs this through
/// `sh -c 'exec "$@" < <task_file>'`; see [`stdin_from`].
pub fn headless_command(launch: &AgentLaunch, task_file: &str) -> Vec<String> {
    let mut argv: Vec<String> = Vec::new();
    // Adapters only launch Claude Code and OpenCode.
    if launch.executor != BaseCodingAgent::Opencode {
        argv.extend([
            "npx".into(),
            "-y".into(),
            format!("{CLAUDE_CODE_PACKAGE}@{CLAUDE_CODE_VERSION}"),
            "-p".into(),
            "--output-format".into(),
            "json".into(),
            "--model".into(),
            launch.model.clone(),
        ]);
        argv.extend(launch.args.iter().cloned());
    } else {
        argv.extend([
            "npx".into(),
            "-y".into(),
            format!("{OPENCODE_PACKAGE}@{OPENCODE_VERSION}"),
            "run".into(),
            // The message goes first: `--file` takes every value after it.
            "Do the task in the attached task file.".into(),
            "--format".into(),
            "json".into(),
            "--model".into(),
            launch.model.clone(),
        ]);
        if let Some(agent) = &launch.runtime_agent {
            argv.extend(["--agent".into(), agent.clone()]);
        }
        argv.extend(launch.args.iter().cloned());
        argv.extend(["--file".into(), task_file.to_string()]);
    }
    argv
}

/// Wraps argv so the process reads `stdin_file` as its stdin (Claude Code's
/// prompt), or runs as is (OpenCode attaches the file itself).
pub fn stdin_from(launch: &AgentLaunch, argv: Vec<String>, stdin_file: &str) -> Vec<String> {
    if launch.executor == BaseCodingAgent::Opencode {
        argv
    } else {
        let mut wrapped = vec![
            "sh".to_string(),
            "-c".to_string(),
            format!("exec \"$@\" < '{stdin_file}'"),
            "sh".to_string(),
        ];
        wrapped.extend(argv);
        wrapped
    }
}

/// The agent's final message from a headless run's stdout.
pub fn parse_headless_output(executor: BaseCodingAgent, stdout: &str) -> Result<String, String> {
    if executor == BaseCodingAgent::Opencode {
        parse_opencode(stdout)
    } else {
        parse_claude(stdout)
    }
}

/// `claude -p --output-format json` prints one result object.
fn parse_claude(stdout: &str) -> Result<String, String> {
    let result = stdout
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .find(|v| v.get("type").and_then(Value::as_str) == Some("result"))
        .ok_or_else(|| "The agent printed no result".to_string())?;
    let text = result
        .get("result")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if result.get("is_error").and_then(Value::as_bool) == Some(true) {
        let reason = if text.is_empty() {
            result
                .get("subtype")
                .and_then(Value::as_str)
                .unwrap_or("error")
                .to_string()
        } else {
            text
        };
        return Err(format!("The agent failed: {reason}"));
    }
    Ok(text)
}

/// `opencode run --format json` prints one JSON event per line; the final
/// message is the text of the last assistant message.
fn parse_opencode(stdout: &str) -> Result<String, String> {
    let mut messages: Vec<(String, String)> = Vec::new();
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("text") => {
                let part = event.get("part").unwrap_or(&Value::Null);
                let message_id = part
                    .get("messageID")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
                match messages.last_mut() {
                    Some((id, buf)) if *id == message_id => buf.push_str(text),
                    _ => messages.push((message_id, text.to_string())),
                }
            }
            Some("error") => {
                let error = event.get("error").unwrap_or(&Value::Null);
                let message = error
                    .pointer("/data/message")
                    .or_else(|| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| error.to_string());
                return Err(format!("The agent failed: {message}"));
            }
            _ => {}
        }
    }
    messages
        .pop()
        .map(|(_, text)| text)
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| "The agent printed no answer".to_string())
}

/// The first JSON object in an agent's answer: a fenced ```json block, or
/// the outermost `{…}`.
pub fn extract_json_object(text: &str) -> Option<Value> {
    if let Some(start) = text.find("```") {
        let after = &text[start + 3..];
        let body_start = after.find('\n').map(|i| i + 1).unwrap_or(0);
        if let Some(end) = after[body_start..].find("```") {
            let body = &after[body_start..body_start + end];
            if let Ok(value @ Value::Object(_)) = serde_json::from_str(body.trim()) {
                return Some(value);
            }
        }
    }
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    match serde_json::from_str(&text[start..=end]) {
        Ok(value @ Value::Object(_)) => Some(value),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn launch(executor: BaseCodingAgent) -> AgentLaunch {
        AgentLaunch {
            agent: "intake".into(),
            executor,
            model: "haiku".into(),
            args: vec!["--tools=".into()],
            env: HashMap::new(),
            files: vec![],
            skills: vec![],
            runtime_agent: Some("kanbain-intake".into()),
        }
    }

    #[test]
    fn claude_command_reads_task_from_stdin() {
        let l = launch(BaseCodingAgent::ClaudeCode);
        let argv = stdin_from(
            &l,
            headless_command(&l, "/kanbain/context/task.md"),
            "/kanbain/context/task.md",
        );
        assert_eq!(
            argv[..3],
            ["sh", "-c", "exec \"$@\" < '/kanbain/context/task.md'"]
        );
        assert!(argv.contains(&"-p".to_string()));
        assert!(argv.ends_with(&["haiku".to_string(), "--tools=".to_string()]));
    }

    #[test]
    fn opencode_command_attaches_task() {
        let l = launch(BaseCodingAgent::Opencode);
        let argv = stdin_from(&l, headless_command(&l, "/t.md"), "/t.md");
        assert_eq!(argv[3], "run");
        assert!(argv.windows(2).any(|w| w == ["--agent", "kanbain-intake"]));
        assert_eq!(argv[argv.len() - 2..], ["--file", "/t.md"]);
    }

    #[test]
    fn parses_claude_result() {
        let out = "noise\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"{\\\"title\\\":\\\"x\\\"}\"}\n";
        assert_eq!(
            parse_headless_output(BaseCodingAgent::ClaudeCode, out).unwrap(),
            "{\"title\":\"x\"}"
        );
        let err = "{\"type\":\"result\",\"subtype\":\"error_max_budget_usd\",\"is_error\":true}";
        assert!(
            parse_headless_output(BaseCodingAgent::ClaudeCode, err)
                .unwrap_err()
                .contains("error_max_budget_usd")
        );
        assert!(parse_headless_output(BaseCodingAgent::ClaudeCode, "").is_err());
    }

    #[test]
    fn parses_opencode_events() {
        let out = [
            r#"{"type":"step_start","part":{"messageID":"m1"}}"#,
            r#"{"type":"text","part":{"messageID":"m1","text":"Thinking"}}"#,
            r#"{"type":"text","part":{"messageID":"m2","text":"{\"title\":"}}"#,
            r#"{"type":"text","part":{"messageID":"m2","text":"\"y\"}"}}"#,
        ]
        .join("\n");
        assert_eq!(
            parse_headless_output(BaseCodingAgent::Opencode, &out).unwrap(),
            "{\"title\":\"y\"}"
        );
        let err = r#"{"type":"error","error":{"name":"APIError","data":{"message":"bad key"}}}"#;
        assert_eq!(
            parse_headless_output(BaseCodingAgent::Opencode, err).unwrap_err(),
            "The agent failed: bad key"
        );
    }

    #[test]
    fn extracts_json() {
        assert_eq!(
            extract_json_object("Here:\n```json\n{\"a\":1}\n```\nDone")
                .unwrap()
                .get("a"),
            Some(&Value::from(1))
        );
        assert_eq!(
            extract_json_object("Sure {\"a\":{\"b\":2}} ok").unwrap()["a"]["b"],
            2
        );
        assert!(extract_json_object("no json").is_none());
    }
}
