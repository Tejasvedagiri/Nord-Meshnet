//! nordvpnd under PID 1: start it, wait for its socket, forward termination
//! signals, and reap it plus anything it orphans (openvpn, norduserd, ...).

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::cli::Nordvpn;
use crate::log::Level;
use crate::sys::{self, Reaped};

pub const NORDVPND_BIN: &str = "/usr/sbin/nordvpnd";
pub const SOCKET: &str = "/run/nordvpn/nordvpnd.sock";
const LOGLEVEL_FILE: &str = "/run/nordvpn/loglevel";
const TICK: Duration = Duration::from_millis(250);

/// Why the supervisor wants the init process to stop.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    /// SIGTERM/SIGINT received.
    Signal(i32),
    /// nordvpnd exited on its own with this code.
    DaemonExited(i32),
}

pub struct Supervisor {
    pid: u32,
    exited: Option<i32>,
}

impl Supervisor {
    pub fn start(level: Level) -> std::io::Result<Supervisor> {
        sys::install_signal_handlers();
        // nordvpnd defaults to "debug" (full HTTP header dumps) when this
        // file is missing, which it always is in a fresh container.
        std::fs::create_dir_all("/run/nordvpn")?;
        if let Err(e) = std::fs::write(LOGLEVEL_FILE, level.as_str()) {
            crate::warn!("cannot set nordvpnd log level: {e}");
        }
        let child = Command::new(NORDVPND_BIN).stdin(Stdio::null()).spawn()?;
        crate::info!("started nordvpnd (pid {})", child.id());
        // Not waited on through `Child`: reaping happens in `poll`.
        Ok(Supervisor {
            pid: child.id(),
            exited: None,
        })
    }

    /// Reap exited children; report whether we should stop.
    pub fn poll(&mut self) -> Result<(), Stop> {
        if let Some(sig) = sys::pending_signal() {
            return Err(Stop::Signal(sig));
        }
        loop {
            match sys::reap_one() {
                Reaped::Exited { pid, code } if pid as u32 == self.pid => {
                    self.exited = Some(code);
                }
                Reaped::Exited { pid, code } => {
                    crate::debug!("reaped pid {pid} (exit {code})");
                }
                Reaped::Nothing | Reaped::NoChildren => break,
            }
        }
        match self.exited {
            Some(code) => Err(Stop::DaemonExited(code)),
            None => Ok(()),
        }
    }

    /// Sleep while staying responsive to signals and daemon exit.
    pub fn sleep(&mut self, d: Duration) -> Result<(), Stop> {
        let end = Instant::now() + d;
        loop {
            self.poll()?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            thread::sleep(TICK.min(end - now));
        }
    }

    /// Wait until the socket exists and the CLI gets an answer.
    pub fn wait_ready(&mut self, cli: &Nordvpn, timeout: Duration) -> Result<bool, Stop> {
        let end = Instant::now() + timeout;
        let quick = cli.with_timeout(Duration::from_secs(5));
        while Instant::now() < end {
            self.poll()?;
            if Path::new(SOCKET).exists() && quick.run(&["status"]).is_ok_and(|o| o.success) {
                return Ok(true);
            }
            self.sleep(Duration::from_secs(1))?;
        }
        Ok(false)
    }

    /// Forward a termination signal, give nordvpnd 10s, then SIGKILL.
    /// Returns nordvpnd's exit code if it was observed.
    pub fn shutdown(&mut self, sig: i32) -> Option<i32> {
        if self.exited.is_none() {
            crate::info!("stopping nordvpnd");
            sys::send_signal(self.pid, sig);
            let end = Instant::now() + Duration::from_secs(10);
            while self.exited.is_none() && Instant::now() < end {
                self.reap_quietly();
                thread::sleep(TICK);
            }
            if self.exited.is_none() {
                crate::warn!("nordvpnd did not stop in 10s, killing it");
                sys::send_signal(self.pid, sys::SIGKILL);
                thread::sleep(TICK);
                self.reap_quietly();
            }
        }
        // Collect any remaining zombies before exiting.
        self.reap_quietly();
        self.exited
    }

    fn reap_quietly(&mut self) {
        while let Reaped::Exited { pid, code } = sys::reap_one() {
            if pid as u32 == self.pid {
                self.exited = Some(code);
            }
        }
    }
}
