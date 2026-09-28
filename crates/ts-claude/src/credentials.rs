//! Reads `.credentials.json`. Never writes it, never refreshes it, never logs
//! the tokens it holds.

use std::path::Path;

use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum CredentialsError {
    #[error("credentials file not found")]
    NotFound,
    #[error("cannot read credentials file: {0}")]
    Read(std::io::Error),
    #[error("cannot parse credentials file: {0}")]
    Parse(serde_json::Error),
}

// Neither struct derives `Debug`: both hold the raw access token string
// before it is wrapped in `SecretString`, and a derived `Debug` would let it
// leak into a log line by accident.
#[derive(Deserialize)]
struct CredentialsFile {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: OauthCreds,
}

#[derive(Deserialize)]
struct OauthCreds {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
    #[serde(rename = "subscriptionType", default)]
    subscription_type: String,
    #[serde(rename = "rateLimitTier", default)]
    rate_limit_tier: String,
}

/// Parsed OAuth credentials. The access token never leaves this type except as a
/// [`SecretString`].
#[derive(Debug, Clone)]
pub struct Credentials {
    pub access_token: SecretString,
    /// Milliseconds since the epoch.
    pub expires_at_ms: i64,
    pub subscription_type: String,
    pub rate_limit_tier: String,
}

impl Credentials {
    /// True when the token expires within `now + 60s` (or already has).
    pub fn expired(&self, now: i64) -> bool {
        self.expires_at_ms / 1000 <= now + 60
    }
}

/// Load credentials from `path`. Only reads; never touches the file's mtime or
/// content.
pub fn load(path: &Path) -> Result<Credentials, CredentialsError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CredentialsError::NotFound);
        }
        Err(e) => return Err(CredentialsError::Read(e)),
    };
    let parsed: CredentialsFile = serde_json::from_str(&text).map_err(CredentialsError::Parse)?;
    let oauth = parsed.claude_ai_oauth;
    Ok(Credentials {
        access_token: SecretString::from(oauth.access_token),
        expires_at_ms: oauth.expires_at,
        subscription_type: oauth.subscription_type,
        rate_limit_tier: oauth.rate_limit_tier,
    })
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    fn fixture_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/credentials.json")
    }

    #[test]
    fn loads_real_fixture() {
        let creds = load(&fixture_path()).unwrap();
        assert_eq!(creds.access_token.expose_secret(), "sk-ant-oat01-FIXTURE");
        assert_eq!(creds.subscription_type, "max");
        assert_eq!(creds.rate_limit_tier, "default_claude_max_20x");
        assert_eq!(creds.expires_at_ms, 1_790_610_000_000);
    }

    #[test]
    fn missing_file_is_not_found() {
        let err = load(Path::new("/does/not/exist.json")).unwrap_err();
        assert!(matches!(err, CredentialsError::NotFound));
    }

    #[test]
    fn garbage_json_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("credentials.json");
        std::fs::write(&path, "{not json").unwrap();
        assert!(matches!(load(&path), Err(CredentialsError::Parse(_))));
    }

    #[test]
    fn expiry_uses_sixty_second_floor() {
        let creds = Credentials {
            access_token: SecretString::from("t".to_string()),
            expires_at_ms: 100_000,
            subscription_type: "max".into(),
            rate_limit_tier: "default_claude_max_20x".into(),
        };
        assert!(!creds.expired(30)); // 100s - 60s floor = 40s away
        assert!(creds.expired(41));
    }
}
