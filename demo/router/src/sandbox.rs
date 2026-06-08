//! Spawning, readiness-polling, and teardown of `explore` sandboxes, plus the
//! startup sweep of leftover temp directories. `explore` seeds a temp dir under
//! /tmp/explore-* and removes it on SIGINT (its graceful-shutdown signal), so
//! teardown sends SIGINT, and the sweep is a backstop for dirs left by a process
//! that died another way.

use std::path::Path;
use std::time::Duration;

use anyhow::Context;
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;

/// What a successful spawn yields: the child handle (kept so the OS does not
/// reap it out from under us) and its pid for signalling.
pub struct Spawned {
    pub child: tokio::process::Child,
    pub pid: u32,
}

/// Spawning is behind a trait so the proxy can be exercised against a fake that
/// points at an already-running stub server instead of launching a process.
#[async_trait::async_trait]
pub trait Launcher: Send + Sync {
    /// Launch a sandbox serving `scenario` on `port`, returning once it answers
    /// on `GET /` or erroring on timeout.
    async fn launch(&self, scenario: &str, port: u16, ready_timeout: Duration)
        -> anyhow::Result<Spawned>;
}

/// The production launcher: runs the compiled `explore` binary.
pub struct RealLauncher {
    pub explore_bin: String,
    /// The shared client, used here only to poll a new sandbox for readiness.
    pub client: reqwest::Client,
}

#[async_trait::async_trait]
impl Launcher for RealLauncher {
    async fn launch(
        &self,
        scenario: &str,
        port: u16,
        ready_timeout: Duration,
    ) -> anyhow::Result<Spawned> {
        let child = tokio::process::Command::new(&self.explore_bin)
            .arg(scenario)
            .arg("--port")
            .arg(port.to_string())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawning {} {scenario}", self.explore_bin))?;
        let pid = child.id().context("spawned child has no pid")?;
        wait_ready(&self.client, port, ready_timeout).await?;
        Ok(Spawned { child, pid })
    }
}

/// Poll `GET http://127.0.0.1:{port}/` until it returns any HTTP response or the
/// timeout elapses.
pub async fn wait_ready(client: &reqwest::Client, port: u16, timeout: Duration) -> anyhow::Result<()> {
    let url = format!("http://127.0.0.1:{port}/");
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if client.get(&url).send().await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("sandbox on port {port} did not become ready in {timeout:?}");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Send SIGINT to a sandbox so `explore` runs its graceful shutdown and removes
/// its temp directory. A failure here (already gone) is logged, not fatal.
pub fn shutdown(pid: u32) {
    if let Err(err) = kill(Pid::from_raw(pid as i32), Signal::SIGINT) {
        tracing::warn!(pid, %err, "failed to SIGINT sandbox; relying on temp sweep");
    }
}

/// Remove leftover `explore-*` directories under `tmp_root`. Run at startup: the
/// router is its container's main process, so a restart already reaps child
/// processes, leaving only their temp dirs to clear.
pub fn sweep_temp_dirs(tmp_root: &Path) -> anyhow::Result<usize> {
    let mut removed = 0;
    let entries = match std::fs::read_dir(tmp_root) {
        Ok(entries) => entries,
        // No temp root yet is fine: nothing to sweep.
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).context("reading temp root"),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("explore-") && entry.path().is_dir() {
            if std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sweep_removes_only_explore_dirs() {
        let tmp = tempdir();
        std::fs::create_dir(tmp.join("explore-abc")).unwrap();
        std::fs::create_dir(tmp.join("explore-def")).unwrap();
        std::fs::create_dir(tmp.join("keep-me")).unwrap();

        let removed = sweep_temp_dirs(&tmp).unwrap();

        assert_eq!(removed, 2);
        assert!(!tmp.join("explore-abc").exists());
        assert!(tmp.join("keep-me").exists());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn sweep_of_missing_root_is_ok() {
        assert_eq!(sweep_temp_dirs(Path::new("/no/such/dir")).unwrap(), 0);
    }

    /// A unique temp directory for one test, without pulling in the tempfile
    /// crate as a dependency of the router.
    fn tempdir() -> std::path::PathBuf {
        let mut buf = [0u8; 8];
        getrandom::getrandom(&mut buf).unwrap();
        let dir = std::env::temp_dir().join(format!("router-test-{}", hex(&buf)));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}
