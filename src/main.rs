//! argv[0] / argv[1] dispatch:
//!   sysctl                     → the sysctl shim (via the /usr/sbin/sysctl symlink)
//!   nordmesh-init healthcheck  → Docker HEALTHCHECK
//!   nordmesh-init              → PID 1

use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use nordmesh::cli::Nordvpn;
use nordmesh::config::Config;
use nordmesh::daemon::{Stop, Supervisor};
use nordmesh::reconcile::{Fatal, Reconciler};
use nordmesh::{error, health, info, log, sysctl};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let argv0 = args
        .first()
        .map(|a| {
            Path::new(a)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        })
        .unwrap_or_default();

    let code = if argv0 == "sysctl" {
        sysctl::shim(&args[1..], sysctl::REAL_SYSCTL, Path::new("/proc/sys"))
    } else {
        match args.get(1).map(String::as_str) {
            Some("healthcheck") => health::check(),
            Some("check-config") => check_config(),
            Some("--version" | "version") => {
                println!("nordmesh-init {}", env!("CARGO_PKG_VERSION"));
                0
            }
            None | Some("init") => init(),
            Some(other) => {
                eprintln!(
                    "usage: nordmesh-init [init|healthcheck|check-config|version] (got {other:?})"
                );
                2
            }
        }
    };
    ExitCode::from(code.clamp(0, 255) as u8)
}

/// Validate the environment without starting anything.
fn check_config() -> i32 {
    match Config::from_env() {
        Ok(cfg) => {
            println!("{cfg}");
            0
        }
        Err(e) => {
            eprint!("{e}");
            2
        }
    }
}

fn init() -> i32 {
    // 1. Parse and validate.
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprint!("[nordmesh] {e}");
            return 2;
        }
    };
    log::set_level(cfg.log_level);
    info!("nordmesh-init {}", env!("CARGO_PKG_VERSION"));
    for line in cfg.to_string().lines() {
        info!("  {line}");
    }

    // 2. Start nordvpnd and wait for it.
    let mut sup = match Supervisor::start(cfg.log_level) {
        Ok(s) => s,
        Err(e) => {
            error!("cannot start nordvpnd: {e}");
            return 1;
        }
    };
    let cli = Nordvpn::default();
    match sup.wait_ready(&cli, Duration::from_secs(30)) {
        Ok(true) => info!("nordvpnd is ready"),
        Ok(false) => {
            error!("nordvpnd did not become ready within 30s");
            return finish(&mut sup, Stop::Signal(nordmesh::sys::SIGTERM), 1);
        }
        Err(stop) => return finish(&mut sup, stop, 0),
    }

    let mut rec = Reconciler::new(&cfg, cli, &mut sup);
    // 3–6. Fatal on failure.
    if let Err(f) = rec.bootstrap() {
        drop(rec);
        return match f {
            Fatal::Stop(stop) => finish(&mut sup, stop, 0),
            Fatal::Msg(msg) => {
                error!("{msg}");
                finish(&mut sup, Stop::Signal(nordmesh::sys::SIGTERM), 1)
            }
        };
    }
    // 7–9, then 10.
    let stop = match rec.converge() {
        Ok(()) => {
            info!(
                "startup complete; reconciling every {}s",
                cfg.reconcile_interval.as_secs()
            );
            rec.run_forever()
        }
        Err(stop) => stop,
    };
    drop(rec);
    finish(&mut sup, stop, 0)
}

/// Tear down and pick the exit code. `requested` is used for a clean stop.
fn finish(sup: &mut Supervisor, stop: Stop, requested: i32) -> i32 {
    match stop {
        Stop::Signal(sig) => {
            if requested == 0 {
                info!("received signal {sig}, shutting down");
            }
            sup.shutdown(sig);
            requested
        }
        Stop::DaemonExited(code) => {
            error!("nordvpnd exited unexpectedly (code {code})");
            sup.shutdown(nordmesh::sys::SIGTERM);
            if code == 0 {
                1
            } else {
                code
            }
        }
    }
}
