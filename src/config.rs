//! Environment → validated `Config`. Every value is checked up front so a
//! typo fails the container immediately instead of half-applying.

use std::collections::HashSet;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

use crate::log::Level;
use crate::peers::{PeerRules, Perm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerDefault {
    /// Leave peers that aren't listed alone.
    Keep,
    /// Deny routing and local access to peers not in the matching ALLOW list.
    Deny,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Port {
    Single(u16),
    Range(u16, u16),
}

#[derive(Clone, Debug)]
pub struct Config {
    pub token: Option<String>,
    /// `None` = no VPN tunnel. `Some(vec![])` = recommended server.
    pub connect: Option<Vec<String>>,
    pub protocol: String,
    pub killswitch: bool,
    pub allowlist_subnets: Vec<String>,
    pub allowlist_ports: Vec<Port>,
    pub dns: Vec<String>,
    /// `nordvpn set` argument lists from NORDVPN_SET.
    pub set: Vec<Vec<String>>,
    pub meshnet: bool,
    pub nickname: Option<String>,
    pub peers: PeerRules,
    pub reconcile_interval: Duration,
    pub log_level: Level,
    pub analytics: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfigError(pub Vec<String>);

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "invalid configuration:")?;
        for e in &self.0 {
            writeln!(f, "  - {e}")?;
        }
        Ok(())
    }
}

pub const PROTOCOLS: &[&str] = &["nordlynx", "openvpn_udp", "openvpn_tcp", "nordwhisper"];

/// `nordvpn set` keys that NORDVPN_SET may not touch because a dedicated
/// variable owns them and the startup order depends on it.
const RESERVED_SET_KEYS: &[(&str, &str)] = &[
    ("killswitch", "NORDVPN_KILLSWITCH"),
    ("meshnet", "NORDVPN_MESHNET"),
    ("protocol", "NORDVPN_PROTOCOL"),
    ("dns", "NORDVPN_DNS"),
    ("analytics", "NORDMESH_ANALYTICS"),
];

impl Config {
    pub fn from_env() -> Result<Config, ConfigError> {
        Config::from_lookup(
            |k| std::env::var(k).ok(),
            |path| std::fs::read_to_string(path),
        )
    }

    /// `lookup` reads a variable; `read_file` backs the `*_FILE` variants.
    pub fn from_lookup(
        lookup: impl Fn(&str) -> Option<String>,
        read_file: impl Fn(&str) -> std::io::Result<String>,
    ) -> Result<Config, ConfigError> {
        let mut errs = Vec::new();

        // --- auth ---
        let token = match lookup("NORDVPN_TOKEN_FILE").filter(|p| !p.trim().is_empty()) {
            Some(path) => match read_file(path.trim()) {
                Ok(s) => Some(s.trim().to_string()).filter(|t| !t.is_empty()),
                Err(e) => {
                    errs.push(format!(
                        "NORDVPN_TOKEN_FILE: cannot read {}: {e}",
                        path.trim()
                    ));
                    None
                }
            },
            None => lookup("NORDVPN_TOKEN")
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty()),
        };
        if let Some(t) = &token {
            if !t.chars().all(|c| c.is_ascii_alphanumeric()) {
                errs.push("NORDVPN_TOKEN: expected an alphanumeric access token".into());
            }
        }

        // --- VPN ---
        let connect = lookup("NORDVPN_CONNECT")
            .map(|v| v.split_whitespace().map(str::to_string).collect::<Vec<_>>());
        if let Some(args) = &connect {
            for a in args {
                if !valid_connect_arg(a) {
                    errs.push(format!("NORDVPN_CONNECT: invalid argument {a:?}"));
                }
            }
        }

