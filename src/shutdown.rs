//! Process-lifecycle signal handling. Resolves on the first relevant
//! termination signal so `axum::serve` can drain in-flight requests before
//! exit. Listens for SIGINT and SIGTERM on Unix, Ctrl-C on Windows.

/// Resolves when the process receives SIGINT or SIGTERM (Unix) or Ctrl-C
/// (Windows). Logs which signal fired before returning. Intended to be
/// passed to `axum::serve(..).with_graceful_shutdown(..)`.
/// When the SIGTERM handler cannot be installed, logs a warning and waits
/// on SIGINT alone rather than taking the server down.
pub async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal as unix_signal};
        let mut term = match unix_signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "could not install SIGTERM handler, waiting for SIGINT only"
                );
                let _ = tokio::signal::ctrl_c().await;
                tracing::info!("received SIGINT, shutting down");
                return;
            }
        };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => tracing::info!("received SIGINT, shutting down"),
            _ = term.recv() => tracing::info!("received SIGTERM, shutting down"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("received Ctrl-C, shutting down");
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn sigterm_resolves_signal_future() {
        // Spawn the signal future, then raise SIGTERM at this process. The
        // future must resolve before the timeout.
        let task = tokio::spawn(signal());
        // Yield once so the signal handler is installed before we raise.
        tokio::task::yield_now().await;
        // SAFETY: libc::raise sends a signal to the current process, safe to
        // call from any thread. We have just installed a handler that will
        // observe SIGTERM and complete the spawned future.
        unsafe {
            libc::raise(libc::SIGTERM);
        }
        tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .expect("signal() did not resolve within 500ms after SIGTERM")
            .expect("signal() task panicked");
    }
}
