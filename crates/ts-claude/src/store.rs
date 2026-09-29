//! Where the Claude Code sign-in is read from, whichever that is on this
//! platform.
//!
//! Linux keeps it in `~/.claude/.credentials.json`; macOS keeps it in the login
//! Keychain and only falls back to that file. Both are read-only here: Token
//! Station never writes, refreshes or logs a credential.
//!
//! A store is built once per provider and reads are cached for [`CACHE_TTL`],
//! because one `refresh_limits` asks for the sign-in two or three times over
//! and on macOS each ask is a `/usr/bin/security` subprocess. The cached value
//! lives in memory, keeps its token inside a `SecretString`, and is never
//! written anywhere.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::credentials::{self, Credentials, CredentialsError};

#[cfg(target_os = "macos")]
use crate::keychain::{self, KeychainError, KeychainId, SecurityRunner};

/// How long one read stands in for the next.
///
/// Long enough to cover a single refresh pass, far too short to hide a
/// re-sign-in from the next one: plan limits are polled every few minutes.
pub const CACHE_TTL: Duration = Duration::from_secs(5);

/// The last successful read, and when it was taken.
struct Cached {
    at: Instant,
    credentials: Credentials,
}

/// The places one sign-in may live, in the order they are tried.
pub struct CredentialStore {
    file: PathBuf,
    #[cfg(target_os = "macos")]
    keychain: Vec<KeychainId>,
    #[cfg(target_os = "macos")]
    runner: Box<dyn SecurityRunner>,
    cache: Mutex<Option<Cached>>,
}

impl CredentialStore {
    /// The file alone; what every platform but macOS uses.
    #[must_use]
    pub fn file(path: PathBuf) -> CredentialStore {
        CredentialStore {
            file: path,
            #[cfg(target_os = "macos")]
            keychain: Vec::new(),
            #[cfg(target_os = "macos")]
            runner: Box::new(keychain::Security),
            cache: Mutex::new(None),
        }
    }

    /// The Keychain items in `ids`, in order, then the file.
    #[cfg(target_os = "macos")]
    #[must_use]
    pub fn keychain_then_file(ids: Vec<KeychainId>, path: PathBuf) -> CredentialStore {
        CredentialStore {
            file: path,
            keychain: ids,
            runner: Box::new(keychain::Security),
            cache: Mutex::new(None),
        }
    }

    /// As [`CredentialStore::keychain_then_file`], with the `security` runner
    /// replaced. Tests only, which is why it is not part of the crate's API.
    #[cfg(all(target_os = "macos", test))]
    fn with_runner(
        ids: Vec<KeychainId>,
        path: PathBuf,
        runner: Box<dyn SecurityRunner>,
    ) -> CredentialStore {
        CredentialStore {
            file: path,
            keychain: ids,
            runner,
            cache: Mutex::new(None),
        }
    }

    /// Where the file half of the store looks.
    #[must_use]
    pub fn file_path(&self) -> &Path {
        &self.file
    }

    /// Read the sign-in, using a read taken within the last [`CACHE_TTL`] if
    /// there is one. Blocking.
    ///
    /// # Errors
    ///
    /// Whatever [`CredentialStore::load_fresh`] reports. A failure is never
    /// cached: only a successful read is worth repeating.
    pub fn load(&self) -> Result<Credentials, CredentialsError> {
        if let Some(cached) = self.cached() {
            return Ok(cached);
        }
        let credentials = self.load_fresh()?;
        *lock(&self.cache) = Some(Cached {
            at: Instant::now(),
            credentials: credentials.clone(),
        });
        Ok(credentials)
    }

    fn cached(&self) -> Option<Credentials> {
        let cache = lock(&self.cache);
        cache
            .as_ref()
            .filter(|cached| cached.at.elapsed() < CACHE_TTL)
            .map(|cached| cached.credentials.clone())
    }