        let protocol = lookup("NORDVPN_PROTOCOL")
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "nordlynx".into());
        if !PROTOCOLS.contains(&protocol.as_str()) {
            errs.push(format!(
                "NORDVPN_PROTOCOL: {protocol:?} is not one of {}",
                PROTOCOLS.join(", ")
            ));
        }

        let killswitch = match nonempty(lookup("NORDVPN_KILLSWITCH")) {
            None => connect.is_some(),
            Some(v) => parse_bool(&v).unwrap_or_else(|| {
                errs.push(format!("NORDVPN_KILLSWITCH: expected on/off, got {v:?}"));
                false
            }),
        };
        if killswitch && connect.is_none() {
            errs.push(
                "NORDVPN_KILLSWITCH=on requires NORDVPN_CONNECT: the kill switch is only \
                 enabled after a successful connect"
                    .into(),
            );
        }

        let mut allowlist_subnets = Vec::new();
        for s in list(lookup("NORDVPN_ALLOWLIST_SUBNETS")) {
            if valid_cidr(&s) {
                allowlist_subnets.push(s);
            } else {
                errs.push(format!("NORDVPN_ALLOWLIST_SUBNETS: invalid subnet {s:?}"));
            }
        }

        let mut allowlist_ports = Vec::new();
        for p in list(lookup("NORDVPN_ALLOWLIST_PORTS")) {
            match parse_port(&p) {
                Some(port) => allowlist_ports.push(port),
                None => errs.push(format!(
                    "NORDVPN_ALLOWLIST_PORTS: invalid port or range {p:?} (e.g. 8080 or 6881-6889)"
                )),
            }
        }

        let mut dns = Vec::new();
        for d in list(lookup("NORDVPN_DNS")) {
            if d.parse::<IpAddr>().is_ok() {
                dns.push(d);
            } else {
                errs.push(format!("NORDVPN_DNS: invalid IP address {d:?}"));
            }
        }
        if dns.len() > 3 {
            errs.push("NORDVPN_DNS: at most 3 servers are supported".into());
        }

        let mut set = Vec::new();
        for entry in lookup("NORDVPN_SET").unwrap_or_default().split(';') {
            let args: Vec<String> = entry.split_whitespace().map(str::to_string).collect();
            let Some(key) = args.first() else { continue };
            let key_lc = key.to_ascii_lowercase();
            if let Some((_, owner)) = RESERVED_SET_KEYS.iter().find(|(k, _)| *k == key_lc) {
                errs.push(format!("NORDVPN_SET: use {owner} instead of `set {key}`"));
            } else if !args.iter().all(|a| valid_set_arg(a)) {
                errs.push(format!("NORDVPN_SET: invalid entry {:?}", entry.trim()));
            } else {
                set.push(args);
            }
        }

        // --- Meshnet ---
        let meshnet = match nonempty(lookup("NORDVPN_MESHNET")) {
            None => false,
            Some(v) => parse_bool(&v).unwrap_or_else(|| {
                errs.push(format!("NORDVPN_MESHNET: expected on/off, got {v:?}"));
                false
            }),
        };

        let nickname = nonempty(lookup("NORDVPN_NICKNAME")).map(|n| n.trim().to_string());
        if let Some(n) = &nickname {
            if !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
                || n.starts_with('-')
                || n.ends_with('-')
            {
                errs.push(format!(
                    "NORDVPN_NICKNAME: {n:?} may only contain letters, digits and inner hyphens"
                ));
            }
            if !meshnet {
                errs.push("NORDVPN_NICKNAME requires NORDVPN_MESHNET=on".into());
            }
        }

        let mut peers = PeerRules::default();
        for perm in Perm::ALL {
            for (allow, prefix) in [(true, "ALLOW"), (false, "DENY")] {
                let var = format!("NORDVPN_{prefix}_PEER_{}", perm.env_suffix());
                for name in list(lookup(&var)) {
                    if !valid_peer_id(&name) {
                        errs.push(format!(
                            "{var}: {name:?} is not a nickname, hostname or IPv4 address"
                        ));
                        continue;
                    }
                    let target = if allow {
                        &mut peers.allow[perm.index()]
                    } else {
                        &mut peers.deny[perm.index()]
                    };
                    if !target.iter().any(|n| n.eq_ignore_ascii_case(&name)) {
                        target.push(name);
                    }
                }
            }
        }
        peers.default = match nonempty(lookup("NORDVPN_PEER_DEFAULT"))
            .map(|v| v.trim().to_ascii_lowercase())
            .as_deref()
        {
            None | Some("keep") => PeerDefault::Keep,
            Some("deny") => PeerDefault::Deny,
            Some(v) => {
                errs.push(format!(
                    "NORDVPN_PEER_DEFAULT: expected keep or deny, got {v:?}"
                ));
                PeerDefault::Keep
            }
        };

        let routing: HashSet<String> = peers.allow[Perm::Routing.index()]
            .iter()
            .map(|n| n.to_ascii_lowercase())
            .collect();
        for name in &peers.allow[Perm::Local.index()] {
            if !routing.contains(&name.to_ascii_lowercase()) {
                errs.push(format!(
                    "NORDVPN_ALLOW_PEER_LOCAL: {name:?} must also be in NORDVPN_ALLOW_PEER_ROUTING \
                     (local network access only works through routing)"
                ));
            }
        }
        if !meshnet && (!peers.is_empty() || peers.default == PeerDefault::Deny) {
            errs.push("peer permission variables require NORDVPN_MESHNET=on".into());
        }

        // --- runtime ---
        let reconcile_interval = match nonempty(lookup("NORDMESH_RECONCILE_INTERVAL")) {
            None => Duration::from_secs(60),
            Some(v) => match v.trim().parse::<u64>() {
                Ok(n) if n >= 10 => Duration::from_secs(n),
                _ => {
                    errs.push(format!(
                        "NORDMESH_RECONCILE_INTERVAL: expected whole seconds >= 10, got {v:?}"
                    ));
                    Duration::from_secs(60)
                }
            },
        };

        let log_level = match nonempty(lookup("NORDMESH_LOG_LEVEL")) {
            None => Level::Info,
            Some(v) => Level::parse(&v).unwrap_or_else(|| {
                errs.push(format!(
                    "NORDMESH_LOG_LEVEL: expected debug, info, warn or error, got {v:?}"
                ));
                Level::Info
            }),
        };

        let analytics = match nonempty(lookup("NORDMESH_ANALYTICS")) {
            None => false,
            Some(v) => parse_bool(&v).unwrap_or_else(|| {
                errs.push(format!("NORDMESH_ANALYTICS: expected on/off, got {v:?}"));
                false
            }),
        };

        if !errs.is_empty() {
            return Err(ConfigError(errs));
        }
        Ok(Config {
            token,
            connect,
            protocol,
            killswitch,
            allowlist_subnets,
            allowlist_ports,
            dns,
            set,
            meshnet,
            nickname,
            peers,
            reconcile_interval,
            log_level,
            analytics,
        })
    }
}

