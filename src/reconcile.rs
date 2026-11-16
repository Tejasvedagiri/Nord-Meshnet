//! Startup sequence (plan §6) and the reconcile loop (§7).
//!
//! The ordering rule that matters most: the kill switch is forced off at boot
//! and only enabled after a confirmed `Status: Connected`. A kill switch
//! applied before a working tunnel blocks login and Meshnet registration too.

use std::collections::HashSet;
use std::time::Duration;

use crate::cli::{Nordvpn, Output};
use crate::config::{Config, Port};
use crate::daemon::{Stop, Supervisor};
use crate::peers::{self, PeerList};
use crate::{debug, info, warn};

/// Values nordvpnd writes during connect (it can't in an unprivileged container).
const REQUIRED_SYSCTLS: &[(&str, &str)] = &[("net.ipv4.conf.all.rp_filter", "2")];

const STALE_OVPN_CONFIG: &str = "/var/lib/nordvpn/data/.config.ovpn";

/// A fatal startup failure: the container exits non-zero so `restart:` retries.
#[derive(Debug)]
pub enum Fatal {
    Stop(Stop),
    Msg(String),
}

impl From<Stop> for Fatal {
    fn from(s: Stop) -> Self {
        Fatal::Stop(s)
    }
}

pub struct Reconciler<'a> {
    cfg: &'a Config,
    cli: Nordvpn,
    sup: &'a mut Supervisor,
    /// Last observed VPN state, to log transitions only.
    vpn_up: Option<bool>,
    killswitch_applied: bool,
    meshnet_ok: Option<bool>,
    /// Names already warned about, so each warns once until it resolves.
    warned: HashSet<String>,
}

impl<'a> Reconciler<'a> {
    pub fn new(cfg: &'a Config, cli: Nordvpn, sup: &'a mut Supervisor) -> Self {
        Reconciler {
            cfg,
            cli,
            sup,
            vpn_up: None,
            killswitch_applied: false,
            meshnet_ok: None,
            warned: HashSet::new(),
        }
    }

    // ---------------------------------------------------------------- startup

