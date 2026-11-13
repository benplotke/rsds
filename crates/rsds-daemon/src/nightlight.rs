//! Nightlight in the daemon: follows the settings file (via inotify) and the
//! schedule (via a wall-clock timer, which also fires correctly after
//! suspend), and tells the backend which colour temperature to apply.

use std::{
    io,
    os::fd::{AsFd, BorrowedFd, OwnedFd},
};

use rsds_core::{
    Backend,
    nightlight::{Nightlight, TimeOfDay},
    profile::config_dir,
};
use rustix::{
    fs::inotify,
    time::{Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec},
};

use crate::log;

pub(crate) struct Service {
    settings: Nightlight,
    /// Colour temperature currently applied (None: no tint).
    applied: Option<u32>,
    inotify: OwnedFd,
    timer: OwnedFd,
}

impl Service {
    pub(crate) fn new() -> io::Result<Self> {
        let dir = config_dir().ok_or_else(|| io::Error::other("no home directory"))?;
        std::fs::create_dir_all(&dir)?;
        let inotify = inotify::init(inotify::CreateFlags::CLOEXEC | inotify::CreateFlags::NONBLOCK)?;
        // Settings are written atomically (rename), so MOVED_TO covers the
        // UI; CLOSE_WRITE covers editing the file in place.
        inotify::add_watch(&inotify, &dir, inotify::WatchFlags::MOVED_TO | inotify::WatchFlags::CLOSE_WRITE)?;
        let timer = rustix::time::timerfd_create(
            TimerfdClockId::Realtime,
            TimerfdFlags::CLOEXEC | TimerfdFlags::NONBLOCK,
        )?;
        Ok(Self { settings: load(), applied: None, inotify, timer })
    }

    pub(crate) fn fds(&self) -> [BorrowedFd<'_>; 2] {
        [self.inotify.as_fd(), self.timer.as_fd()]
    }

    /// Something in the config directory changed: reload the settings.
    pub(crate) fn config_changed<B: Backend>(&mut self, backend: &mut B) {
        drain(&self.inotify);
        let settings = load();
        if settings != self.settings {
            self.settings = settings;
            self.update(backend);
        }
    }

    /// The schedule timer fired (or the clock was changed).
    pub(crate) fn timer_fired<B: Backend>(&mut self, backend: &mut B) {
        drain(&self.timer);
        self.update(backend);
    }

    /// Apply the tint the settings call for right now, and arm the timer
    /// for the next scheduled change.
    pub(crate) fn update<B: Backend>(&mut self, backend: &mut B) {
        let (now, epoch) = local_now();
        let want = self.settings.active_at(now).then_some(self.settings.temperature);
        if want != self.applied {
            match backend.set_color_temperature(want) {
                Ok(()) => {
                    match want {
                        Some(k) => log!("nightlight on ({k} K)"),
                        None => log!("nightlight off"),
                    }
                    self.applied = want;
                }
                Err(e) => log!("nightlight failed: {e}"),
            }
        }

        let next = (self.settings.enabled && self.settings.schedule)
            .then(|| next_change(epoch, &[self.settings.start, self.settings.end]));
        let spec = Itimerspec {
            it_interval: Timespec { tv_sec: 0, tv_nsec: 0 },
            // All zero disarms the timer.
            it_value: Timespec { tv_sec: next.unwrap_or(0), tv_nsec: 0 },
        };
        let flags = TimerfdTimerFlags::ABSTIME | TimerfdTimerFlags::CANCEL_ON_SET;
        if let Err(e) = rustix::time::timerfd_settime(&self.timer, flags, &spec) {
            log!("can't set nightlight timer: {e}");
        }
    }
}

fn load() -> Nightlight {
    Nightlight::load().unwrap_or_else(|e| {
        log!("can't read nightlight settings: {e}");
        Nightlight::default()
    })
}

/// Read and discard everything pending on a non-blocking fd. (For the
/// timer, a read can fail with ECANCELED after a clock change; that's fine,
/// we re-evaluate either way.)
fn drain(fd: &OwnedFd) {
    let mut buf = [0u8; 4096];
    while let Ok(n) = rustix::io::read(fd, &mut buf) {
        if n == 0 {
            break;
        }
    }
}

/// Current local time of day, and the current time in seconds since the epoch.
fn local_now() -> (TimeOfDay, i64) {
    // SAFETY: time(NULL) and localtime_r with valid pointers.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        (TimeOfDay((tm.tm_hour * 60 + tm.tm_min) as u16), now)
    }
}

/// The next moment (seconds since the epoch, strictly after `now`) at which
/// local time reaches one of `times`. Uses mktime so DST is handled.
fn next_change(now: i64, times: &[TimeOfDay]) -> i64 {
    times
        .iter()
        .map(|t| {
            // SAFETY: localtime_r/mktime with valid pointers.
            unsafe {
                let mut tm: libc::tm = std::mem::zeroed();
                libc::localtime_r(&now, &mut tm);
                tm.tm_hour = t.hour() as i32;
                tm.tm_min = t.minute() as i32;
                tm.tm_sec = 0;
                tm.tm_isdst = -1;
                let mut at = libc::mktime(&mut tm);
                if at <= now {
                    tm.tm_mday += 1; // mktime normalises the date.
                    tm.tm_isdst = -1;
                    at = libc::mktime(&mut tm);
                }
                at
            }
        })
        .min()
        .unwrap_or(now + 86400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_change_is_soonest_future_time() {
        let (now_tod, now) = local_now();
        let soon = TimeOfDay((now_tod.0 + 2) % TimeOfDay::MINUTES_PER_DAY);
        let later = TimeOfDay((now_tod.0 + 600) % TimeOfDay::MINUTES_PER_DAY);
        let next = next_change(now, &[later, soon]);
        // Within ~2 minutes (seconds of the current minute already elapsed),
        // allowing an hour either way in case a DST change falls in between.
        assert!(next > now && next <= now + 120 + 3600, "{}", next - now);
        // A time equal to now (this minute) rolls over to tomorrow.
        let next = next_change(now - now % 60 + 60, &[TimeOfDay((now_tod.0 + 1) % TimeOfDay::MINUTES_PER_DAY)]);
        assert!(next - now > 20 * 3600, "{}", next - now);
    }
}
