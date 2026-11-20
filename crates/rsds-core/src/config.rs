//! Files in the config directory, `$XDG_CONFIG_HOME/rsds` (by default
//! `~/.config/rsds`): JSON, written by the UI (and Nightlight's by the
//! daemon's schedule), watched by both.

use std::{
    io,
    os::fd::{AsFd, OwnedFd},
    path::{Path, PathBuf},
};

use rustix::fs::inotify;
use serde::{Serialize, de::DeserializeOwned};

/// `$XDG_CONFIG_HOME/rsds`, defaulting to `~/.config/rsds`.
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("rsds"))
}

/// A settings file in the config directory. A missing file reads as the
/// defaults.
pub trait ConfigFile: Serialize + DeserializeOwned + Default {
    /// File name in the config directory.
    const NAME: &'static str;

    fn path() -> io::Result<PathBuf> {
        dir().map(|d| d.join(Self::NAME)).ok_or_else(|| io::Error::other("no home directory"))
    }

    fn load() -> io::Result<Self> {
        Self::load_from(&Self::path()?)
    }

    fn load_from(path: &Path) -> io::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| io::Error::other(format!("{}: {e}", path.display()))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    fn save(&self) -> io::Result<()> {
        self.save_to(&Self::path()?)
    }

    fn save_to(&self, path: &Path) -> io::Result<()> {
        write_atomic(path, &serde_json::to_vec_pretty(self).map_err(io::Error::other)?)
    }
}

/// Write a file atomically: to a temporary file, then rename over the old
/// one, so readers never see a half-written file.
pub fn write_atomic(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// A non-blocking inotify fd that becomes readable when a file in the
/// config directory is written; then [`drain`] it and reload.
pub fn watch() -> io::Result<OwnedFd> {
    let dir = dir().ok_or_else(|| io::Error::other("no home directory"))?;
    std::fs::create_dir_all(&dir)?;
    let fd = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)?;
    // Files are written atomically (rename), so MOVED_TO covers ours;
    // CLOSE_WRITE covers editing a file in place.
    inotify::add_watch(&fd, &dir, inotify::WatchFlags::MOVED_TO | inotify::WatchFlags::CLOSE_WRITE)?;
    Ok(fd)
}

/// Read and discard everything pending on a non-blocking fd (an inotify
/// fd, or a timerfd, whose read can fail with ECANCELED after a clock
/// change: that's fine, the caller re-evaluates either way).
pub fn drain(fd: impl AsFd) {
    let mut buf = [0u8; 4096];
    while rustix::io::read(&fd, &mut buf).is_ok_and(|n| n > 0) {}
}
