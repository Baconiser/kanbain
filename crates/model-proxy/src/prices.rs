//! Model price table (`prices.json`), used to turn token usage into cost.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::usage::Usage;

pub const DEFAULT_PRICES_JSON: &str = include_str!("../prices.json");

/// Prices in USD per million tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Higher rates once a request's total input passes a threshold.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub long_context: Option<LongContextPrice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LongContextPrice {
    pub above_input_tokens: u64,
    pub input: f64,
    pub output: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PriceTable {
    pub cache_write_5m_multiplier: f64,
    pub cache_write_1h_multiplier: f64,
    pub cache_read_multiplier: f64,
    pub models: BTreeMap<String, ModelPrice>,
}

impl Default for PriceTable {
    fn default() -> Self {
        serde_json::from_str(DEFAULT_PRICES_JSON).expect("embedded prices.json is valid")
    }
}

impl PriceTable {
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Price entry whose key is the longest prefix of `model` (so dated ids
    /// such as `claude-opus-4-5-20251101` match `claude-opus-4-5`).
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        let model = model.rsplit('/').next().unwrap_or(model);
        self.models
            .iter()
            .filter(|(key, _)| model.starts_with(key.as_str()))
            .max_by_key(|(key, _)| key.len())
            .map(|(_, price)| price)
    }

    /// Cost in USD, or `None` when the model has no price entry.
    pub fn cost(&self, model: &str, usage: &Usage) -> Option<f64> {
        let price = self.lookup(model)?;
        let total_input = usage.input_tokens + usage.cache_write_tokens() + usage.cache_read_tokens;
        let (input, output) = match &price.long_context {
            Some(long) if total_input > long.above_input_tokens => (long.input, long.output),
            _ => (price.input, price.output),
        };
        let cache_read = price
            .cache_read
            .unwrap_or(input * self.cache_read_multiplier);
        let per_token = |tokens: u64, rate: f64| tokens as f64 * rate / 1_000_000.0;
        Some(
            per_token(usage.input_tokens, input)
                + per_token(usage.output_tokens, output)
                + per_token(
                    usage.cache_write_5m_tokens,
                    input * self.cache_write_5m_multiplier,
                )
                + per_token(
                    usage.cache_write_1h_tokens,
                    input * self.cache_write_1h_multiplier,
                )
                + per_token(usage.cache_read_tokens, cache_read),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn longest_prefix_wins() {
        let table = PriceTable::default();
        assert_eq!(table.lookup("claude-opus-5-5").unwrap().input, 4.0);
        assert_eq!(table.lookup("claude-opus-5").unwrap().input, 5.0);
        assert_eq!(table.lookup("claude-opus-4-5-20251101").unwrap().input, 5.0);
        assert_eq!(
            table.lookup("anthropic/claude-sonnet-5-5").unwrap().input,
            2.0
        );
        assert!(table.lookup("gpt-unknown").is_none());
    }

    #[test]
    fn computes_cost_with_cache() {
        let table = PriceTable::default();
        let usage = Usage {
            input_tokens: 1_000_000,
            output_tokens: 100_000,
            cache_write_5m_tokens: 1_000_000,
            cache_write_1h_tokens: 0,
            cache_read_tokens: 1_000_000,
        };
        // sonnet 4.6: 3 + 1.5 + 3.75 + 0.3
        assert!(close(
            table.cost("claude-sonnet-4-6", &usage).unwrap(),
            8.55
        ));
        // opus 5.5 with explicit cache read price: 4 + 2 + 5 + 0.2
        assert!(close(table.cost("claude-opus-5-5", &usage).unwrap(), 11.2));
    }

    #[test]
    fn long_context_rates() {
        let table = PriceTable::default();
        let small = Usage {
            input_tokens: 50_000,
            output_tokens: 0,
            ..Default::default()
        };
        let large = Usage {
            input_tokens: 200_000,
            output_tokens: 0,
            ..Default::default()
        };
        assert!(close(
            table.cost("claude-haiku-5-5", &small).unwrap(),
            0.005
        ));
        assert!(close(table.cost("claude-haiku-5-5", &large).unwrap(), 0.1));
    }
}
