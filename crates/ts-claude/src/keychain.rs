//! Where Claude Code keeps its sign-in on macOS.
//!
//! There is no `.credentials.json` on a normal macOS install: Claude Code puts
//! the same JSON in the login Keychain, under a service name derived from the
//! config directory and an account name derived from the user. The derivation is
//! reproduced here as pure functions, so it can be asserted on any platform.
//!
//! The item is read through `/usr/bin/security`, which is what Claude Code
//! itself uses to write it. That matters: the binary that created the item is on
//! its ACL, so reading it back the same way does not prompt for the login
//! password, while a direct Security-framework call from an unsigned binary
//! would. Nothing here writes, refreshes or logs a credential — not the payload,
//! not a prefix of it, not its length.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Service-name prefix Claude Code uses.
const SERVICE_BASE: &str = "Claude Code";
/// Suffix Claude Code appends to the service name.
const SERVICE_KIND: &str = "-credentials";
/// Account name used when the environment's user name is unusable.
pub const FALLBACK_ACCOUNT: &str = "claude-code-user";
/// How many hex characters of the digest go into the service name.
const DIGEST_CHARS: usize = 8;

/// How long `security` gets before it is killed.
///
/// It normally answers in milliseconds. The bound is here because this runs on
/// the refresh path and a Keychain prompt that nobody is there to answer would
/// otherwise wedge it for good.
pub const SECURITY_TIMEOUT: Duration = Duration::from_secs(10);

/// The absolute path, never a `PATH` lookup: this reads a credential.
pub const SECURITY_BINARY: &str = "/usr/bin/security";

/// `security` exits with this when the item is not in the Keychain.
pub const ERR_ITEM_NOT_FOUND: i32 = 44;
/// `security` exits with this when it cannot ask the user — an SSH session with
/// no GUI Security session, for instance (`errSecInteractionNotAllowed`).
pub const ERR_INTERACTION_NOT_ALLOWED: i32 = 36;

/// Which Keychain item holds the sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeychainId {
    pub service: String,
    pub account: String,
}

/// Why the Keychain did not hand over a credential.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeychainError {
    #[error("no Claude Code sign-in in the login Keychain")]
    NotFound,
    #[error("Keychain unavailable")]
    Unavailable,
    #[error("cannot read the login Keychain: {0}")]
    Failed(String),
}

/// Which directory the service-name suffix is derived from.
///
/// `None` means no suffix at all, which is what Claude Code uses when neither
/// variable is set and the config directory is the default `~/.claude`.
#[derive(Debug, Clone, Default)]
pub struct ConfigOrigin {
    /// `$CLAUDE_SECURESTORAGE_CONFIG_DIR`; an empty value means `~/.claude`.
    pub securestorage_dir: Option<String>,
    /// Token Station's own `claude.config_dir` setting.
    pub setting: String,
    /// `$CLAUDE_CONFIG_DIR`.
    pub config_dir: Option<String>,
}

/// The directory whose path is hashed into the service name, if any.
fn hashed_dir(origin: &ConfigOrigin, home: &Path) -> Option<PathBuf> {
    // `$CLAUDE_SECURESTORAGE_CONFIG_DIR` wins even when it is empty: Claude Code
    // reads "set but empty" as "the default config directory", which still gets
    // a suffix.
    if let Some(dir) = &origin.securestorage_dir {
        let trimmed = dir.trim();
        return Some(if trimmed.is_empty() {
            home.join(".claude")
        } else {
            ts_core::discovery::expand_tilde(trimmed, home)
        });
    }
    // Token Station pointed at a config directory of its own is the same
    // statement as `$CLAUDE_CONFIG_DIR`: Claude Code was signed in against that
    // directory, so its sign-in is under that directory's suffix.
    let setting = origin.setting.trim();
    if !setting.is_empty() {
        return Some(ts_core::discovery::expand_tilde(setting, home));
    }
    match origin.config_dir.as_deref().map(str::trim) {
        Some(dir) if !dir.is_empty() => Some(ts_core::discovery::expand_tilde(dir, home)),
        _ => None,
    }
}

/// The service name of the Keychain item holding the sign-in.
#[must_use]
pub fn service_name(origin: &ConfigOrigin, home: &Path) -> String {
    let suffix = match hashed_dir(origin, home) {
        Some(dir) => format!("-{}", short_digest(dir.as_os_str().as_encoded_bytes())),
        None => String::new(),
    };
    format!("{SERVICE_BASE}{SERVICE_KIND}{suffix}")
}

/// The account name of that item.
///
/// Anything that is not a plain name is replaced rather than escaped: the
/// account is an argument to `security`, and a user name with a space or a
/// quote in it is a name Claude Code itself would not have used.
#[must_use]
pub fn account_name(user: Option<&str>) -> String {
    let usable = user.filter(|name| {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    });
    usable.unwrap_or(FALLBACK_ACCOUNT).to_string()
}