    /// Steps 3–6: fatal on failure.
    pub fn bootstrap(&mut self) -> Result<(), Fatal> {
        let cfg = self.cfg;
        // 3. First-run CLI commands block on an analytics y/n prompt.
        let analytics = if cfg.analytics { "on" } else { "off" };
        self.must(&["set", "analytics", analytics], "setting analytics")?;

        // 4. Login.
        if !self.logged_in()? {
            let Some(token) = cfg.token.clone() else {
                if cfg.connect.is_some() || cfg.meshnet {
                    return Err(Fatal::Msg(
                        "not logged in and no NORDVPN_TOKEN / NORDVPN_TOKEN_FILE given".into(),
                    ));
                }
                warn!("not logged in and no token given; idling (log in with `docker exec`)");
                return Ok(());
            };
            self.login(&token)?;
        } else {
            info!("already logged in");
        }

        // nordvpnd sets these on connect and fails if it can't; /proc/sys is
        // read-only here, so they must come from `sysctls:` in compose.
        if cfg.connect.is_some() {
            for (key, want) in REQUIRED_SYSCTLS {
                let path = format!("/proc/sys/{}", key.replace('.', "/"));
                match std::fs::read_to_string(&path) {
                    Ok(v) if v.trim() == *want => {}
                    Ok(v) => warn!(
                        "{key} is {} but nordvpnd needs {want}; connects will fail. Add `- {key}={want}` under sysctls:",
                        v.trim()
                    ),
                    Err(e) => debug!("reading {path}: {e}"),
                }
            }
        }

        // 5. A persisted volume may hold `killswitch on` from a previous run.
        self.must(&["set", "killswitch", "off"], "disabling kill switch")?;
        // An interrupted OpenVPN connect leaves this behind; nordvpnd then
        // fails every later OpenVPN connect trying to `chattr -i` it.
        match std::fs::remove_file(STALE_OVPN_CONFIG) {
            Ok(()) => info!("removed stale {STALE_OVPN_CONFIG}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => warn!("removing stale {STALE_OVPN_CONFIG}: {e}"),
        }

        // 6. Settings. Env is the source of truth over the persisted volume.
        self.must(
            &["set", "protocol", cfg.protocol.as_str()],
            "setting protocol",
        )?;
        if cfg.dns.is_empty() {
            self.must(&["set", "dns", "off"], "resetting DNS")?;
        } else {
            let mut args = vec!["set", "dns"];
            args.extend(cfg.dns.iter().map(String::as_str));
            self.must(&args, "setting DNS")?;
        }
        self.must(&["allowlist", "remove", "all"], "clearing allowlist")?;
        for subnet in &cfg.allowlist_subnets {
            self.must(
                &["allowlist", "add", "subnet", subnet.as_str()],
                "allowlisting subnet",
            )?;
        }
        for port in &cfg.allowlist_ports {
            match port {
                Port::Single(p) => {
                    let p = p.to_string();
                    self.must(
                        &["allowlist", "add", "port", p.as_str()],
                        "allowlisting port",
                    )?
                }
                Port::Range(a, b) => {
                    let (a, b) = (a.to_string(), b.to_string());
                    self.must(
                        &["allowlist", "add", "ports", a.as_str(), b.as_str()],
                        "allowlisting ports",
                    )?
                }
            }
        }
        for args in &cfg.set {
            let mut full = vec!["set"];
            full.extend(args.iter().map(String::as_str));
            self.must(&full, &format!("set {}", args.join(" ")))?;
        }
        Ok(())
    }

    /// Steps 7–9: retried, then handed to the loop if still failing.
    pub fn converge(&mut self) -> Result<(), Stop> {
        let cfg = self.cfg;
        if cfg.connect.is_some() {
            let mut delay = Duration::from_secs(5);
            for attempt in 1..=5 {
                if self.ensure_vpn()? {
                    break;
                }
                if attempt == 5 {
                    warn!("still not connected; the reconcile loop will keep trying");
                    break;
                }
                self.sup.sleep(delay)?;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
        if cfg.meshnet {
            // Peers and our own registration can take a few seconds to
            // appear after `set meshnet on` (lesson 7).
            for attempt in 1..=12 {
                // Don't let a slow Meshnet hold off a reconnect.
                if cfg.connect.is_some() {
                    self.ensure_vpn()?;
                }
                if self.ensure_meshnet()? {
                    break;
                }
                if attempt == 12 {
                    warn!("Meshnet not fully applied yet; the reconcile loop will keep trying");
                    break;
                }
                self.sup.sleep(Duration::from_secs(5))?;
            }
        }
        Ok(())
    }

    /// Step 10.
    pub fn run_forever(&mut self) -> Stop {
        let cfg = self.cfg;
        loop {
            if let Err(stop) = self.sup.sleep(cfg.reconcile_interval) {
                return stop;
            }
            if let Err(stop) = self.pass() {
                return stop;
            }
        }
    }

    pub fn pass(&mut self) -> Result<(), Stop> {
        let cfg = self.cfg;
        if cfg.connect.is_some() {
            self.ensure_vpn()?;
        }
        if cfg.meshnet {
            self.ensure_meshnet()?;
        }
        Ok(())
    }

    // -------------------------------------------------------------------- VPN

    /// Connect if needed; enable the kill switch only once connected.
    /// Returns whether the tunnel is up.
    fn ensure_vpn(&mut self) -> Result<bool, Stop> {
        let cfg = self.cfg;
        let mut up = self.connected()?;
        if !up {
            if self.vpn_up != Some(false) {
                warn!("VPN not connected; connecting");
            }
            let mut args = vec!["connect"];
            if let Some(target) = &cfg.connect {
                args.extend(target.iter().map(String::as_str));
            }
            // Server selection + handshake can be slow.
            let out = self.call_with(&args, Duration::from_secs(90))?;
            up = self.connected()?;
            if !up {
                let why = out
                    .as_ref()
                    .map(Output::summary)
                    .unwrap_or("command failed");
                warn!("connect failed: {why}");
            }
        }
        if up && self.vpn_up != Some(true) {
            let st = self.cli.status().ok().unwrap_or_default();
            info!(
                "VPN connected: {} {} {}",
                st.server.unwrap_or_default(),
                st.country.unwrap_or_default(),
                st.city.unwrap_or_default()
            );
        }
        self.vpn_up = Some(up);

        if up && cfg.killswitch && !self.killswitch_applied {
            if let Some(out) = self.call(&["set", "killswitch", "on"])? {
                if out.ok_or_already() {
                    info!("kill switch on");
                    self.killswitch_applied = true;
                } else {
                    warn!("enabling kill switch failed: {}", out.summary());
                }
            }
        }
        Ok(up)
    }

    fn connected(&mut self) -> Result<bool, Stop> {
        self.sup.poll()?;
        Ok(self.cli.status().map(|s| s.connected).unwrap_or(false))
    }

    // ---------------------------------------------------------------- Meshnet

    /// Meshnet on, nickname, peer permissions. Returns whether everything
    /// configured is in place.
    fn ensure_meshnet(&mut self) -> Result<bool, Stop> {
        let cfg = self.cfg;
        self.sup.poll()?;
        let enabled = self
            .cli
            .settings()
            .ok()
            .and_then(|s| s.meshnet)
            .unwrap_or(false);
        if !enabled {
            if self.meshnet_ok != Some(false) {
                info!("enabling Meshnet");
            }
            let ok = self
                .call(&["set", "meshnet", "on"])?
                .is_some_and(|o| o.ok_or_already() || warn_false("enabling Meshnet", &o));
            if !ok {
                self.meshnet_ok = Some(false);
                return Ok(false);
            }
        }

        let Some(out) = self.call(&["meshnet", "peer", "list"])? else {
            return Ok(false);
        };
        if !out.success {
            debug!("peer list not available yet: {}", out.summary());
            self.meshnet_ok = Some(false);
            return Ok(false);
        }
        let mut list = peers::parse_peer_list(&out.text);
        let mut done = true;

        if let Some(nick) = &cfg.nickname {
            let current = list.this_device.as_ref().and_then(|d| d.nickname.clone());
            if !current
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case(nick))
            {
                info!("setting Meshnet nickname to {nick}");
                // Succeeds silently before the device is registered, so the
                // peer list is the source of truth, checked next pass.
                if let Some(o) = self.call(&["meshnet", "set", "nickname", nick.as_str()])? {
                    if !o.ok_or_already() {
                        warn!("setting nickname failed: {}", o.summary());
                    }
                }
                match self.call(&["meshnet", "peer", "list"])? {
                    Some(o) if o.success => list = peers::parse_peer_list(&o.text),
                    _ => {}
                }
                let now = list.this_device.as_ref().and_then(|d| d.nickname.clone());
                done &= now.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(nick));
            }
        }

        if cfg.peers.is_active() {
            done &= self.apply_peers(&list)?;
        }

        if done && self.meshnet_ok != Some(true) {
            info!("Meshnet ready ({} peers visible)", list.peers.len());
        }
        self.meshnet_ok = Some(done);
        Ok(done)
    }

    fn apply_peers(&mut self, list: &PeerList) -> Result<bool, Stop> {
        let cfg = self.cfg;
        let plan = peers::plan(&cfg.peers, list);
        let mut done = true;

        for name in &plan.unresolved {
            done = false;
            if self.warned.insert(name.to_ascii_lowercase()) {
                warn!("peer {name:?} not found in the Meshnet peer list yet; will retry");
            }
        }
        for name in &plan.ambiguous {
            done = false;
            if self.warned.insert(name.to_ascii_lowercase()) {
                warn!("peer {name:?} matches more than one peer; use its hostname or IP instead");
            }
        }
        // Forget warnings for names that resolve now, so a later loss warns again.
        let configured: HashSet<String> = cfg
            .peers
            .names()
            .iter()
            .map(|n| n.to_ascii_lowercase())
            .filter(|n| {
                !plan.unresolved.iter().any(|u| u.eq_ignore_ascii_case(n))
                    && !plan.ambiguous.iter().any(|u| u.eq_ignore_ascii_case(n))
            })
            .collect();
        self.warned.retain(|n| !configured.contains(n));

        for action in &plan.actions {
            let verb = if action.allow { "allow" } else { "deny" };
            match self.call(&action.args())? {
                Some(o) if o.ok_or_already() => {
                    info!("peer {}: {} {verb}", action.name, action.perm.cli_verb());
                }
                Some(o) => {
                    done = false;
                    warn!(
                        "peer {}: {} {verb} failed: {}",
                        action.name,
                        action.perm.cli_verb(),
                        o.summary()
                    );
                }
                None => done = false,
            }
        }
        Ok(done)
    }

    // ---------------------------------------------------------------- helpers

    fn logged_in(&mut self) -> Result<bool, Fatal> {
        for _ in 0..3 {
            self.sup.poll()?;
            match self.cli.logged_in() {
                Ok(v) => return Ok(v),
                Err(e) => warn!("checking login state: {e}"),
            }
            self.sup.sleep(Duration::from_secs(2))?;
        }
        Err(Fatal::Msg("cannot determine login state".into()))
    }

    /// NordVPN's credentials API has transient timeouts: retry 5× with backoff.
    /// Login output is never logged.
    fn login(&mut self, token: &str) -> Result<(), Fatal> {
        let mut delay = Duration::from_secs(5);
        for attempt in 1..=5 {
            self.sup.poll()?;
            match self
                .cli
                .with_timeout(Duration::from_secs(60))
                .run(&["login", "--token", token])
            {
                Ok(o) if o.ok_or_already() => {
                    if self.logged_in()? {
                        info!("logged in");
                        return Ok(());
                    }
                    warn!(
                        "login reported success but account shows logged out (attempt {attempt}/5)"
                    );
                }
                Ok(_) => warn!("login failed (attempt {attempt}/5)"),
                Err(e) => warn!("login failed (attempt {attempt}/5): {e}"),
            }
            if attempt < 5 {
                self.sup.sleep(delay)?;
                delay *= 2;
            }
        }
        Err(Fatal::Msg(
            "login failed after 5 attempts (check the token)".into(),
        ))
    }

    /// A bootstrap command that must succeed (or already hold).
    fn must(&mut self, args: &[&str], what: &str) -> Result<(), Fatal> {
        self.sup.poll()?;
        match self.cli.run(args) {
            Ok(o) if o.ok_or_already() => Ok(()),
            Ok(o) => Err(Fatal::Msg(format!("{what} failed: {}", o.summary()))),
            Err(e) => Err(Fatal::Msg(format!("{what} failed: {e}"))),
        }
    }

    /// A best-effort command. `Ok(None)` when it couldn't run at all.
    fn call(&mut self, args: &[&str]) -> Result<Option<Output>, Stop> {
        let cli = self.cli.clone();
        self.call_on(&cli, args)
    }

    fn call_with(&mut self, args: &[&str], timeout: Duration) -> Result<Option<Output>, Stop> {
        let cli = self.cli.with_timeout(timeout);
        self.call_on(&cli, args)
    }

    fn call_on(&mut self, cli: &Nordvpn, args: &[&str]) -> Result<Option<Output>, Stop> {
        self.sup.poll()?;
        match cli.run(args) {
            Ok(o) => Ok(Some(o)),
            Err(e) => {
                warn!("nordvpn {}: {e}", args.first().copied().unwrap_or(""));
                Ok(None)
            }
        }
    }
}

fn warn_false(what: &str, o: &Output) -> bool {
    warn!("{what} failed: {}", o.summary());
    false
}
