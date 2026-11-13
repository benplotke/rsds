//! A lock file held by the running daemon, so only one daemon runs at a
//! time and the UI can tell whether one is running.

use std::{fs::File, io, path::PathBuf};

use rustix::fs::{FlockOperation, flock};

/// `$XDG_RUNTIME_DIR/rsds-daemon.lock` (falls back to /tmp, per user).
fn path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => PathBuf::from(dir).join("rsds-daemon.lock"),
        None => PathBuf::from(format!("/tmp/rsds-daemon-{}.lock", rustix::process::getuid().as_raw())),
    }
}

/// Held for as long as the daemon runs; released when dropped or on exit.
pub struct DaemonLock(#[allow(dead_code)] File);

/// Take the lock. Ok(None) if another daemon already holds it.
pub fn acquire() -> io::Result<Option<DaemonLock>> {
    let file = File::options().create(true).truncate(false).write(true).open(path())?;
    match flock(&file, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(Some(DaemonLock(file))),
        Err(rustix::io::Errno::WOULDBLOCK) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Whether a daemon currently holds the lock.
pub fn is_running() -> bool {
    let Ok(file) = File::open(path()) else { return false };
    match flock(&file, FlockOperation::NonBlockingLockShared) {
        Ok(()) => false, // We got it, so nobody holds it exclusively.
        Err(e) => e == rustix::io::Errno::WOULDBLOCK,
    }
}
