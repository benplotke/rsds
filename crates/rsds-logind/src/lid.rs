//! The laptop lid, through systemd-logind: whether it is closed, change
//! notifications, and the inhibitor that stops logind acting on it.

use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd},
    time::Duration,
};

use dbus::{
    blocking::{Connection, stdintf::org_freedesktop_dbus::Properties},
    channel::{BusType, Channel},
};

const TIMEOUT: Duration = Duration::from_secs(2);
const LOGIND: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";

/// Whether the lid is closed right now. False if there is no lid or logind
/// can't be asked.
pub fn is_closed() -> bool {
    Connection::new_system().is_ok_and(|bus| lid_closed(&bus).unwrap_or(false))
}

fn lid_closed(bus: &Connection) -> io::Result<bool> {
    bus.with_proxy(LOGIND, PATH, TIMEOUT).get(MANAGER, "LidClosed").map_err(io::Error::other)
}

/// A connection to logind that is notified when the lid opens or closes.
pub struct Lid {
    bus: Connection,
}

/// While held, logind ignores the lid switch (it doesn't suspend on close).
pub struct Inhibitor {
    _fd: dbus::arg::OwnedFd,
}

impl Lid {
    /// Watch the lid. Without a lid, it simply never reads as closed.
    pub fn watch() -> io::Result<Self> {
        let mut channel = Channel::get_private(BusType::System).map_err(io::Error::other)?;
        channel.set_watch_enabled(true);
        let bus = Connection::from(channel);
        bus.add_match_no_cb(&format!(
            "type='signal',sender='{LOGIND}',path='{PATH}',\
             interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"
        ))
        .map_err(io::Error::other)?;
        Ok(Self { bus })
    }

    /// Becomes readable when logind sends a notification; then call
    /// [`Lid::is_closed`].
    pub fn fd(&self) -> BorrowedFd<'_> {
        // SAFETY: the connection owns this fd and outlives the borrow.
        unsafe { BorrowedFd::borrow_raw(self.bus.channel().watch().fd.as_raw_fd()) }
    }

    /// Read pending notifications and return whether the lid is closed.
    pub fn is_closed(&self) -> io::Result<bool> {
        loop {
            self.drain()?;
            let closed = lid_closed(&self.bus)?;
            // A notification that arrived during the query may be newer.
            if self.drain()? == 0 {
                return Ok(closed);
            }
        }
    }

    /// Discard queued notifications; returns how many there were.
    fn drain(&self) -> io::Result<usize> {
        let channel = self.bus.channel();
        channel
            .read_write(Some(Duration::ZERO))
            .map_err(|()| io::Error::other("lost the D-Bus connection"))?;
        let mut n = 0;
        while channel.pop_message().is_some() {
            n += 1;
        }
        Ok(n)
    }

    /// Ask logind not to act on the lid switch until the result is dropped.
    pub fn inhibit(&self) -> io::Result<Inhibitor> {
        let (fd,): (dbus::arg::OwnedFd,) = self
            .bus
            .with_proxy(LOGIND, PATH, TIMEOUT)
            .method_call(
                MANAGER,
                "Inhibit",
                ("handle-lid-switch", "rsds", "rsds handles the lid as set in its Lid Close settings", "block"),
            )
            .map_err(io::Error::other)?;
        Ok(Inhibitor { _fd: fd })
    }

    /// Suspend the machine now.
    pub fn suspend(&self) -> io::Result<()> {
        self.bus
            .with_proxy(LOGIND, PATH, TIMEOUT)
            .method_call(MANAGER, "Suspend", (false,))
            .map_err(io::Error::other)
    }
}
