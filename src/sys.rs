//! The handful of libc calls std doesn't expose: signal handlers, reaping
//! arbitrary children, and sending signals. Declared directly so the crate
//! stays dependency-free; every Unix libc (glibc, musl, macOS) exports them
//! and the constants used here are identical across them.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicI32, Ordering};

pub const SIGINT: c_int = 2;
pub const SIGKILL: c_int = 9;
pub const SIGTERM: c_int = 15;
const WNOHANG: c_int = 1;
const ECHILD: i32 = 10;

extern "C" {
    fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
    fn waitpid(pid: c_int, status: *mut c_int, options: c_int) -> c_int;
    fn kill(pid: c_int, sig: c_int) -> c_int;
}

static PENDING: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(sig: c_int) {
    PENDING.store(sig, Ordering::SeqCst);
}

/// Record SIGTERM/SIGINT in a flag instead of dying; the main loop polls it.
pub fn install_signal_handlers() {
    // SAFETY: the handler only performs an atomic store, which is async-signal-safe.
    unsafe {
        signal(SIGTERM, on_signal);
        signal(SIGINT, on_signal);
    }
}

/// The termination signal received so far, if any.
pub fn pending_signal() -> Option<c_int> {
    match PENDING.load(Ordering::SeqCst) {
        0 => None,
        s => Some(s),
    }
}

pub enum Reaped {
    /// A child exited; `code` follows shell conventions (128+n when signalled).
    Exited { pid: i32, code: i32 },
    /// Children exist but none has exited.
    Nothing,
    /// No children at all.
    NoChildren,
}

/// Reap one exited child without blocking.
///
/// Only call this when no `std::process::Child` is being waited on, or it
/// could steal that child's exit status. The init loop is single-threaded and
/// never calls this while a CLI command is running.
pub fn reap_one() -> Reaped {
    let mut status: c_int = 0;
    // SAFETY: plain syscall wrapper with a valid out-pointer.
    let pid = unsafe { waitpid(-1, &mut status, WNOHANG) };
    if pid > 0 {
        let code = if status & 0x7f == 0 {
            (status >> 8) & 0xff
        } else {
            128 + (status & 0x7f)
        };
        Reaped::Exited { pid, code }
    } else if pid == 0 {
        Reaped::Nothing
    } else if std::io::Error::last_os_error().raw_os_error() == Some(ECHILD) {
        Reaped::NoChildren
    } else {
        Reaped::Nothing
    }
}

pub fn send_signal(pid: u32, sig: c_int) -> bool {
    // SAFETY: plain syscall wrapper.
    unsafe { kill(pid as c_int, sig) == 0 }
}
