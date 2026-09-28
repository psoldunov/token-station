//! Read-only access to `<codex home>/auth.json`.
//!
//! This file is never written or refreshed here: Codex CLI owns its own
//! refresh cycle, and touching the file risks racing it or invalidating the
//! session. Tokens are wrapped in [`SecretString`] so they cannot end up in a
//! `Debug` or log line by accident.

use std::path::Path;

use secrecy::SecretString;
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("no auth.json in {0}")]
    NotFound(std::path::PathBuf),
    #[error("cannot read {path}: {source}")]
    Read {
        path: std::path::PathBuf,
        source: std::io::Error,
    },
    #[error("invalid auth.json: {0}")]
    Invalid(String),
}

/// The subset of `auth.json` needed for the HTTP fallback.
pub struct AuthTokens {
    pub access_token: SecretString,
    pub account_id: String,
}

// Neither struct derives `Debug`: `AuthFileTokens` holds the raw access token
// string before it is wrapped in `SecretString`, and a derived `Debug` would
// let it leak into a log line by accident.
#[derive(Deserialize)]
struct AuthFile {
    tokens: Option<AuthFileTokens>,
}

#[derive(Deserialize)]
struct AuthFileTokens {
    access_token: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
}

/// Read `<home>/auth.json`. Never mutates the file.
pub fn read_auth(home: &Path) -> Result<AuthTokens, AuthError> {
    let path = home.join("auth.json");
    let text = std::fs::read_to_string(&path).map_err(|source| {
        if source.kind() == std::io::ErrorKind::NotFound {
            AuthError::NotFound(path.clone())
        } else {
            AuthError::Read { path, source }
        }
    })?;
    let file: AuthFile =
        serde_json::from_str(&text).map_err(|e| AuthError::Invalid(e.to_string()))?;
    let tokens = file
        .tokens
        .ok_or_else(|| AuthError::Invalid("missing `tokens`".into()))?;
    let access_token = tokens
        .access_token
        .ok_or_else(|| AuthError::Invalid("missing `tokens.access_token`".into()))?;
    Ok(AuthTokens {
        access_token: SecretString::from(access_token),
        account_id: tokens.account_id.unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use secrecy::ExposeSecret;

    use super::*;

    #[test]
    fn reads_fixture_auth_file() {
        let dir = tempfile::tempdir().unwrap();
        let raw = include_str!("../tests/fixtures/auth.json");
        std::fs::write(dir.path().join("auth.json"), raw).unwrap();
        let tokens = read_auth(dir.path()).unwrap();
        assert_eq!(tokens.access_token.expose_secret(), "eyJ.FIXTURE");
        assert_eq!(tokens.account_id, "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn missing_file_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(read_auth(dir.path()), Err(AuthError::NotFound(_))));
    }

    #[test]
    fn invalid_json_is_reported_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("auth.json"), "not json").unwrap();
        assert!(matches!(read_auth(dir.path()), Err(AuthError::Invalid(_))));
    }

    #[test]
    fn reading_never_touches_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let raw = include_str!("../tests/fixtures/auth.json");
        let path = dir.path().join("auth.json");
        std::fs::write(&path, raw).unwrap();
        let before = std::fs::metadata(&path).unwrap().modified().unwrap();
        let before_bytes = std::fs::read(&path).unwrap();

        let _ = read_auth(dir.path()).unwrap();
        let _ = read_auth(dir.path()).unwrap();

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        let after_bytes = std::fs::read(&path).unwrap();
        assert_eq!(before, after);
        assert_eq!(before_bytes, after_bytes);
    }
}
