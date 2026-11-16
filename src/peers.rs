//! Meshnet peers: parse `nordvpn meshnet peer list`, resolve configured names,
//! and diff desired against actual permissions so only real changes are sent.

use std::collections::HashMap;

use crate::config::PeerDefault;
use crate::status::clean;

/// A permission *this node* grants a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Perm {
    /// Remote access: "Allow Incoming Traffic" / `peer incoming`.
    Remote,
    Routing,
    Local,
    Fileshare,
}

impl Perm {
    pub const ALL: [Perm; 4] = [Perm::Remote, Perm::Routing, Perm::Local, Perm::Fileshare];

    pub fn index(self) -> usize {
        self as usize
    }

    /// Suffix of the NORDVPN_{ALLOW,DENY}_PEER_* variables.
    pub fn env_suffix(self) -> &'static str {
        match self {
            Perm::Remote => "REMOTE",
            Perm::Routing => "ROUTING",
            Perm::Local => "LOCAL",
            Perm::Fileshare => "FILESHARE",
        }
    }

    /// `nordvpn meshnet peer <verb> allow|deny <peer>`
    pub fn cli_verb(self) -> &'static str {
        match self {
            Perm::Remote => "incoming",
            Perm::Routing => "routing",
            Perm::Local => "local",
            Perm::Fileshare => "fileshare",
        }
    }

    /// Peer-list key (lowercased) for what this node allows the peer.
    /// "Allow X" is ours; "Allows X" is what the peer grants us.
    fn from_list_key(key: &str) -> Option<Perm> {
        match key {
            "allow incoming traffic" | "allow incoming connections" => Some(Perm::Remote),
            "allow routing" => Some(Perm::Routing),
            "allow local network access" => Some(Perm::Local),
            "allow sending files" => Some(Perm::Fileshare),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerRules {
    pub allow: [Vec<String>; 4],
    pub deny: [Vec<String>; 4],
    pub default: PeerDefault,
}

impl Default for PeerRules {
    fn default() -> Self {
        PeerRules {
            allow: Default::default(),
            deny: Default::default(),
            default: PeerDefault::Keep,
        }
    }
}

impl PeerRules {
    pub fn is_empty(&self) -> bool {
        self.allow.iter().chain(self.deny.iter()).all(Vec::is_empty)
    }

    /// Every configured identifier, deduplicated (case-insensitive).
    pub fn names(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for n in self.deny.iter().chain(self.allow.iter()).flatten() {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(n)) {
                out.push(n);
            }
        }
        out
    }

    pub fn is_active(&self) -> bool {
        !self.is_empty() || self.default == PeerDefault::Deny
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub hostname: String,
    pub nickname: Option<String>,
    pub ip: String,
    pub pubkey: String,
    /// What this node grants the peer; `None` if the field wasn't printed.
    pub perms: [Option<bool>; 4],
}

impl Peer {
    pub fn display_name(&self) -> &str {
        self.nickname.as_deref().unwrap_or(&self.hostname)
    }

    pub fn matches(&self, id: &str) -> bool {
        let id = id.trim();
        self.nickname
            .as_deref()
            .is_some_and(|n| n.eq_ignore_ascii_case(id))
            || self.hostname.eq_ignore_ascii_case(id)
            || self
                .hostname
                .strip_suffix(".nord")
                .is_some_and(|h| h.eq_ignore_ascii_case(id))
            || (!self.ip.is_empty() && self.ip == id)
            || (!self.pubkey.is_empty() && self.pubkey == id)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerList {
    /// This node, from the "This device:" section.
    pub this_device: Option<Peer>,
    pub peers: Vec<Peer>,
}

impl PeerList {
    /// Resolve a configured identifier. Ambiguous matches (e.g. a nickname
    /// equal to another peer's hostname) resolve to nothing, loudly.
    pub fn resolve(&self, id: &str) -> Resolution<'_> {
        let hits: Vec<&Peer> = self.peers.iter().filter(|p| p.matches(id)).collect();
        match hits.as_slice() {
            [] => Resolution::NotFound,
            [p] => Resolution::Found(p),
            _ => Resolution::Ambiguous,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Resolution<'a> {
    Found(&'a Peer),
    NotFound,
    Ambiguous,
}

pub fn parse_peer_list(raw: &str) -> PeerList {
    #[derive(PartialEq)]
    enum Section {
        None,
        ThisDevice,
        Peers,
    }

    let mut list = PeerList::default();
    let mut section = Section::None;
    let mut current: Option<Peer> = None;

    fn flush(list: &mut PeerList, section: &Section, current: &mut Option<Peer>) {
        if let Some(p) = current.take() {
            if p.hostname.is_empty() && p.pubkey.is_empty() && p.ip.is_empty() {
                return;
            }
            match section {
                Section::ThisDevice => list.this_device = Some(p),
                Section::Peers => list.peers.push(p),
                Section::None => {}
            }
        }
    }

    for line in clean(raw).lines() {
        let line = line.trim();
        if line.is_empty() {
            flush(&mut list, &section, &mut current);
            continue;
        }
        match line.to_ascii_lowercase().as_str() {
            "this device:" => {
                flush(&mut list, &section, &mut current);
                section = Section::ThisDevice;
                continue;
            }
            "local peers:" | "external peers:" => {
                flush(&mut list, &section, &mut current);
                section = Section::Peers;
                continue;
            }
            _ => {}
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim();
        // Each peer block starts with its title line: Hostname or Nickname.
        let starts_block = matches!(key.as_str(), "hostname" | "nickname");
        let peer = current.get_or_insert_with(Peer::default);
        if starts_block
            && ((key == "hostname" && !peer.hostname.is_empty())
                || (key == "nickname" && peer.nickname.is_some()))
        {
            // A new block without a blank line in between.
            flush(&mut list, &section, &mut current);
        }
        let peer = current.get_or_insert_with(Peer::default);
        match key.as_str() {
            "hostname" => peer.hostname = value.to_string(),
            "nickname" => {
                peer.nickname = Some(value.to_string()).filter(|v| !v.is_empty() && v != "-")
            }
            "ip" => peer.ip = value.to_string(),
            "public key" => peer.pubkey = value.to_string(),
            k => {
                if let Some(perm) = Perm::from_list_key(k) {
                    peer.perms[perm.index()] = crate::config::parse_bool(value);
                }
            }
        }
    }
    flush(&mut list, &section, &mut current);
    list
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// Passed to the CLI: the public key is the one identifier it always
    /// matches exactly.
    pub target: String,
    pub name: String,
    pub perm: Perm,
    pub allow: bool,
}

impl Action {
    pub fn args(&self) -> [&str; 5] {
        [
            "meshnet",
            "peer",
            self.perm.cli_verb(),
            if self.allow { "allow" } else { "deny" },
            &self.target,
        ]
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// Configured names with no matching peer (yet).
    pub unresolved: Vec<String>,
    /// Configured names that match more than one peer.
    pub ambiguous: Vec<String>,
}

/// Desired permissions: DENY lists, then ALLOW lists (ALLOW wins), then
/// strict-mode denies for routing/local on anything not explicitly allowed.
pub fn plan(rules: &PeerRules, list: &PeerList) -> Plan {
    let mut out = Plan::default();
    // pubkey (or hostname) → desired flags
    let mut desired: HashMap<String, [Option<bool>; 4]> = HashMap::new();
    let key = |p: &Peer| {
        if p.pubkey.is_empty() {
            p.hostname.clone()
        } else {
            p.pubkey.clone()
        }
    };

    for (lists, allow) in [(&rules.deny, false), (&rules.allow, true)] {
        for perm in Perm::ALL {
            for name in &lists[perm.index()] {
                match list.resolve(name) {
                    Resolution::Found(p) => {
                        desired.entry(key(p)).or_default()[perm.index()] = Some(allow);
                    }
                    Resolution::NotFound => push_unique(&mut out.unresolved, name),
                    Resolution::Ambiguous => push_unique(&mut out.ambiguous, name),
                }
            }
        }
    }

    if rules.default == PeerDefault::Deny {
        for p in &list.peers {
            let d = desired.entry(key(p)).or_default();
            for perm in [Perm::Routing, Perm::Local] {
                if d[perm.index()] != Some(true) {
                    d[perm.index()] = Some(false);
                }
            }
        }
    }

    // Denies first (local before routing), then allows (routing before
    // local), since local network access rides on routing.
    const DENY_ORDER: [Perm; 4] = [Perm::Local, Perm::Routing, Perm::Remote, Perm::Fileshare];
    const ALLOW_ORDER: [Perm; 4] = [Perm::Routing, Perm::Local, Perm::Remote, Perm::Fileshare];
    for (order, allow) in [(DENY_ORDER, false), (ALLOW_ORDER, true)] {
        for p in &list.peers {
            let Some(d) = desired.get(&key(p)) else {
                continue;
            };
            for perm in order {
                if d[perm.index()] == Some(allow) && p.perms[perm.index()] != Some(allow) {
                    out.actions.push(Action {
                        target: key(p),
                        name: p.display_name().to_string(),
                        perm,
                        allow,
                    });
                }
            }
        }
    }
    out
}

fn push_unique(v: &mut Vec<String>, s: &str) {
    if !v.iter().any(|x| x.eq_ignore_ascii_case(s)) {
        v.push(s.to_string());
    }
}
