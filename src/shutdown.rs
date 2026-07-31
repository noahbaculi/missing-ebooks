//! Process-lifecycle signal handling. Resolves on the first relevant
//! termination signal so `axum::serve` can drain in-flight requests before
//! exit. Listens for SIGINT and SIGTERM on Unix, Ctrl-C on Windows.

/// Await ctrl_c(), logging `received_msg` on receipt or `install_failed_msg`
/// on an install failure. Returns whether the signal was actually received,
/// so callers pick their own fallback instead of this deciding for them
async fn await_ctrl_c(received_msg: &str, install_failed_msg: &str) -> bool {
    match tokio::signal::ctrl_c().await {
        Ok(()) => {
            tracing::info!("{received_msg}");
            true
        }
        Err(err) => {
            tracing::warn!(error = %err, "{install_failed_msg}");
            false
        }
    }
}

/// Resolves when the process receives SIGINT or SIGTERM (Unix) or Ctrl-C
/// (Windows). Logs which signal fired before returning. Intended to be
/// passed to `axum::serve(..).with_graceful_shutdown(..)`.
/// When a signal handler cannot be installed, this keeps waiting on
/// whichever handler remains rather than resolving immediately. When no
/// handler at all is available, it stays pending forever instead of
/// stopping the server on a phantom signal.
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
                if !await_ctrl_c(
                    "received SIGINT, shutting down",
                    "could not install the SIGINT handler either; signal-driven shutdown is disabled",
                )
                .await
                {
                    std::future::pending::<()>().await;
                }
                return;
            }
        };
        tokio::select! {
            received = await_ctrl_c(
                "received SIGINT, shutting down",
                "could not install SIGINT handler, waiting for SIGTERM only",
            ) => if !received {
                term.recv().await;
                tracing::info!("received SIGTERM, shutting down");
            },
            _ = term.recv() => tracing::info!("received SIGTERM, shutting down"),
        }
    }
    #[cfg(not(unix))]
    {
        if !await_ctrl_c(
            "received Ctrl-C, shutting down",
            "could not install the Ctrl-C handler; signal-driven shutdown is disabled",
        )
        .await
        {
            std::future::pending::<()>().await;
        }
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
