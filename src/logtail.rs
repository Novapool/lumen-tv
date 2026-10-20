// Keeps the last lines Lumen itself printed, for the Settings Log panel.
// Linux: stdout and stderr are swapped for pipes; a thread per pipe forwards
// every byte to the original fd (so the journal is unchanged) and feeds a
// ring buffer. We don't read journalctl: lumen-test's journal isn't readable
// without extra groups. Elsewhere `install` does nothing and the tail is empty.

use std::collections::VecDeque;
use std::sync::Mutex;
#[cfg(target_os = "linux")]
use std::time::Duration;

/// Lines kept.
pub const KEEP: usize = 200;

/// Finished lines, oldest first, bounded.
pub struct Ring {
    lines: VecDeque<String>,
    cap: usize,
}

impl Ring {
    pub const fn new(cap: usize) -> Ring {
        Ring {
            lines: VecDeque::new(),
            cap,
        }
    }

    pub fn push(&mut self, line: String) {
        if self.lines.len() == self.cap {
            self.lines.pop_front();
        }
        self.lines.push_back(line);
    }

    /// Up to `n` newest lines, oldest first.
    pub fn tail(&self, n: usize) -> Vec<String> {
        self.lines
            .iter()
            .skip(self.lines.len().saturating_sub(n))
            .cloned()
            .collect()
    }
}

/// Turns a byte stream into lines (a partial line waits for its newline).
#[derive(Default)]
pub struct LineSplitter {
    partial: Vec<u8>,
}

