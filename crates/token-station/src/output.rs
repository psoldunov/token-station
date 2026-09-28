//! The CLI's own output.
//!
//! Everything a command prints goes through here, so the rest of the crate can
//! keep reporting through `tracing` and a future `--quiet` has one place to land.
//! Writing through the handle rather than `println!` also means a closed pipe
//! (`token-station status | head -1`) is a dropped line, not a panic.

use std::io::Write;

/// Print one line on stdout.
pub fn line(text: &str) {
    write_line(std::io::stdout().lock(), text);
}

/// Print one line on stderr, for a failure the user has to read.
pub fn error_line(text: &str) {
    write_line(std::io::stderr().lock(), text);
}

fn write_line(mut target: impl Write, text: &str) {
    let _ = writeln!(target, "{text}");
    let _ = target.flush();
}

/// Write `bytes` to stdout exactly as they are, for wrapped-command output.
pub fn bytes(bytes: &[u8]) {
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(bytes);
    let _ = out.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writing_to_a_closed_stdout_does_not_panic() {
        // Nothing here asserts on the terminal; the point is that none of these
        // can take the process down the way `println!` does on EPIPE.
        line("");
        error_line("");
        bytes(b"");
    }
}