    /// Read the sign-in, ignoring and replacing anything cached. Blocking.
    ///
    /// This is the one to call after something that may have changed the
    /// sign-in — `claude auth status` refreshing an expired token, say — where
    /// a five-second-old answer is exactly the wrong answer.
    ///
    /// # Errors
    ///
    /// Returns [`CredentialsError::NotFound`] when there is no sign-in
    /// anywhere, and the read or parse errors of whichever source answered. On
    /// macOS it also returns [`CredentialsError::Unavailable`] when the
    /// Keychain could not be asked and there is no file to fall back to.
    pub fn load_fresh(&self) -> Result<Credentials, CredentialsError> {
        let fresh = self.read_through();
        if let Ok(credentials) = &fresh {
            *lock(&self.cache) = Some(Cached {
                at: Instant::now(),
                credentials: credentials.clone(),
            });
        }
        fresh
    }

    #[cfg(not(target_os = "macos"))]
    fn read_through(&self) -> Result<Credentials, CredentialsError> {
        credentials::load(&self.file)
    }

    #[cfg(target_os = "macos")]
    fn read_through(&self) -> Result<Credentials, CredentialsError> {
        let mut unavailable = None;
        for id in &self.keychain {
            match keychain::read(self.runner.as_ref(), id) {
                Ok(payload) => return credentials::parse(&payload),
                // Not under this name. Another name, or the file, may still
                // have it: a machine signed in before Claude Code moved to the
                // Keychain has the file, and a directory named by hand has an
                // item under a name Claude Code never derived.
                Err(KeychainError::NotFound) => {}
                Err(error) => {
                    unavailable = Some(error);
                    break;
                }
            }
        }
        match (credentials::load(&self.file), unavailable) {
            (Ok(credentials), _) => Ok(credentials),
            // Saying "not signed in" here would be a lie: the sign-in is very
            // probably there and this process cannot see it, which is what an
            // SSH session without a GUI Security session looks like.
            (Err(CredentialsError::NotFound), Some(error)) => {
                Err(CredentialsError::Unavailable(error.to_string()))
            }
            (Err(error), _) => Err(error),
        }
    }
}

