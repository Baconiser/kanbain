//! Token usage extraction from Anthropic Messages and OpenAI-compatible
//! responses, both plain JSON and SSE streams.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Uncached input tokens.
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_write_5m_tokens: u64,
    pub cache_write_1h_tokens: u64,
    pub cache_read_tokens: u64,
}

impl Usage {
    pub fn cache_write_tokens(&self) -> u64 {
        self.cache_write_5m_tokens + self.cache_write_1h_tokens
    }
}

/// Accumulates usage and the model name across the events of one response.
#[derive(Debug, Clone, Default)]
pub struct UsageTracker {
    pub model: Option<String>,
    pub usage: Usage,
}

impl UsageTracker {
    pub fn with_model(model: Option<String>) -> Self {
        Self {
            model,
            ..Default::default()
        }
    }

    /// Feed one JSON document: a whole response body or one SSE `data:`
    /// payload. Later events overwrite the fields they carry, which matches
    /// how both APIs report cumulative counts.
    pub fn observe(&mut self, value: &Value) {
        match value.get("type").and_then(Value::as_str) {
            // Anthropic streaming
            Some("message_start") => {
                if let Some(message) = value.get("message") {
                    self.observe_model(message);
                    if let Some(usage) = message.get("usage") {
                        self.observe_anthropic_usage(usage);
                    }
                }
            }
            Some("message_delta") => {
                if let Some(usage) = value.get("usage") {
                    self.observe_anthropic_usage(usage);
                }
            }
            // OpenAI Responses streaming
            Some("response.completed") | Some("response.incomplete") | Some("response.failed") => {
                if let Some(response) = value.get("response") {
                    self.observe_model(response);
                    if let Some(usage) = response.get("usage") {
                        self.observe_openai_usage(usage);
                    }
                }
            }
            // Anthropic non-streaming responses have "type": "message"; OpenAI
            // chat completions and responses have no/other "type".
            _ => {
                self.observe_model(value);
                if let Some(usage) = value.get("usage").filter(|u| u.is_object()) {
                    if usage.get("prompt_tokens").is_some()
                        || usage.get("input_tokens_details").is_some()
                        || value.get("object").is_some()
                    {
                        self.observe_openai_usage(usage);
                    } else {
                        self.observe_anthropic_usage(usage);
                    }
                }
            }
        }
    }

    fn observe_model(&mut self, value: &Value) {
        if let Some(model) = value.get("model").and_then(Value::as_str)
            && !model.is_empty()
        {
            self.model = Some(model.to_string());
        }
    }

    fn observe_anthropic_usage(&mut self, usage: &Value) {
        let get = |key: &str| usage.get(key).and_then(Value::as_u64);
        if let Some(n) = get("input_tokens") {
            self.usage.input_tokens = n;
        }
        if let Some(n) = get("output_tokens") {
            self.usage.output_tokens = n;
        }
        if let Some(n) = get("cache_read_input_tokens") {
            self.usage.cache_read_tokens = n;
        }
        let breakdown = usage.get("cache_creation").filter(|v| v.is_object());
        match (get("cache_creation_input_tokens"), breakdown) {
            (_, Some(breakdown)) => {
                let five = breakdown
                    .get("ephemeral_5m_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let hour = breakdown
                    .get("ephemeral_1h_input_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                self.usage.cache_write_5m_tokens = five;
                self.usage.cache_write_1h_tokens = hour;
            }
            (Some(total), None) => {
                self.usage.cache_write_5m_tokens = total;
                self.usage.cache_write_1h_tokens = 0;
            }
            (None, None) => {}
        }
    }

    fn observe_openai_usage(&mut self, usage: &Value) {
        let get = |key: &str| usage.get(key).and_then(Value::as_u64);
        let input = get("prompt_tokens").or_else(|| get("input_tokens"));
        let output = get("completion_tokens").or_else(|| get("output_tokens"));
        let cached = usage
            .get("prompt_tokens_details")
            .or_else(|| usage.get("input_tokens_details"))
            .and_then(|d| d.get("cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if let Some(input) = input {
            // OpenAI counts cached tokens inside the input total.
            self.usage.input_tokens = input.saturating_sub(cached);
            self.usage.cache_read_tokens = cached;
        }
        if let Some(output) = output {
            self.usage.output_tokens = output;
        }
    }
}

/// Incremental SSE parser: feed raw bytes, get complete `data:` payloads.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    data: String,
}

impl SseParser {
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\n', '\r']);
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(std::mem::take(&mut self.data));
                }
            } else if let Some(data) = line.strip_prefix("data:") {
                if !self.data.is_empty() {
                    self.data.push('\n');
                }
                self.data.push_str(data.strip_prefix(' ').unwrap_or(data));
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn anthropic_stream_usage() {
        let mut tracker = UsageTracker::default();
        let mut parser = SseParser::default();
        let stream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"model\":\"claude-sonnet-5-5\",\"usage\":{\"input_tokens\":10,\"output_tokens\":1,\"cache_creation_input_tokens\":300,\"cache_read_input_tokens\":2000,\"cache_creation\":{\"ephemeral_5m_input_tokens\":100,\"ephemeral_1h_input_tokens\":200}}}}\n",
            "\n",
            "event: content_block_delta\n",
            "data: {\"type\":\"content_block_delta\",\"index\":0}\n\n",
            "event: message_delta\n",
            "data: {\"type\":\"message_delta\",\"usage\":{\"output_tokens\":42}}\n\n",
        );
        // Split mid-line to exercise buffering.
        let (a, b) = stream.split_at(57);
        for chunk in [a, b] {
            for data in parser.feed(chunk.as_bytes()) {
                tracker.observe(&serde_json::from_str(&data).unwrap());
            }
        }
        assert_eq!(tracker.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(
            tracker.usage,
            Usage {
                input_tokens: 10,
                output_tokens: 42,
                cache_write_5m_tokens: 100,
                cache_write_1h_tokens: 200,
                cache_read_tokens: 2000,
            }
        );
    }

    #[test]
    fn anthropic_json_without_breakdown() {
        let mut tracker = UsageTracker::default();
        tracker.observe(&json!({
            "type": "message",
            "model": "claude-haiku-5-5",
            "usage": {"input_tokens": 5, "output_tokens": 7, "cache_creation_input_tokens": 9}
        }));
        assert_eq!(tracker.usage.cache_write_5m_tokens, 9);
        assert_eq!(tracker.usage.output_tokens, 7);
    }

    #[test]
    fn openai_chat_and_responses() {
        let mut chat = UsageTracker::default();
        chat.observe(&json!({
            "object": "chat.completion.chunk",
            "model": "gpt-x",
            "choices": [],
            "usage": {"prompt_tokens": 100, "completion_tokens": 20, "prompt_tokens_details": {"cached_tokens": 60}}
        }));
        assert_eq!(chat.usage.input_tokens, 40);
        assert_eq!(chat.usage.cache_read_tokens, 60);
        assert_eq!(chat.usage.output_tokens, 20);

        let mut responses = UsageTracker::default();
        responses.observe(&json!({
            "type": "response.completed",
            "response": {"model": "gpt-y", "usage": {"input_tokens": 50, "output_tokens": 5, "input_tokens_details": {"cached_tokens": 10}}}
        }));
        assert_eq!(responses.model.as_deref(), Some("gpt-y"));
        assert_eq!(responses.usage.input_tokens, 40);
        assert_eq!(responses.usage.output_tokens, 5);
    }
}
