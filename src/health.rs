//! `nordmesh-init healthcheck`: healthy when what the env asks for is up.

use std::time::Duration;

use crate::cli::Nordvpn;
use crate::config::Config;

pub fn check() -> i32 {
    let cfg = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprint!("unhealthy: {e}");
            return 1;
        }
    };
    let cli = Nordvpn::default().with_timeout(Duration::from_secs(5));

    if cfg.connect.is_some() {
        match cli.status() {
            Ok(s) if s.connected => {}
            Ok(_) => {
                eprintln!("unhealthy: VPN not connected");
                return 1;
            }
            Err(e) => {
                eprintln!("unhealthy: {e}");
                return 1;
            }
        }
    }
    if cfg.meshnet {
        match cli.settings() {
            Ok(s) if s.meshnet == Some(true) => {}
            Ok(_) => {
                eprintln!("unhealthy: Meshnet not enabled");
                return 1;
            }
            Err(e) => {
                eprintln!("unhealthy: {e}");
                return 1;
            }
        }
    }
    if cfg.connect.is_none() && !cfg.meshnet {
        // Idle mode: just require the daemon to answer.
        if !cli.run(&["status"]).is_ok_and(|o| o.success) {
            eprintln!("unhealthy: nordvpnd not answering");
            return 1;
        }
    }
    println!("healthy");
    0
}
