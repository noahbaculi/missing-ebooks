//! Read-only benchmark: time the real scanner against the configured library
//! roots, local disk versus an SMB (CIFS) mount.
//!
//! `cargo run --release --example scan_bench -- --config config.toml --label smb --drop-caches`
//! loads the real `Config`, compiles `ScanSettings`, and times `scanner::scan`
//! (gaps-only) and `scanner::scan_all` (full walk) per root, in cold and warm
//! cache conditions, then saves a JSON report. The walks only read directory
//! entries and names; nothing here writes to the library. The single privileged
//! action is the optional `--drop-caches` page-cache flush on Linux.

use std::process::ExitCode;

fn main() -> ExitCode {
    // Wired in Task 11. The skeleton compiles so earlier tasks can add and test
    // pure helpers against a real target.
    eprintln!("scan_bench is not wired up yet");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {}
