//! API-equivalent pricing from LiteLLM's `model_prices_and_context_window.json`.
//!
//! A subset is vendored in `data/pricing/litellm-subset.json`; the daemon may merge a
//! fresher copy on top with [`Pricing::merged`].

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::tokens::TokenCounts;

/// Requests whose prompt (input + cache) exceeds this use the "above 200k" tier.
pub const LONG_CONTEXT_THRESHOLD: u64 = 200_000;

const BUNDLED_JSON: &str = include_str!("../../../data/pricing/litellm-subset.json");

/// Per-token USD prices for one model.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input_cost_per_token: f64,
    pub output_cost_per_token: f64,
    #[serde(default)]
    pub cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    pub input_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub output_cost_per_token_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_creation_input_token_cost_above_200k_tokens: Option<f64>,
    #[serde(default)]
    pub cache_read_input_token_cost_above_200k_tokens: Option<f64>,
}

impl ModelPrice {
    /// Cost of one request.
    pub fn cost(&self, counts: &TokenCounts) -> f64 {
        let long = counts.prompt_tokens() > LONG_CONTEXT_THRESHOLD;
        let pick = |base: f64, above: Option<f64>| if long { above.unwrap_or(base) } else { base };
        let input = pick(
            self.input_cost_per_token,
            self.input_cost_per_token_above_200k_tokens,
        );
        let output = pick(
            self.output_cost_per_token,
            self.output_cost_per_token_above_200k_tokens,
        );
        let cache_write_base = self.cache_creation_input_token_cost.unwrap_or(input);
        let cache_write = pick(
            cache_write_base,
            self.cache_creation_input_token_cost_above_200k_tokens,
        );
        let cache_read_base = self.cache_read_input_token_cost.unwrap_or(input);
        let cache_read = pick(
            cache_read_base,
            self.cache_read_input_token_cost_above_200k_tokens,
        );
        counts.input as f64 * input
            + counts.output as f64 * output
            + counts.cache_write as f64 * cache_write
            + counts.cache_read as f64 * cache_read
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PricingError {
    #[error("pricing JSON is not an object: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("pricing JSON contained no usable model prices")]
    Empty,
}

/// Model price table.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pricing {
    models: HashMap<String, ModelPrice>,
}

impl Pricing {
    /// The vendored snapshot compiled into the binary.
    pub fn bundled() -> Pricing {
        Pricing::from_litellm_json(BUNDLED_JSON).unwrap_or_default()
    }

    /// Parse LiteLLM's JSON. Entries without input/output prices are skipped.
    pub fn from_litellm_json(text: &str) -> Result<Pricing, PricingError> {
        let raw: HashMap<String, serde_json::Value> = serde_json::from_str(text)?;
        let models: HashMap<String, ModelPrice> = raw
            .into_iter()
            .filter_map(|(name, value)| {
                serde_json::from_value::<ModelPrice>(value)
                    .ok()
                    .map(|price| (name.to_ascii_lowercase(), price))
            })
            .collect();
        if models.is_empty() {
            return Err(PricingError::Empty);
        }
        Ok(Pricing { models })
    }

    /// `other`'s entries override `self`'s.
    pub fn merged(&self, other: &Pricing) -> Pricing {
        let mut models = self.models.clone();
        models.extend(other.models.iter().map(|(k, v)| (k.clone(), v.clone())));
        Pricing { models }
    }

    pub fn len(&self) -> usize {
        self.models.len()
    }

    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Find a price for a model id as written by the CLIs, tolerating provider
    /// prefixes (`anthropic/`), bracket suffixes (`[1m]`) and date suffixes
    /// (`-20250929`).
    pub fn lookup(&self, model: &str) -> Option<&ModelPrice> {
        candidates(model)
            .into_iter()
            .find_map(|candidate| self.models.get(&candidate))
    }

