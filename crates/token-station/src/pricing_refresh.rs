//! Keeping the model price table current.
//!
//! The bundled table always works; a cached download is merged on top, and once a
//! day a fresh copy replaces the cache. Every failure is logged and otherwise ignored.

use std::path::Path;
use std::time::Duration;

use ts_core::config::PricingConfig;
use ts_core::pricing::{Pricing, SharedPricing};

use crate::atomic::{modified_secs, write_atomic};

/// Refuse to read more than this from the pricing URL.
pub const MAX_PRICING_BYTES: usize = 20 * 1024 * 1024;
/// Re-download once the cache is older than this.
pub const CACHE_MAX_AGE_SECS: i64 = 24 * 60 * 60;
/// Whole-request timeout.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    #[error("cannot build the HTTP client: {0}")]
    Client(String),
    #[error("request failed: {0}")]
    Request(String),
    #[error("server answered {0}")]
    Status(u16),
    #[error("pricing document is larger than {MAX_PRICING_BYTES} bytes")]
    TooLarge,
    #[error("pricing document is not valid UTF-8")]
    Encoding,
    #[error("unusable pricing document: {0}")]
    Parse(String),
    #[error("cannot write the pricing cache: {0}")]
    Cache(String),
    #[error("the pricing update did not run: {0}")]
    NotRun(String),
}

/// Read a cached table; a missing or broken cache is simply `None`.
pub fn load_cache(path: &Path) -> Option<Pricing> {
    let text = std::fs::read_to_string(path).ok()?;
    match Pricing::from_litellm_json(&text) {
        Ok(pricing) => Some(pricing),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "ignoring unusable pricing cache");
            None
        }
    }
}

/// Bundled table with the cache merged over it.
#[must_use]
pub fn bundled_with_cache(path: &Path) -> Pricing {
    match load_cache(path) {
        Some(cached) => Pricing::bundled().merged(&cached),
        None => Pricing::bundled(),
    }
}

/// Put the bundled table plus `fresh` into the shared slot.
pub fn install(shared: &SharedPricing, fresh: &Pricing) {
    let merged = Pricing::bundled().merged(fresh);
    match shared.write() {
        Ok(mut slot) => *slot = merged,
        Err(poisoned) => *poisoned.into_inner() = merged,
    }
}

/// Is a new download due?
#[must_use]
pub fn cache_is_stale(path: &Path, now: i64) -> bool {
    match modified_secs(path) {
        Some(modified) => now - modified >= CACHE_MAX_AGE_SECS,
        None => true,
    }
}

