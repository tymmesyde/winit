//! # Orbital / Redox OS
//!
//! Redox OS has some functionality not yet present that will be implemented
//! when its orbital display server provides it.

use std::{fmt, str};

pub use self::event_loop::{EventLoop, PlatformSpecificEventLoopAttributes};

macro_rules! os_error {
    ($error:expr) => {{
        winit_core::error::OsError::new(line!(), file!(), $error)
    }};
}

pub mod event_loop;
pub mod window;

#[derive(Debug)]
struct RedoxSocket {
    fd: usize,
}

impl RedoxSocket {
    fn event() -> syscall::Result<Self> {
        Self::open_raw("/scheme/event")
    }

    fn orbital(properties: &WindowProperties<'_>) -> syscall::Result<Self> {
        Self::open_raw(&format!("{properties}"))
    }

    // Paths should be checked to ensure they are actually sockets and not normal files. If a
    // non-socket path is used, it could cause read and write to not function as expected. For
    // example, the seek would change in a potentially unpredictable way if either read or write
    // were called at the same time by multiple threads.
    fn open_raw(path: &str) -> syscall::Result<Self> {
        let fd = syscall::open(path, syscall::O_RDWR | syscall::O_CLOEXEC)?;
        Ok(Self { fd })
    }

    fn read(&self, buf: &mut [u8]) -> syscall::Result<()> {
        let count = syscall::read(self.fd, buf)?;
        if count == buf.len() {
            Ok(())
        } else {
            Err(syscall::Error::new(syscall::EINVAL))
        }
    }

    fn write(&self, buf: &[u8]) -> syscall::Result<()> {
        let count = syscall::write(self.fd, buf)?;
        if count == buf.len() {
            Ok(())
        } else {
            Err(syscall::Error::new(syscall::EINVAL))
        }
    }

    fn fpath<'a>(&self, buf: &'a mut [u8]) -> syscall::Result<&'a str> {
        let count = syscall::fpath(self.fd, buf)?;
        str::from_utf8(&buf[..count]).map_err(|_err| syscall::Error::new(syscall::EINVAL))
    }
}

impl Drop for RedoxSocket {
    fn drop(&mut self) {
        let _ = syscall::close(self.fd);
    }
}

#[derive(Debug)]
struct TimeSocket(RedoxSocket);

impl TimeSocket {
    fn open() -> syscall::Result<Self> {
        RedoxSocket::open_raw("/scheme/time/4").map(Self)
    }

    // Read current time.
    fn current_time(&self) -> syscall::Result<syscall::TimeSpec> {
        let mut timespec = syscall::TimeSpec::default();
        self.0.read(&mut timespec)?;
        Ok(timespec)
    }

    // Write a timeout.
    fn timeout(&self, timespec: &syscall::TimeSpec) -> syscall::Result<()> {
        self.0.write(timespec)
    }

    // Wake immediately.
    fn wake(&self) -> syscall::Result<()> {
        // Writing a default TimeSpec will always trigger a time event.
        self.timeout(&syscall::TimeSpec::default())
    }
}

struct WindowProperties<'a> {
    flags: &'a str,
    x: i32,
    y: i32,
    w: u32,
    h: u32,
    title: &'a str,
}

impl<'a> WindowProperties<'a> {
    fn new(path: &'a str) -> Self {
        // orbital:flags/x/y/w/h/t
        let mut parts = path.splitn(6, '/');
        let flags = parts.next().unwrap_or("");
        let x = parts.next().map_or(0, |part| part.parse::<i32>().unwrap_or(0));
        let y = parts.next().map_or(0, |part| part.parse::<i32>().unwrap_or(0));
        let w = parts.next().map_or(0, |part| part.parse::<u32>().unwrap_or(0));
        let h = parts.next().map_or(0, |part| part.parse::<u32>().unwrap_or(0));
        let title = parts.next().unwrap_or("");
        Self { flags, x, y, w, h, title }
    }
}

impl fmt::Display for WindowProperties<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "/scheme/orbital/{}/{}/{}/{}/{}/{}",
            self.flags, self.x, self.y, self.w, self.h, self.title
        )
    }
}
