//! `/usr/sbin/sysctl` shim.
//!
//! nordvpnd shells out to `sysctl -w key=value` on connect and aborts when the
//! write fails. `/proc/sys` is read-only in unprivileged containers, even for
//! writes that change nothing, so a value preset with `--sysctl` would still
//! fail. Succeed when every requested value already holds; otherwise behave
//! exactly like the real tool. (Approach from drumandbytes/nordvpn, MIT.)

use std::io::Write;
use std::path::Path;
use std::process::Command;

pub const REAL_SYSCTL: &str = "/usr/sbin/sysctl.real";

pub fn shim(args: &[String], real: &str, proc_root: &Path) -> i32 {
    let out = match Command::new(real).args(args).output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("sysctl: cannot run {real}: {e}");
            return 127;
        }
    };
    if !out.status.success() && already_applied(args, proc_root) {
        return 0;
    }
    let _ = std::io::stdout().write_all(&out.stdout);
    let _ = std::io::stderr().write_all(&out.stderr);
    out.status.code().unwrap_or(1)
}

/// True when every `key=value` argument already holds under `proc_root`.
/// False when there is nothing to compare, so a failed read is never masked.
pub fn already_applied(args: &[String], proc_root: &Path) -> bool {
    let mut found = false;
    for a in args {
        if a.starts_with('-') {
            continue;
        }
        let Some((key, want)) = a.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() || key.contains("..") {
            return false;
        }
        found = true;
        // Note: dots inside interface names (eth0.100) would mis-map to '/';
        // none of nordvpnd's keys contain them.
        let path = proc_root.join(key.replace('.', "/"));
        match std::fs::read_to_string(path) {
            Ok(got) if normalize(&got) == normalize(want) => {}
            _ => return false,
        }
    }
    found
}

fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
