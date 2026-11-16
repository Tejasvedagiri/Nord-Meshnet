//! Typed wrapper over the official `nordvpn` CLI: argv only (never a shell),
//! stdin closed (so first-run prompts can't hang us), hard timeouts.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::status::{self, Settings, Status};

pub const NORDVPN_BIN: &str = "/usr/bin/nordvpn";

#[derive(Clone, Debug)]
pub struct Output {
    pub success: bool,
    /// stdout + stderr, ANSI and spinner frames removed.
    pub text: String,
}

impl Output {
    /// Success, or a failure that only says the state already holds.
    pub fn ok_or_already(&self) -> bool {
        self.success || status::already_done(&self.text)
    }

    /// First non-empty line, for log messages.
    pub fn summary(&self) -> &str {
        self.text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("(no output)")
    }
}

#[derive(Debug)]
pub enum CliError {
    Spawn(std::io::Error),
    Timeout(Duration),
}

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CliError::Spawn(e) => write!(f, "cannot run nordvpn: {e}"),
            CliError::Timeout(d) => write!(f, "nordvpn timed out after {}s", d.as_secs()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Nordvpn {
    pub bin: String,
    pub timeout: Duration,
}

impl Default for Nordvpn {
    fn default() -> Self {
        Nordvpn {
            bin: NORDVPN_BIN.into(),
            timeout: Duration::from_secs(30),
        }
    }
}

impl Nordvpn {
    pub fn with_timeout(&self, timeout: Duration) -> Nordvpn {
        Nordvpn {
            bin: self.bin.clone(),
            timeout,
        }
    }

    pub fn run<S: AsRef<str>>(&self, args: &[S]) -> Result<Output, CliError> {
        let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
        crate::debug!("$ nordvpn {}", redact(&args).join(" "));
        let mut child = Command::new(&self.bin)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(CliError::Spawn)?;

        // Drain both pipes on threads so a chatty command can't fill a pipe
        // buffer and block forever.
        let mut out = child.stdout.take().expect("piped stdout");
        let mut err = child.stderr.take().expect("piped stderr");
        let t_out = thread::spawn(move || {
            let mut b = Vec::new();
            let _ = out.read_to_end(&mut b);
            b
        });
        let t_err = thread::spawn(move || {
            let mut b = Vec::new();
            let _ = err.read_to_end(&mut b);
            b
        });

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(50)),
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let mut raw = t_out.join().unwrap_or_default();
        raw.extend(t_err.join().unwrap_or_default());
        let Some(status) = status else {
            return Err(CliError::Timeout(self.timeout));
        };
        let text = status::clean(&String::from_utf8_lossy(&raw));
        crate::debug!(
            "  -> exit {:?}: {}",
            status.code(),
            text.replace('\n', " | ")
        );
        Ok(Output {
            success: status.success(),
            text,
        })
    }

    pub fn status(&self) -> Result<Status, CliError> {
        Ok(status::parse_status(&self.run(&["status"])?.text))
    }

    pub fn settings(&self) -> Result<Settings, CliError> {
        Ok(status::parse_settings(&self.run(&["settings"])?.text))
    }

    pub fn logged_in(&self) -> Result<bool, CliError> {
        let out = self.run(&["account"])?;
        Ok(status::logged_in(out.success, &out.text))
    }

    pub fn peer_list(&self) -> Result<Output, CliError> {
        self.run(&["meshnet", "peer", "list"])
    }
}

/// Hide the value after `--token` in debug logs.
fn redact<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut out = Vec::with_capacity(args.len());
    let mut hide = false;
    for a in args {
        out.push(if hide { "<redacted>" } else { a });
        hide = *a == "--token";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_token() {
        assert_eq!(
            redact(&["login", "--token", "abc123"]),
            vec!["login", "--token", "<redacted>"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn times_out() {
        let cli = Nordvpn {
            bin: "/bin/sleep".into(),
            timeout: Duration::from_millis(200),
        };
        assert!(matches!(cli.run(&["5"]), Err(CliError::Timeout(_))));
    }

    #[cfg(unix)]
    #[test]
    fn captures_output_and_exit() {
        let cli = Nordvpn {
            bin: "/bin/sh".into(),
            timeout: Duration::from_secs(5),
        };
        let out = cli
            .run(&["-c", "echo 'Status: Connected'; echo oops >&2; exit 3"])
            .unwrap();
        assert!(!out.success);
        assert!(out.text.contains("Status: Connected"));
        assert!(out.text.contains("oops"));
    }
}
