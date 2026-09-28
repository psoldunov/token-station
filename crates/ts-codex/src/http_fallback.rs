//! HTTP fallback against the ChatGPT usage endpoint, used only when the
//! app-server is unavailable or too old to support it.

use secrecy::ExposeSecret;
use serde::Deserialize;
use ts_core::snapshot::{Credits, UsageWindow};

use crate::auth::AuthTokens;
use crate::windows::{plan_label, window_kind, window_label};

#[derive(Debug, thiserror::Error)]
pub enum HttpFallbackError {
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("not signed in (401)")]
    Unauthorized,
    #[error("unexpected status {0}")]
    Status(u16),
    #[error("response body was not valid JSON: {0}")]
    Parse(String),
}

/// Windows, credits and plan mapped from the fallback endpoint.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpUsageMapping {
    pub plan: Option<String>,
    pub windows: Vec<UsageWindow>,
    pub credits: Option<Credits>,
}

pub struct HttpFallback {
    client: reqwest::Client,
    base_url: String,
    user_agent: String,
}

impl HttpFallback {
    pub fn new(base_url: String, user_agent: String) -> HttpFallback {
        let client = reqwest::Client::builder()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        HttpFallback {
            client,
            base_url,
            user_agent,
        }
    }

    pub async fn fetch_usage(
        &self,
        tokens: &AuthTokens,
        now: i64,
    ) -> Result<HttpUsageMapping, HttpFallbackError> {
        let url = format!(
            "{}/backend-api/wham/usage",
            self.base_url.trim_end_matches('/')
        );
        let response = self
            .client
            .get(url)
            .bearer_auth(tokens.access_token.expose_secret())
            .header("ChatGPT-Account-Id", &tokens.account_id)
            .header(reqwest::header::USER_AGENT, &self.user_agent)
            .send()
            .await?;

        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(HttpFallbackError::Unauthorized);
        }
        if !response.status().is_success() {
            return Err(HttpFallbackError::Status(response.status().as_u16()));
        }
        let text = response.text().await?;
        parse_usage_body(&text, now).map_err(HttpFallbackError::Parse)
    }
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WhamUsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitDto>,
    credits: Option<HttpCreditsDto>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct RateLimitDto {
    primary_window: Option<WindowDto>,
    secondary_window: Option<WindowDto>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct WindowDto {
    used_percent: f64,
    limit_window_seconds: Option<i64>,
    reset_at: Option<serde_json::Value>,
    reset_after_seconds: Option<i64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct HttpCreditsDto {
    has_credits: bool,
    unlimited: bool,
    balance: Option<String>,
}

fn parse_usage_body(text: &str, now: i64) -> Result<HttpUsageMapping, String> {
    let parsed: WhamUsageResponse = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let plan = parsed.plan_type.as_deref().map(plan_label);
    let mut windows = Vec::new();
    if let Some(rl) = &parsed.rate_limit {
        for (suffix, window) in [
            ("primary", rl.primary_window.as_ref()),
            ("secondary", rl.secondary_window.as_ref()),
        ] {
            if let Some(w) = window {
                windows.push(map_window(suffix, w, now));
            }
        }
    }
    let credits = parsed.credits.map(|c| Credits {
        label: "Credits".to_string(),
        enabled: c.has_credits || c.unlimited,
        used: None,
        limit: None,
        currency: None,
        percent: None,
        detail: if c.unlimited {
            Some("Unlimited".to_string())
        } else {
            c.balance
                .filter(|b| !b.is_empty() && b != "0")
                .map(|b| format!("Balance: {b}"))
        },
    });
    Ok(HttpUsageMapping {
        plan,
        windows,
        credits,
    })
}

fn map_window(suffix: &str, window: &WindowDto, now: i64) -> UsageWindow {
    let minutes = window.limit_window_seconds.map(|s| s / 60);
    let resets_at = window
        .reset_at
        .as_ref()
        .and_then(ts_core::time::parse_timestamp)
        .or_else(|| window.reset_after_seconds.map(|s| now + s));
    UsageWindow {
        id: format!("http:{suffix}"),
        label: window_label("codex", None, None, minutes),
        kind: window_kind("codex", minutes),
        used_percent: window.used_percent,
        resets_at,
        window_minutes: minutes.and_then(|m| u32::try_from(m).ok()),
        level: ts_core::snapshot::Level::default(),
        source: "http".to_string(),
        observed_at: now,
    }
}

#[cfg(test)]
mod tests {
    use secrecy::SecretString;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    fn tokens() -> AuthTokens {
        AuthTokens {
            access_token: SecretString::from("tok-123".to_string()),
            account_id: "acct-1".to_string(),
        }
    }

    #[test]
    fn parses_primary_and_secondary_windows() {
        let body = r#"{
            "plan_type": "pro",
            "rate_limit": {
                "primary_window": {"used_percent": 10.0, "limit_window_seconds": 604800, "reset_after_seconds": 3600},
                "secondary_window": {"used_percent": 20.0, "limit_window_seconds": 300}
            },
            "credits": {"has_credits": true, "unlimited": false, "balance": "7"}
        }"#;
        let mapping = parse_usage_body(body, 1_000).unwrap();
        assert_eq!(mapping.plan, Some("Pro".to_string()));
        assert_eq!(mapping.windows.len(), 2);
        assert_eq!(mapping.windows[0].id, "http:primary");
        assert_eq!(mapping.windows[0].resets_at, Some(4_600));
        assert_eq!(mapping.windows[1].id, "http:secondary");
        assert_eq!(mapping.credits.unwrap().detail, Some("Balance: 7".into()));
    }

    #[test]
    fn tolerates_empty_body() {
        let mapping = parse_usage_body("{}", 0).unwrap();
        assert!(mapping.windows.is_empty());
        assert!(mapping.plan.is_none());
    }

    #[test]
    fn rejects_garbage_without_panicking() {
        assert!(parse_usage_body("not json", 0).is_err());
    }

    #[tokio::test]
    async fn fetch_usage_sends_expected_headers_and_parses_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/backend-api/wham/usage"))
            .and(header("Authorization", "Bearer tok-123"))
            .and(header("ChatGPT-Account-Id", "acct-1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "plan_type": "plus",
                "rate_limit": {"primary_window": {"used_percent": 5.0, "limit_window_seconds": 604800}}
            })))
            .mount(&server)
            .await;

        let fallback = HttpFallback::new(server.uri(), "token-station/test".into());
        let mapping = fallback.fetch_usage(&tokens(), 0).await.unwrap();
        assert_eq!(mapping.plan, Some("Plus".to_string()));
        assert_eq!(mapping.windows.len(), 1);
    }

    #[tokio::test]
    async fn fetch_usage_maps_401_to_unauthorized() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/backend-api/wham/usage"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let fallback = HttpFallback::new(server.uri(), "token-station/test".into());
        let err = fallback.fetch_usage(&tokens(), 0).await.unwrap_err();
        assert!(matches!(err, HttpFallbackError::Unauthorized));
    }
}