/// Both halves of the item's identity.
#[must_use]
pub fn keychain_id(origin: &ConfigOrigin, home: &Path, user: Option<&str>) -> KeychainId {
    KeychainId {
        service: service_name(origin, home),
        account: account_name(user),
    }
}

/// Every item that might hold the sign-in, in the order to try them.
///
/// Usually one. A second appears when the config directory is the default
/// `~/.claude` and something still named it explicitly — Token Station's own
/// `claude.config_dir`, or `$CLAUDE_CONFIG_DIR` — because Claude Code only
/// derives the suffix from the variables *it* was run with. A user who set
/// `claude.config_dir = "~/.claude"` in Token Station, and signed in from a
/// shell that set neither variable, has an unsuffixed item; looking only under
/// the suffix would report them as not signed in.
#[must_use]
pub fn candidates(origin: &ConfigOrigin, home: &Path, user: Option<&str>) -> Vec<KeychainId> {
    let primary = keychain_id(origin, home, user);
    let default_dir = hashed_dir(origin, home).is_some_and(|dir| dir == home.join(".claude"));
    let unsuffixed = format!("{SERVICE_BASE}{SERVICE_KIND}");
    if !default_dir || primary.service == unsuffixed {
        return vec![primary];
    }
    let account = primary.account.clone();
    vec![
        primary,
        KeychainId {
            service: unsuffixed,
            account,
        },
    ]
}

/// The environment's [`ConfigOrigin`].
#[must_use]
pub fn origin_of(config_dir_setting: &str, env: &crate::env::ClaudeEnv) -> ConfigOrigin {
    ConfigOrigin {
        securestorage_dir: env.claude_securestorage_config_dir.clone(),
        setting: config_dir_setting.to_string(),
        config_dir: env.claude_config_dir.clone(),
    }
}

/// The items Claude Code might have written its sign-in to, for this
/// provider's configuration and environment.
///
/// Only macOS reads the result, but it is derived on every platform so the
/// derivation is tested where the CI runner is.
#[must_use]
pub fn for_env(config_dir_setting: &str, env: &crate::env::ClaudeEnv) -> Vec<KeychainId> {
    let origin = origin_of(config_dir_setting, env);
    candidates(&origin, &env.home, env.search.user.as_deref())
}

/// The first [`DIGEST_CHARS`] hex characters of the SHA-256 of `bytes`.
fn short_digest(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    hex(digest.as_ref().iter().take(DIGEST_CHARS.div_ceil(2)))
}

/// Lowercase hexadecimal, the way `sha256sum` prints it.
fn hex<'a>(bytes: impl IntoIterator<Item = &'a u8>) -> String {
    use std::fmt::Write;
    bytes.into_iter().fold(String::new(), |mut out, byte| {
        // Writing into a `String` cannot fail.
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// What `security` said, without ever exposing the secret it printed.
pub struct SecurityOutput {
    /// `None` when the process was killed by a signal or by the timeout.
    pub status: Option<i32>,
    /// The item's payload. Never logged and never put into an error.
    pub stdout: String,
}

/// Runs `security find-generic-password`. Replaced in tests.
pub trait SecurityRunner: Send + Sync {
    /// Read the password of the item `id` names.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] when the binary cannot be started or its
    /// output cannot be collected.
    fn find_generic_password(&self, id: &KeychainId) -> std::io::Result<SecurityOutput>;
}

/// Turn one run of `security` into a credential payload.
///
/// # Errors
///
/// Returns [`KeychainError::NotFound`] for exit 44, [`KeychainError::Unavailable`]
/// for exit 36, and [`KeychainError::Failed`] for every other status — including
/// the timeout, which reports no status at all. No exit code path carries the
/// payload into the error.
pub fn interpret(output: &SecurityOutput) -> Result<String, KeychainError> {
    match output.status {
        Some(0) => Ok(output.stdout.trim().to_string()),
        Some(ERR_ITEM_NOT_FOUND) => Err(KeychainError::NotFound),
        Some(ERR_INTERACTION_NOT_ALLOWED) => Err(KeychainError::Unavailable),
        Some(code) => Err(KeychainError::Failed(format!(
            "{SECURITY_BINARY} exited with {code}"
        ))),
        None => Err(KeychainError::Failed(format!(
            "{SECURITY_BINARY} did not finish within {} s",
            SECURITY_TIMEOUT.as_secs()
        ))),
    }
}

/// Read the sign-in out of the Keychain. Blocking.
///
/// # Errors
///
/// Returns [`KeychainError::Failed`] when `security` cannot be run at all, plus
/// whatever [`interpret`] reports.
pub fn read(runner: &dyn SecurityRunner, id: &KeychainId) -> Result<String, KeychainError> {
    let output = runner
        .find_generic_password(id)
        .map_err(|error| KeychainError::Failed(format!("cannot run {SECURITY_BINARY}: {error}")))?;
    let payload = interpret(&output)?;
    if payload.is_empty() {
        return Err(KeychainError::NotFound);
    }
    Ok(payload)
}

/// The real `/usr/bin/security`.
#[cfg(target_os = "macos")]
pub struct Security;

#[cfg(target_os = "macos")]
impl SecurityRunner for Security {
    fn find_generic_password(&self, id: &KeychainId) -> std::io::Result<SecurityOutput> {
        use std::io::Read;
        use std::process::{Command, Stdio};

        let mut child = Command::new(SECURITY_BINARY)
            .args(["find-generic-password", "-a"])
            .arg(&id.account)
            .arg("-w")
            .arg("-s")
            .arg(&id.service)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            // Its diagnostics are not ours to relay, and one of them is the
            // item's own description.
            .stderr(Stdio::null())
            .spawn()?;

        // Read on a thread of its own: a child that fills the pipe and a parent
        // that waits before reading is the usual way to deadlock the two.
        let reader = child.stdout.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let mut buffer = String::new();
                let _ = pipe.read_to_string(&mut buffer);
                buffer
            })
        });

        let status = wait_or_kill(&mut child, SECURITY_TIMEOUT);
        // Joined, not raced against a grace period: the process has exited or
        // been killed, so its end of the pipe is closed and the read is already
        // over or about to be. Giving up on it early would turn a slow read
        // into an empty payload, which reads as "not signed in" — the one
        // answer that sends the user off to sign in again for no reason.
        let stdout = reader.map(|reader| reader.join().unwrap_or_default());
        Ok(SecurityOutput {
            status,
            stdout: stdout.unwrap_or_default(),
        })
    }
}

