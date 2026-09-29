//! Lenient DTOs for the app-server and HTTP-fallback JSON payloads.
//!
//! Every field is optional or defaulted: this is untrusted external JSON, and
//! a missing or unexpected field must never fail parsing.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct AccountReadResult {
    pub account: Option<AccountInfo>,
    pub requires_openai_auth: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum AccountInfo {
    ApiKey {},
    #[serde(rename_all = "camelCase")]
    Chatgpt {
        #[serde(default)]
        plan_type: Option<String>,
    },
    AmazonBedrock {},
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimitsReadResult {
    pub ordinary_usage_allowed: Option<bool>,
    pub rate_limits: Option<RateLimitSnapshotDto>,
    pub rate_limits_by_limit_id: std::collections::BTreeMap<String, RateLimitSnapshotDto>,
    pub rate_limit_reset_credits: Option<RateLimitResetCreditsDto>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimitSnapshotDto {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub normal_model_slug: Option<String>,
    pub primary: Option<RateLimitWindowDto>,
    pub secondary: Option<RateLimitWindowDto>,
    pub credits: Option<CreditsDto>,
    pub plan_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimitWindowDto {
    pub used_percent: f64,
    pub window_duration_mins: Option<i64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct CreditsDto {
    pub has_credits: bool,
    pub unlimited: bool,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct RateLimitResetCreditsDto {
    pub available_count: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageReadResult {
    pub summary: Option<UsageSummaryDto>,
    pub daily_usage_buckets: Vec<DailyBucketDto>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct UsageSummaryDto {
    pub lifetime_tokens: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct DailyBucketDto {
    pub start_date: String,
    pub tokens: u64,
}

#[cfg(test)]
mod tests {
    use serde::de::DeserializeOwned;

    use super::*;

    /// Parse `fixtures/<name>`'s `result` field as `T`.
    fn result_fixture<T: DeserializeOwned>(raw: &str) -> T {
        let value: serde_json::Value = serde_json::from_str(raw).unwrap();
        serde_json::from_value(value["result"].clone()).unwrap()
    }

    #[test]
    fn parses_chatgpt_account_fixture() {
        let result: AccountReadResult =
            result_fixture(include_str!("../tests/fixtures/account_read_response.json"));
        assert!(result.requires_openai_auth);
        match result.account {
            Some(AccountInfo::Chatgpt { plan_type }) => assert_eq!(plan_type, Some("pro".into())),
            other => panic!("expected chatgpt account, got {other:?}"),
        }
    }

    #[test]
    fn parses_rate_limits_fixture() {
        let result: RateLimitsReadResult = result_fixture(include_str!(
            "../tests/fixtures/rate_limits_read_response.json"
        ));
        assert_eq!(result.ordinary_usage_allowed, Some(true));
        let codex = result.rate_limits_by_limit_id.get("codex").unwrap();
        assert_eq!(
            codex.primary.as_ref().unwrap().window_duration_mins,
            Some(10080)
        );
        assert!(codex.secondary.is_none());
        assert_eq!(
            result.rate_limit_reset_credits.unwrap().available_count,
            Some(1)
        );
    }

    #[test]
    fn parses_usage_fixture() {
        let result: UsageReadResult =
            result_fixture(include_str!("../tests/fixtures/usage_read_response.json"));
        assert_eq!(result.summary.unwrap().lifetime_tokens, Some(4_743_603_614));
        assert_eq!(result.daily_usage_buckets.len(), 4);
    }

    #[test]
    fn tolerates_unknown_account_type() {
        let value = serde_json::json!({"type": "somethingNew"});
        let account: AccountInfo = serde_json::from_value(value).unwrap();
        assert!(matches!(account, AccountInfo::Unknown));
    }

    #[test]
    fn missing_fields_default_instead_of_failing() {
        let result: AccountReadResult = serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(result.account.is_none());
        assert!(!result.requires_openai_auth);
    }
}
