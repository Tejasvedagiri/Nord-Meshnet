//! Parsers for `nordvpn status`, `nordvpn settings` and `nordvpn account`.
//! All of them print `Key: Value` lines, sometimes after spinner noise.

use std::collections::HashMap;

/// Strip ANSI escapes and carriage-return spinner frames (`\r-\r  \r`),
/// keeping only what a terminal would finally show on each line.
pub fn clean(raw: &str) -> String {
    let mut no_ansi = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // CSI: parameters/intermediates until a final byte in @..~
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        no_ansi.push(c);
    }
    no_ansi
        .lines()
        .map(|l| {
            l.rsplit('\r')
                .find(|s| !s.trim().is_empty())
                .unwrap_or("")
                .trim_end()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `Key: Value` lines into a map with lowercased keys. Later keys win.
pub fn key_values(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in clean(text).lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            if !k.is_empty() {
                map.insert(k.to_ascii_lowercase(), v.trim().to_string());
            }
        }
    }
    map
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub connected: bool,
    pub server: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
}

pub fn parse_status(text: &str) -> Status {
    let kv = key_values(text);
    Status {
        connected: kv
            .get("status")
            .is_some_and(|s| s.eq_ignore_ascii_case("connected")),
        server: kv.get("server").or_else(|| kv.get("hostname")).cloned(),
        country: kv.get("country").cloned(),
        city: kv.get("city").cloned(),
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub killswitch: Option<bool>,
    pub meshnet: Option<bool>,
}

pub fn parse_settings(text: &str) -> Settings {
    let kv = key_values(text);
    let flag = |k: &str| kv.get(k).and_then(|v| crate::config::parse_bool(v));
    Settings {
        killswitch: flag("kill switch"),
        meshnet: flag("meshnet"),
    }
}

/// `nordvpn account` exits 0 and prints account details when logged in.
pub fn logged_in(exit_ok: bool, text: &str) -> bool {
    exit_ok && !text.to_ascii_lowercase().contains("not logged in")
}

/// The CLI exits non-zero when asked for the state it's already in, which
/// happens on every restart with a persisted /var/lib/nordvpn.
/// "already exists" / "already in use" are conflicts, not no-ops (e.g. a
/// nickname equal to the hostname: "A domain with this name already exists").
pub fn already_done(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    let conflict = t.contains("already exists") || t.contains("already in use");
    (t.contains("already") && !conflict) || t.contains("nothing to do")
}