/// Effective config, token redacted. Printed once at startup.
impl fmt::Display for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let onoff = |b: bool| if b { "on" } else { "off" };
        let join = |v: &[String]| {
            if v.is_empty() {
                "-".to_string()
            } else {
                v.join(",")
            }
        };
        writeln!(
            f,
            "token:          {}",
            if self.token.is_some() {
                "<redacted>"
            } else {
                "-"
            }
        )?;
        let connect = match &self.connect {
            None => "- (no VPN tunnel)".to_string(),
            Some(a) if a.is_empty() => "(recommended server)".to_string(),
            Some(a) => a.join(" "),
        };
        writeln!(f, "connect:        {connect}")?;
        writeln!(f, "protocol:       {}", self.protocol)?;
        writeln!(f, "killswitch:     {}", onoff(self.killswitch))?;
        writeln!(f, "allow subnets:  {}", join(&self.allowlist_subnets))?;
        let ports: Vec<String> = self
            .allowlist_ports
            .iter()
            .map(|p| match p {
                Port::Single(p) => p.to_string(),
                Port::Range(a, b) => format!("{a}-{b}"),
            })
            .collect();
        writeln!(f, "allow ports:    {}", join(&ports))?;
        writeln!(f, "dns:            {}", join(&self.dns))?;
        let set: Vec<String> = self.set.iter().map(|a| a.join(" ")).collect();
        writeln!(
            f,
            "set:            {}",
            if set.is_empty() {
                "-".into()
            } else {
                set.join("; ")
            }
        )?;
        writeln!(f, "meshnet:        {}", onoff(self.meshnet))?;
        writeln!(
            f,
            "nickname:       {}",
            self.nickname.as_deref().unwrap_or("-")
        )?;
        for perm in Perm::ALL {
            let i = perm.index();
            if !self.peers.allow[i].is_empty() {
                writeln!(
                    f,
                    "allow {:<9} {}",
                    perm.env_suffix().to_ascii_lowercase(),
                    join(&self.peers.allow[i])
                )?;
            }
            if !self.peers.deny[i].is_empty() {
                writeln!(
                    f,
                    "deny {:<10} {}",
                    perm.env_suffix().to_ascii_lowercase(),
                    join(&self.peers.deny[i])
                )?;
            }
        }
        let default = match self.peers.default {
            PeerDefault::Keep => "keep",
            PeerDefault::Deny => "deny",
        };
        writeln!(f, "peer default:   {default}")?;
        writeln!(f, "reconcile:      {}s", self.reconcile_interval.as_secs())?;
        writeln!(f, "log level:      {}", self.log_level.as_str())?;
        write!(f, "analytics:      {}", onoff(self.analytics))
    }
}

fn nonempty(v: Option<String>) -> Option<String> {
    v.filter(|s| !s.trim().is_empty())
}

/// Comma-separated list, whitespace trimmed, empties dropped.
pub fn list(v: Option<String>) -> Vec<String> {
    v.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "1" | "enabled" | "enable" => Some(true),
        "off" | "false" | "no" | "0" | "disabled" | "disable" => Some(false),
        _ => None,
    }
}

fn parse_port(s: &str) -> Option<Port> {
    let port = |p: &str| p.trim().parse::<u16>().ok().filter(|&n| n > 0);
    match s.split_once('-') {
        None => port(s).map(Port::Single),
        Some((a, b)) => {
            let (a, b) = (port(a)?, port(b)?);
            (a < b).then_some(Port::Range(a, b))
        }
    }
}

pub fn valid_cidr(s: &str) -> bool {
    let (addr, prefix) = match s.split_once('/') {
        Some((a, p)) => (a, Some(p)),
        None => (s, None),
    };
    let Ok(ip) = addr.parse::<IpAddr>() else {
        return false;
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    match prefix {
        None => true,
        Some(p) => p.parse::<u8>().map(|n| n <= max).unwrap_or(false),
    }
}

/// Peer identifiers: nickname / hostname (`[A-Za-z0-9._-]+`) or IPv4.
pub fn valid_peer_id(s: &str) -> bool {
    s.parse::<Ipv4Addr>().is_ok()
        || (!s.is_empty()
            && !s.starts_with('-')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')))
}

/// `nordvpn connect` args: country/city/server names and `--group X`.
fn valid_connect_arg(s: &str) -> bool {
    let body = s.strip_prefix("--").unwrap_or(s);
    !body.is_empty()
        && !body.starts_with('-')
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// `nordvpn set` args. Never reaches a shell; this just keeps junk out.
fn valid_set_arg(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/' | ','))
}
