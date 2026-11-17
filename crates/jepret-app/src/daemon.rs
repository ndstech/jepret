//! Daemon: single instance, tray, global hotkeys, message loop. Owner: **T12**.
//!
//! * Single instance via named mutex `Local\Jepret.Daemon`; second instance
//!   forwards its command (e.g. `capture region`) over a named pipe or
//!   WM_COPYDATA to the first one, then exits.
//! * Hotkeys from `config.hotkeys`; conflicts → notification, not panic.
//! * Each hotkey → `dispatch::handle(kind, actions)` on a worker thread,
//!   except overlay which needs a UI thread (T12 decides the threading).
//! * Startup: cleanup on background thread if `history.cleanup.run_on_startup`.
//! * Reload config when `config.toml` changes.

pub fn run() -> anyhow::Result<()> {
    anyhow::bail!("TODO(T12): daemon::run")
}

/// Forward `capture <kind>` to a running daemon, or run it in-process once.
pub fn send_or_run_capture(_kind: &str) -> anyhow::Result<()> {
    let _ = super::dispatch::handle;
    anyhow::bail!("TODO(T12): send_or_run_capture")
}