/// Download `url`, refusing anything bigger than [`MAX_PRICING_BYTES`].
///
/// # Errors
///
/// Returns [`RefreshError::Client`] when the HTTP client cannot be built,
/// [`RefreshError::Request`] when the request or a body chunk fails or times
/// out, [`RefreshError::Status`] on a non-success status, and
/// [`RefreshError::TooLarge`] when the body exceeds [`MAX_PRICING_BYTES`], and
/// [`RefreshError::Encoding`] when the body is not valid UTF-8.
pub async fn download(url: &str) -> Result<String, RefreshError> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .user_agent(concat!("token-station/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| RefreshError::Client(e.to_string()))?;

    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|e| RefreshError::Request(e.to_string()))?;
    if !response.status().is_success() {
        return Err(RefreshError::Status(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|n| n > MAX_PRICING_BYTES as u64)
    {
        return Err(RefreshError::TooLarge);
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| RefreshError::Request(e.to_string()))?
    {
        if body.len() + chunk.len() > MAX_PRICING_BYTES {
            return Err(RefreshError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| RefreshError::Encoding)
}

/// Parse the downloaded document and store it, off the async worker threads.
///
/// The table is allowed to be [`MAX_PRICING_BYTES`] long and `write_atomic`
/// fsyncs, so neither belongs on a thread that is also driving the bus.
async fn parse_and_cache(text: String, cache_path: &Path) -> Result<Pricing, RefreshError> {
    let path = cache_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let fresh =
            Pricing::from_litellm_json(&text).map_err(|e| RefreshError::Parse(e.to_string()))?;
        write_atomic(&path, text.as_bytes()).map_err(|e| RefreshError::Cache(e.to_string()))?;
        Ok(fresh)
    })
    .await
    .map_err(|e| RefreshError::NotRun(e.to_string()))?
}

/// Download, validate, cache and install a fresh table.
///
/// # Errors
///
/// Returns whatever [`download`] returns, plus [`RefreshError::Parse`] when the
/// body holds no usable prices, [`RefreshError::Cache`] when the cache file
/// cannot be written and [`RefreshError::NotRun`] when the blocking parse task
/// is cancelled. The shared table is left untouched on every failure.
pub async fn fetch_and_install(
    url: &str,
    cache_path: &Path,
    shared: &SharedPricing,
) -> Result<usize, RefreshError> {
    let text = download(url).await?;
    let fresh = parse_and_cache(text, cache_path).await?;
    install(shared, &fresh);
    Ok(fresh.len())
}

/// The daemon's startup path: seed from the cache, then update when it is due.
pub async fn refresh_if_due(
    config: &PricingConfig,
    cache_path: &Path,
    shared: &SharedPricing,
    now: i64,
) {
    if !config.auto_update {
        tracing::debug!("pricing auto-update is off");
        return;
    }
    if !cache_is_stale(cache_path, now) {
        tracing::debug!("pricing cache is fresh");
        return;
    }
    match fetch_and_install(&config.url, cache_path, shared).await {
        Ok(count) => tracing::info!(models = count, "pricing table updated"),
        Err(error) => tracing::warn!(%error, "pricing update failed, keeping the bundled table"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::pricing::shared_bundled;

    const TABLE: &str =
        r#"{"made-up-model": {"input_cost_per_token": 1e-6, "output_cost_per_token": 2e-6}}"#;

    #[test]
    fn cache_loading_tolerates_missing_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pricing.json");
        assert!(load_cache(&path).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_cache(&path).is_none());
        std::fs::write(&path, TABLE).unwrap();
        assert_eq!(load_cache(&path).unwrap().len(), 1);
    }

    #[test]
    fn bundled_is_merged_with_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pricing.json");
        let bundled_only = bundled_with_cache(&path).len();
        std::fs::write(&path, TABLE).unwrap();
        let merged = bundled_with_cache(&path);
        assert_eq!(merged.len(), bundled_only + 1);
        assert!(merged.lookup("made-up-model").is_some());
    }

    #[test]
    fn install_keeps_the_bundled_entries() {
        let shared = shared_bundled();
        let before = shared.read().unwrap().len();
        install(&shared, &Pricing::from_litellm_json(TABLE).unwrap());
        let after = shared.read().unwrap();
        assert_eq!(after.len(), before + 1);
        assert!(after.lookup("claude-sonnet-4-5").is_some());
    }

    #[test]
    fn staleness_uses_the_cache_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pricing.json");
        assert!(
            cache_is_stale(&path, 1_790_596_800),
            "missing cache is stale"
        );
        std::fs::write(&path, TABLE).unwrap();
        let now = modified_secs(&path).unwrap();
        assert!(!cache_is_stale(&path, now));
        assert!(!cache_is_stale(&path, now + CACHE_MAX_AGE_SECS - 1));
        assert!(cache_is_stale(&path, now + CACHE_MAX_AGE_SECS));
    }

    #[tokio::test]
    async fn auto_update_off_leaves_everything_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pricing.json");
        let config = PricingConfig {
            auto_update: false,
            url: "https://127.0.0.1:1/never".into(),
        };
        refresh_if_due(&config, &path, &shared_bundled(), 1_790_596_800).await;
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn a_failed_download_keeps_the_bundled_table() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pricing.json");
        let shared = shared_bundled();
        let before = shared.read().unwrap().len();
        let config = PricingConfig {
            auto_update: true,
            // Port 1 refuses instantly, so this stays fast and offline.
            url: "http://127.0.0.1:1/model_prices.json".into(),
        };
        refresh_if_due(&config, &path, &shared, 1_790_596_800).await;
        assert_eq!(shared.read().unwrap().len(), before);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn parsing_and_caching_happen_on_a_blocking_thread() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/pricing.json");
        let fresh = parse_and_cache(TABLE.to_string(), &path).await.unwrap();
        assert_eq!(fresh.len(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TABLE);

        let error = parse_and_cache("not json".to_string(), &path)
            .await
            .unwrap_err();
        assert!(matches!(error, RefreshError::Parse(_)), "{error}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            TABLE,
            "a broken document does not replace the cache"
        );
    }

    #[tokio::test]
    async fn download_rejects_an_unreachable_host() {
        let error = download("http://127.0.0.1:1/x").await.unwrap_err();
        assert!(matches!(error, RefreshError::Request(_)), "{error}");
    }
}