/// Wait for `child`, killing it after `timeout`. `None` means it was killed.
#[cfg(target_os = "macos")]
fn wait_or_kill(child: &mut std::process::Child, timeout: Duration) -> Option<i32> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.code(),
            Ok(None) => {}
            Err(_) => return None,
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> PathBuf {
        PathBuf::from("/Users/u")
    }

    fn origin(securestorage: Option<&str>, setting: &str, config: Option<&str>) -> ConfigOrigin {
        ConfigOrigin {
            securestorage_dir: securestorage.map(str::to_string),
            setting: setting.to_string(),
            config_dir: config.map(str::to_string),
        }
    }

    #[test]
    fn a_default_install_has_no_suffix() {
        assert_eq!(
            service_name(&origin(None, "", None), &home()),
            "Claude Code-credentials"
        );
    }

    #[test]
    fn a_custom_config_dir_adds_eight_hex_characters() {
        let service = service_name(&origin(None, "", Some("/Users/u/work/.claude")), &home());
        let suffix = service
            .strip_prefix("Claude Code-credentials-")
            .expect("a suffix");
        assert_eq!(suffix.len(), 8);
        assert!(
            suffix
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "{suffix}"
        );
    }

    #[test]
    fn the_suffix_is_the_sha256_of_the_path() {
        // sha256("/Users/u/work/.claude"), first four bytes.
        let expected = {
            let digest = ring::digest::digest(&ring::digest::SHA256, b"/Users/u/work/.claude");
            hex(digest.as_ref().iter().take(4))
        };
        assert_eq!(
            service_name(&origin(None, "", Some("/Users/u/work/.claude")), &home()),
            format!("Claude Code-credentials-{expected}")
        );
    }

    #[test]
    fn securestorage_wins_and_an_empty_value_means_the_default_dir() {
        // Set but empty is not the same as unset: it names `~/.claude`, which
        // still gets a suffix.
        let empty = service_name(&origin(Some(""), "", None), &home());
        assert_ne!(empty, "Claude Code-credentials");
        assert_eq!(
            empty,
            service_name(&origin(Some("/Users/u/.claude"), "", None), &home())
        );
        // And it beats both the setting and `$CLAUDE_CONFIG_DIR`.
        assert_eq!(
            service_name(&origin(Some("/a"), "/b", Some("/c")), &home()),
            service_name(&origin(Some("/a"), "", None), &home())
        );
    }

    #[test]
    fn token_stations_own_setting_counts_as_a_custom_config_dir() {
        let by_setting = service_name(&origin(None, "~/work/.claude", None), &home());
        let by_env = service_name(&origin(None, "", Some("/Users/u/work/.claude")), &home());
        assert_eq!(by_setting, by_env, "the tilde has to expand the same way");
    }

    #[test]
    fn the_id_for_an_environment_uses_the_same_config_directory() {
        let env = crate::env::ClaudeEnv {
            home: home(),
            claude_config_dir: None,
            claude_securestorage_config_dir: None,
            api_base_url: String::new(),
            claude_binary: None,
            search: ts_core::discovery::SearchEnv {
                path: None,
                home: home(),
                user: Some("u".into()),
            },
        };

        let default = for_env("", &env);
        assert_eq!(default.len(), 1);
        assert_eq!(default[0].service, "Claude Code-credentials");
        assert_eq!(default[0].account, "u");

        let custom = for_env("~/work/.claude", &env);
        assert_eq!(custom.len(), 1);
        assert_eq!(
            custom[0].service,
            service_name(&origin(None, "~/work/.claude", None), &home())
        );
    }

    #[test]
    fn naming_the_default_directory_also_tries_the_unsuffixed_item() {
        // Claude Code derives the suffix from the variables it was run with, so
        // a user who told Token Station where the directory is — and signed in
        // from a shell that said nothing — has an unsuffixed item.
        let ids = candidates(&origin(None, "~/.claude", None), &home(), Some("u"));
        assert_eq!(ids.len(), 2);
        assert!(ids[0].service.starts_with("Claude Code-credentials-"));
        assert_eq!(ids[1].service, "Claude Code-credentials");
        assert_eq!(ids[1].account, "u");

        // Same for `$CLAUDE_SECURESTORAGE_CONFIG_DIR` set but empty, and for
        // `$CLAUDE_CONFIG_DIR` pointed at the default.
        assert_eq!(
            candidates(&origin(Some(""), "", None), &home(), None).len(),
            2
        );
        assert_eq!(
            candidates(&origin(None, "", Some("/Users/u/.claude")), &home(), None).len(),
            2
        );

        // A directory that is genuinely elsewhere has one item, and so does a
        // plain install: there is no second name to try.
        assert_eq!(
            candidates(
                &origin(None, "", Some("/Users/u/work/.claude")),
                &home(),
                None
            )
            .len(),
            1
        );
        assert_eq!(candidates(&origin(None, "", None), &home(), None).len(), 1);
    }

    #[test]
    fn the_account_is_the_user_name_or_a_safe_stand_in() {
        assert_eq!(account_name(Some("psoldunov")), "psoldunov");
        assert_eq!(account_name(Some("a.b_c-1")), "a.b_c-1");
        for unusable in ["", "with space", "quote\"", "sl/ash", "ümlaut", "$(id)"] {
            assert_eq!(account_name(Some(unusable)), FALLBACK_ACCOUNT, "{unusable}");
        }
        assert_eq!(account_name(None), FALLBACK_ACCOUNT);
    }

    /// A `security` that answers with whatever the test asked for.
    struct FakeSecurity {
        status: Option<i32>,
        stdout: String,
    }

    impl SecurityRunner for FakeSecurity {
        fn find_generic_password(&self, _id: &KeychainId) -> std::io::Result<SecurityOutput> {
            Ok(SecurityOutput {
                status: self.status,
                stdout: self.stdout.clone(),
            })
        }
    }

    fn run(status: Option<i32>, stdout: &str) -> Result<String, KeychainError> {
        read(
            &FakeSecurity {
                status,
                stdout: stdout.to_string(),
            },
            &KeychainId {
                service: "Claude Code-credentials".into(),
                account: "u".into(),
            },
        )
    }

    #[test]
    fn exit_codes_map_to_the_three_answers_that_matter() {
        assert_eq!(
            run(Some(0), "{\"claudeAiOauth\":{}}\n").expect("found"),
            "{\"claudeAiOauth\":{}}"
        );
        assert_eq!(run(Some(44), ""), Err(KeychainError::NotFound));
        assert_eq!(run(Some(36), ""), Err(KeychainError::Unavailable));
        assert_eq!(
            run(Some(36), "").unwrap_err().to_string(),
            "Keychain unavailable"
        );
        assert!(matches!(run(Some(1), ""), Err(KeychainError::Failed(_))));
        // Killed by the timeout: no status, and the message says as much.
        let timed_out = run(None, "").unwrap_err();
        assert!(
            timed_out.to_string().contains("did not finish"),
            "{timed_out}"
        );
    }

    #[test]
    fn an_empty_payload_is_not_a_credential() {
        assert_eq!(run(Some(0), "   \n"), Err(KeychainError::NotFound));
    }

    #[test]
    fn a_failure_never_carries_the_payload() {
        // Whatever `security` printed, the error text is about the exit code.
        let secret = "sk-ant-oat01-SECRET";
        for status in [Some(1), Some(36), Some(44), None] {
            let error = run(status, secret).unwrap_err().to_string();
            assert!(!error.contains(secret), "{error}");
        }
    }
}