impl LineSplitter {
    pub fn feed(&mut self, bytes: &[u8], mut line: impl FnMut(String)) {
        for &b in bytes {
            if b == b'\n' {
                let text = String::from_utf8_lossy(&self.partial)
                    .trim_end_matches('\r')
                    .to_string();
                self.partial.clear();
                line(text);
            } else if self.partial.len() < 4096 {
                self.partial.push(b);
            }
        }
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new(KEEP));

/// The newest `n` captured lines, oldest first.
pub fn tail(n: usize) -> Vec<String> {
    RING.lock().map(|r| r.tail(n)).unwrap_or_default()
}

/// Call first thing in `main`.
#[cfg(target_os = "linux")]
pub fn install() {
    for fd in [libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        if let Err(e) = capture(fd) {
            // Can't use eprintln for a stderr failure reliably; best effort.
            eprintln!("[logtail] could not capture fd {fd}: {e}");
        }
    }
    // A panic message must reach the journal even if the process dies before
    // the forwarder thread runs, so the hook writes to the original stderr
    // itself (and not through the pipe, which would print it twice).
    std::panic::set_hook(Box::new(|info| {
        let name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        let text = format!("thread '{name}' {info}\n");
        let orig = ORIG[1].load(std::sync::atomic::Ordering::Relaxed);
        if orig >= 0 {
            write_all_fd(orig, text.as_bytes());
        } else {
            eprint!("{text}");
        }
        // try_lock: the panic may have happened while the ring was locked.
        if let Ok(mut r) = RING.try_lock() {
            text.lines().for_each(|l| r.push(l.to_string()));
        }
    }));
}

#[cfg(not(target_os = "linux"))]
pub fn install() {}

/// Call before exiting normally: points stdout/stderr back at the journal and
/// gives the forwarders (up to half a second) to drain what is still in the pipes.
#[cfg(target_os = "linux")]
pub fn shutdown() {
    use std::sync::atomic::Ordering;
    for (i, fd) in [libc::STDOUT_FILENO, libc::STDERR_FILENO]
        .into_iter()
        .enumerate()
    {
        let orig = ORIG[i].load(Ordering::Relaxed);
        if orig >= 0 {
            // SAFETY: both are open fds; this closes the pipe's last write end, so the reader sees EOF.
            unsafe { libc::dup2(orig, fd) };
        }
    }
    let deadline = std::time::Instant::now() + Duration::from_millis(500);
    let handles = std::mem::take(&mut *FORWARDERS.lock().unwrap_or_else(|e| e.into_inner()));
    for h in handles {
        while !h.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if h.is_finished() {
            let _ = h.join();
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn shutdown() {}

/// Original stdout/stderr (the journal) saved before the capture, or -1.
#[cfg(target_os = "linux")]
static ORIG: [std::sync::atomic::AtomicI32; 2] = [
    std::sync::atomic::AtomicI32::new(-1),
    std::sync::atomic::AtomicI32::new(-1),
];

#[cfg(target_os = "linux")]
static FORWARDERS: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

/// stdout/stderr for a child app: the journal as it was before the capture,
/// so a chatty or killed app never touches the log ring or its pipe.
/// Falls back to inheriting (the capture pipe) if the originals are unknown.
pub fn child_stdio() -> (std::process::Stdio, std::process::Stdio) {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::{BorrowedFd, OwnedFd};
        use std::sync::atomic::Ordering;
        let one = |i: usize| {
            let fd = ORIG[i].load(Ordering::Relaxed);
            if fd < 0 {
                return std::process::Stdio::inherit();
            }
            // SAFETY: `fd` stays open for the life of the process (the capture thread owns it).
            match unsafe { BorrowedFd::borrow_raw(fd) }.try_clone_to_owned() {
                Ok(o) => std::process::Stdio::from(OwnedFd::from(o)),
                Err(_) => std::process::Stdio::inherit(),
            }
        };
        (one(0), one(1))
    }
    #[cfg(not(target_os = "linux"))]
    {
        (
            std::process::Stdio::inherit(),
            std::process::Stdio::inherit(),
        )
    }
}

/// Writes all of `bytes` to `fd`. Retries EINTR, and waits (up to ~5 s in all)
/// on EAGAIN, which a child that set O_NONBLOCK on the shared journal
/// description can cause. Any other error drops the rest.
#[cfg(target_os = "linux")]
fn write_all_fd(fd: i32, mut bytes: &[u8]) {
    use std::io::{Error, ErrorKind};
    let mut waits = 0;
    while !bytes.is_empty() {
        // SAFETY: writes from our own buffer.
        let w = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if w > 0 {
            bytes = &bytes[w as usize..];
            continue;
        }
        match Error::last_os_error().kind() {
            ErrorKind::Interrupted => {}
            ErrorKind::WouldBlock if waits < 50 => {
                waits += 1;
                let mut p = libc::pollfd {
                    fd,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                // SAFETY: one valid pollfd.
                unsafe { libc::poll(&mut p, 1, 100) };
            }
            _ => return, // the journal went away; keep capturing
        }
    }
}

#[cfg(target_os = "linux")]
fn capture(fd: i32) -> std::io::Result<()> {
    use std::io::Error;
    // SAFETY: plain fd plumbing. `pipe2` fills two fresh close-on-exec fds;
    // F_DUPFD_CLOEXEC keeps the original target (children get theirs from
    // `child_stdio`); `dup2` points `fd` at the pipe's write end. The reader
    // thread owns `rd` and `orig` for the life of the process.
    let (rd, orig) = unsafe {
        let mut p = [0i32; 2];
        if libc::pipe2(p.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return Err(Error::last_os_error());
        }
        let orig = libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3);
        if orig < 0 || libc::dup2(p[1], fd) < 0 {
            let e = Error::last_os_error();
            libc::close(p[0]);
            libc::close(p[1]);
            if orig >= 0 {
                libc::close(orig);
            }
            return Err(e);
        }
        libc::close(p[1]);
        (p[0], orig)
    };
    ORIG[if fd == libc::STDOUT_FILENO { 0 } else { 1 }]
        .store(orig, std::sync::atomic::Ordering::Relaxed);
    let handle = std::thread::spawn(move || {
        let mut split = LineSplitter::default();
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: reads into our own buffer.
            let n = unsafe { libc::read(rd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 && Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                break;
            }
            let chunk = &buf[..n as usize];
            write_all_fd(orig, chunk);
            split.feed(chunk, |l| {
                if let Ok(mut r) = RING.lock() {
                    r.push(l);
                }
            });
        }
        // SAFETY: the pipe's read end is ours and no longer used.
        unsafe { libc::close(rd) };
    });
    FORWARDERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(handle);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_newest_lines() {
        let mut r = Ring::new(3);
        for i in 1..=5 {
            r.push(format!("l{i}"));
        }
        assert_eq!(r.tail(10), ["l3", "l4", "l5"]);
        assert_eq!(r.tail(2), ["l4", "l5"]);
        assert!(Ring::new(3).tail(5).is_empty());
    }

    #[test]
    fn splitter_joins_partial_lines_across_chunks() {
        let mut s = LineSplitter::default();
        let mut out = Vec::new();
        s.feed(b"one\ntw", |l| out.push(l));
        s.feed(b"o\r\n\nthree", |l| out.push(l));
        assert_eq!(out, ["one", "two", ""]);
        s.feed(b"\n", |l| out.push(l));
        assert_eq!(out.last().unwrap(), "three");
    }
}