fn lock(cache: &Mutex<Option<Cached>>) -> std::sync::MutexGuard<'_, Option<Cached>> {
    cache
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &str = r#"{"claudeAiOauth":{"accessToken":"sk-ant-oat01-FIXTURE","expiresAt":1790610000000,"subscriptionType":"max","rateLimitTier":"default_claude_max_20x"}}"#;

    #[test]
    fn the_file_store_reads_the_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, PAYLOAD).expect("written");
        let store = CredentialStore::file(path.clone());
        assert_eq!(store.file_path(), path);
        assert_eq!(store.load().expect("loaded").subscription_type, "max");
    }

    #[test]
    fn a_file_store_with_nothing_in_it_is_not_found() {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = CredentialStore::file(dir.path().join("absent.json"));
        assert!(matches!(store.load(), Err(CredentialsError::NotFound)));
    }

    #[test]
    fn a_second_read_comes_from_the_cache_and_a_fresh_one_does_not() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".credentials.json");
        std::fs::write(&path, PAYLOAD).expect("written");
        let store = CredentialStore::file(path.clone());
        assert_eq!(store.load().expect("loaded").subscription_type, "max");

        // The source changes underneath: the cached read still stands...
        std::fs::write(&path, PAYLOAD.replace("max", "pro")).expect("rewritten");
        assert_eq!(store.load().expect("cached").subscription_type, "max");
        // ...and `load_fresh` is what goes back and looks.
        assert_eq!(store.load_fresh().expect("fresh").subscription_type, "pro");
        // Which also replaces what the next cached read hands back.
        assert_eq!(store.load().expect("cached").subscription_type, "pro");
    }

    #[test]
    fn a_failed_read_is_never_cached() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join(".credentials.json");
        let store = CredentialStore::file(path.clone());
        assert!(matches!(store.load(), Err(CredentialsError::NotFound)));

        // Signing in between the two reads has to be noticed straight away.
        std::fs::write(&path, PAYLOAD).expect("written");
        assert!(store.load().is_ok());
    }

    #[cfg(target_os = "macos")]
    mod macos {
        use super::*;
        use crate::keychain::{KeychainId, SecurityOutput};
        use std::sync::atomic::{AtomicUsize, Ordering};

        /// Answers for one named service; anything else is "not found".
        struct FakeSecurity {
            service: String,
            status: Option<i32>,
            stdout: String,
            calls: std::sync::Arc<AtomicUsize>,
        }

        impl SecurityRunner for FakeSecurity {
            fn find_generic_password(&self, id: &KeychainId) -> std::io::Result<SecurityOutput> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                if id.service == self.service {
                    return Ok(SecurityOutput {
                        status: self.status,
                        stdout: self.stdout.clone(),
                    });
                }
                Ok(SecurityOutput {
                    status: Some(crate::keychain::ERR_ITEM_NOT_FOUND),
                    stdout: String::new(),
                })
            }
        }

        fn id(service: &str) -> KeychainId {
            KeychainId {
                service: service.into(),
                account: "u".into(),
            }
        }

        fn store_for(
            service: &str,
            status: Option<i32>,
            stdout: &str,
            ids: Vec<KeychainId>,
            file: PathBuf,
        ) -> (CredentialStore, std::sync::Arc<AtomicUsize>) {
            let calls = std::sync::Arc::new(AtomicUsize::new(0));
            let store = CredentialStore::with_runner(
                ids,
                file,
                Box::new(FakeSecurity {
                    service: service.into(),
                    status,
                    stdout: stdout.to_string(),
                    calls: std::sync::Arc::clone(&calls),
                }),
            );
            (store, calls)
        }

        #[test]
        fn the_keychain_answers_first() {
            let dir = tempfile::tempdir().expect("temp dir");
            let (store, _) = store_for(
                "Claude Code-credentials",
                Some(0),
                PAYLOAD,
                vec![id("Claude Code-credentials")],
                dir.path().join("absent.json"),
            );
            assert_eq!(store.load().expect("loaded").subscription_type, "max");
        }

        #[test]
        fn a_second_name_is_tried_before_the_file() {
            let dir = tempfile::tempdir().expect("temp dir");
            let (store, calls) = store_for(
                "Claude Code-credentials",
                Some(0),
                PAYLOAD,
                vec![
                    id("Claude Code-credentials-deadbeef"),
                    id("Claude Code-credentials"),
                ],
                dir.path().join("absent.json"),
            );
            assert_eq!(store.load().expect("loaded").subscription_type, "max");
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }

        #[test]
        fn an_absent_item_falls_back_to_the_file() {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join(".credentials.json");
            std::fs::write(&path, PAYLOAD).expect("written");
            let (store, _) = store_for(
                "nothing",
                Some(0),
                "",
                vec![id("Claude Code-credentials")],
                path,
            );
            assert!(store.load().is_ok());
        }

        #[test]
        fn a_locked_keychain_with_no_file_says_so_rather_than_not_signed_in() {
            let dir = tempfile::tempdir().expect("temp dir");
            let (store, _) = store_for(
                "Claude Code-credentials",
                Some(crate::keychain::ERR_INTERACTION_NOT_ALLOWED),
                "",
                vec![id("Claude Code-credentials")],
                dir.path().join("absent.json"),
            );
            let error = store.load().expect_err("unavailable");
            assert!(
                matches!(error, CredentialsError::Unavailable(_)),
                "{error:?}"
            );
            assert!(error.to_string().contains("Keychain unavailable"));
        }

        #[test]
        fn a_locked_keychain_stops_before_the_other_names_and_prefers_the_file() {
            let dir = tempfile::tempdir().expect("temp dir");
            let path = dir.path().join(".credentials.json");
            std::fs::write(&path, PAYLOAD).expect("written");
            let (store, calls) = store_for(
                "Claude Code-credentials-deadbeef",
                Some(crate::keychain::ERR_INTERACTION_NOT_ALLOWED),
                "",
                vec![
                    id("Claude Code-credentials-deadbeef"),
                    id("Claude Code-credentials"),
                ],
                path,
            );
            assert!(store.load().is_ok());
            // A Keychain that cannot be asked cannot be asked about the second
            // name either; trying it would only be a second failed subprocess.
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn one_refresh_pass_runs_security_once() {
            let dir = tempfile::tempdir().expect("temp dir");
            let (store, calls) = store_for(
                "Claude Code-credentials",
                Some(0),
                PAYLOAD,
                vec![id("Claude Code-credentials")],
                dir.path().join("absent.json"),
            );
            // What one `refresh_limits` does: the IO probe, then the read.
            assert!(store.load().is_ok());
            assert!(store.load().is_ok());
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            // And the reload after `claude auth status` goes back to the source.
            assert!(store.load_fresh().is_ok());
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
    }
}
