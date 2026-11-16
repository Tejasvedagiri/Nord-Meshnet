//! nordmesh-init: PID 1 for the nordmesh image.
//!
//! One binary, three personalities chosen by `argv[0]` / `argv[1]`
//! (see `main.rs`): the init process, the Docker healthcheck, and the
//! `sysctl` shim that nordvpnd shells out to.

pub mod cli;
pub mod config;
pub mod daemon;
pub mod health;
pub mod log;
pub mod peers;
pub mod reconcile;
pub mod status;
pub mod sys;
pub mod sysctl;