    /// Cost of one request, or `None` when the model is unknown.
    pub fn cost(&self, model: &str, counts: &TokenCounts) -> Option<f64> {
        self.lookup(model).map(|price| price.cost(counts))
    }
}

fn candidates(model: &str) -> Vec<String> {
    let lower = model.trim().to_ascii_lowercase();
    let no_bracket = lower
        .split_once('[')
        .map_or(lower.as_str(), |(head, _)| head)
        .to_string();
    let no_prefix = no_bracket
        .rsplit_once('/')
        .map_or(no_bracket.as_str(), |(_, tail)| tail)
        .to_string();
    let no_date = strip_date_suffix(&no_prefix);
    let no_latest = no_date.trim_end_matches("-latest").to_string();
    let mut out = vec![
        lower.clone(),
        no_bracket.clone(),
        no_prefix.clone(),
        no_date.clone(),
        no_latest,
        format!("anthropic/{no_prefix}"),
        format!("openai/{no_prefix}"),
    ];
    out.dedup();
    out
}

fn strip_date_suffix(model: &str) -> String {
    match model.rsplit_once('-') {
        Some((head, tail)) if tail.len() == 8 && tail.bytes().all(|b| b.is_ascii_digit()) => {
            head.to_string()
        }
        _ => model.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(input: u64, output: u64, cache_read: u64, cache_write: u64) -> TokenCounts {
        TokenCounts {
            input,
            output,
            cache_read,
            cache_write,
            reasoning: 0,
        }
    }

    fn table() -> Pricing {
        Pricing::from_litellm_json(
            r#"{
              "claude-sonnet-4-5": {"input_cost_per_token": 3e-6, "output_cost_per_token": 1.5e-5,
                 "cache_creation_input_token_cost": 3.75e-6, "cache_read_input_token_cost": 3e-7,
                 "input_cost_per_token_above_200k_tokens": 6e-6, "output_cost_per_token_above_200k_tokens": 2.25e-5},
              "gpt-5.3-codex": {"input_cost_per_token": 1.25e-6, "output_cost_per_token": 1e-5,
                 "cache_read_input_token_cost": 1.25e-7},
              "sample_spec": {"max_tokens": "string"}
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn skips_entries_without_prices() {
        assert_eq!(table().len(), 2);
    }

    #[test]
    fn lookup_normalizes_model_ids() {
        let p = table();
        for id in [
            "claude-sonnet-4-5",
            "claude-sonnet-4-5-20250929",
            "anthropic/claude-sonnet-4-5",
            "Claude-Sonnet-4-5[1m]",
            "openai/gpt-5.3-codex",
        ] {
            assert!(p.lookup(id).is_some(), "{id} should resolve");
        }
        assert!(p.lookup("mystery-model").is_none());
    }

    #[test]
    fn cost_uses_each_bucket_price() {
        let p = table();
        let c = p
            .cost("claude-sonnet-4-5", &counts(1_000, 100, 10_000, 2_000))
            .unwrap();
        let expected = 1_000.0 * 3e-6 + 100.0 * 1.5e-5 + 10_000.0 * 3e-7 + 2_000.0 * 3.75e-6;
        assert!((c - expected).abs() < 1e-12);
    }

    #[test]
    fn long_context_tier_applies_above_threshold() {
        let p = table();
        let c = p
            .cost("claude-sonnet-4-5", &counts(250_000, 1_000, 0, 0))
            .unwrap();
        let expected = 250_000.0 * 6e-6 + 1_000.0 * 2.25e-5;
        assert!((c - expected).abs() < 1e-9);
    }

    #[test]
    fn missing_cache_prices_fall_back_to_input_price() {
        let p = table();
        let c = p.cost("gpt-5.3-codex", &counts(0, 0, 0, 1_000)).unwrap();
        assert!((c - 1_000.0 * 1.25e-6).abs() < 1e-12);
    }

    #[test]
    fn merged_overrides() {
        let base = table();
        let newer = Pricing::from_litellm_json(
            r#"{"gpt-5.3-codex": {"input_cost_per_token": 1.0, "output_cost_per_token": 2.0}}"#,
        )
        .unwrap();
        let m = base.merged(&newer);
        assert_eq!(m.len(), 2);
        assert_eq!(m.lookup("gpt-5.3-codex").unwrap().input_cost_per_token, 1.0);
    }

    #[test]
    fn bundled_table_prices_current_models() {
        let p = Pricing::bundled();
        assert!(p.len() > 20);
        assert!(p.lookup("claude-opus-5-5").is_some());
        assert!(p.lookup("gpt-5.3-codex").is_some());
    }

    #[test]
    fn rejects_garbage() {
        assert!(Pricing::from_litellm_json("[1,2]").is_err());
        assert!(matches!(
            Pricing::from_litellm_json("{}"),
            Err(PricingError::Empty)
        ));
    }
}
