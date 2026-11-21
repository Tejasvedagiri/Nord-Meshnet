use std::collections::HashMap;
use std::time::Duration;

use nordmesh::config::{Config, ConfigError, PeerDefault, Port};
use nordmesh::peers::Perm;

fn parse(vars: &[(&str, &str)]) -> Result<Config, ConfigError> {
    parse_with_files(vars, &[])
}

fn parse_with_files(vars: &[(&str, &str)], files: &[(&str, &str)]) -> Result<Config, ConfigError> {
    let env: HashMap<String, String> = vars
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let files: HashMap<String, String> = files
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    Config::from_lookup(
        |k| env.get(k).cloned(),
        |p| {
            files
                .get(p)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "missing"))
        },
    )
}

fn errors(vars: &[(&str, &str)]) -> Vec<String> {
    parse(vars).unwrap_err().0
}

#[test]
fn defaults() {
    let c = parse(&[]).unwrap();
    assert_eq!(c.token, None);
    assert_eq!(c.connect, None);
    assert_eq!(c.protocol, "nordlynx");
    assert!(!c.killswitch);
    assert!(!c.meshnet);
    assert_eq!(c.peers.default, PeerDefault::Keep);
    assert_eq!(c.reconcile_interval, Duration::from_secs(60));
    assert!(!c.analytics);
}

#[test]
fn acceptance_compose() {
    let c = parse(&[
        ("NORDVPN_TOKEN", "abc123def456"),
        ("NORDVPN_CONNECT", "Germany"),
        ("NORDVPN_ALLOWLIST_SUBNETS", "192.168.1.0/24"),
        ("NORDVPN_MESHNET", "on"),
        ("NORDVPN_NICKNAME", "home-node"),
        ("NORDVPN_ALLOW_PEER_ROUTING", "laptop, desktop,server"),
        ("NORDVPN_ALLOW_PEER_LOCAL", "laptop,desktop,server"),
    ])
    .unwrap();
    assert_eq!(c.connect, Some(vec!["Germany".to_string()]));
    assert!(c.killswitch, "kill switch defaults on when CONNECT is set");
    assert_eq!(c.allowlist_subnets, vec!["192.168.1.0/24"]);
    assert!(c.meshnet);
    assert_eq!(c.nickname.as_deref(), Some("home-node"));
    assert_eq!(
        c.peers.allow[Perm::Routing.index()],
        vec!["laptop", "desktop", "server"]
    );
}

#[test]
fn empty_connect_means_recommended() {
    let c = parse(&[("NORDVPN_CONNECT", "")]).unwrap();
    assert_eq!(c.connect, Some(vec![]));
    assert!(c.killswitch);
}

#[test]
fn connect_multiword_and_group() {
    let c = parse(&[("NORDVPN_CONNECT", "--group P2P Netherlands")]).unwrap();
    assert_eq!(
        c.connect,
        Some(vec!["--group".into(), "P2P".into(), "Netherlands".into()])
    );
    assert!(!errors(&[("NORDVPN_CONNECT", "Germany; rm -rf /")]).is_empty());
}

#[test]
fn killswitch_needs_connect() {
    let e = errors(&[("NORDVPN_KILLSWITCH", "on")]);
    assert!(e[0].contains("requires NORDVPN_CONNECT"), "{e:?}");
    let c = parse(&[
        ("NORDVPN_CONNECT", "Germany"),
        ("NORDVPN_KILLSWITCH", "off"),
    ])
    .unwrap();
    assert!(!c.killswitch);
}

#[test]
fn token_file_takes_precedence() {
    let c = parse_with_files(
        &[
            ("NORDVPN_TOKEN", "fromenv"),
            ("NORDVPN_TOKEN_FILE", "/run/secrets/t"),
        ],
        &[("/run/secrets/t", "fromfile\n")],
    )
    .unwrap();
    assert_eq!(c.token.as_deref(), Some("fromfile"));

    let e = parse_with_files(&[("NORDVPN_TOKEN_FILE", "/nope")], &[])
        .unwrap_err()
        .0;
    assert!(e[0].contains("cannot read"), "{e:?}");
}

#[test]
fn token_redacted_in_display() {
    let c = parse(&[("NORDVPN_TOKEN", "supersecret123")]).unwrap();
    let shown = c.to_string();
    assert!(!shown.contains("supersecret123"));
    assert!(shown.contains("<redacted>"));
}

#[test]
fn local_without_routing_is_an_error() {
    let e = errors(&[
        ("NORDVPN_MESHNET", "on"),
        ("NORDVPN_ALLOW_PEER_ROUTING", "a"),
        ("NORDVPN_ALLOW_PEER_LOCAL", "A,b"),
    ]);
    assert_eq!(e.len(), 1, "{e:?}");
    assert!(e[0].contains("\"b\""), "{e:?}");
}

#[test]
fn peer_vars_need_meshnet() {
    let e = errors(&[("NORDVPN_ALLOW_PEER_ROUTING", "a")]);
    assert!(e.iter().any(|m| m.contains("NORDVPN_MESHNET=on")), "{e:?}");
    let e = errors(&[("NORDVPN_NICKNAME", "x")]);
    assert!(e.iter().any(|m| m.contains("NORDVPN_MESHNET=on")), "{e:?}");
}

#[test]
fn peer_identifier_validation() {
    let ok = parse(&[
        ("NORDVPN_MESHNET", "on"),
        (
            "NORDVPN_ALLOW_PEER_FILESHARE",
            "my-mac, host.nord, 100.64.0.5, under_score",
        ),
    ])
    .unwrap();
    assert_eq!(ok.peers.allow[Perm::Fileshare.index()].len(), 4);

    for bad in ["$(id)", "a b", "-rf", "x;y"] {
        let e = errors(&[("NORDVPN_MESHNET", "on"), ("NORDVPN_DENY_PEER_REMOTE", bad)]);
        assert!(!e.is_empty(), "{bad} should be rejected");
    }
}

#[test]
fn peer_default() {
    let c = parse(&[("NORDVPN_MESHNET", "on"), ("NORDVPN_PEER_DEFAULT", "DENY")]).unwrap();
    assert_eq!(c.peers.default, PeerDefault::Deny);
    assert!(!errors(&[("NORDVPN_MESHNET", "on"), ("NORDVPN_PEER_DEFAULT", "drop")]).is_empty());
}

#[test]
fn ports_and_subnets() {
    let c = parse(&[
        ("NORDVPN_ALLOWLIST_PORTS", "8080, 6881-6889"),
        (
            "NORDVPN_ALLOWLIST_SUBNETS",
            "10.0.0.0/8,fd00::/8,192.168.1.5",
        ),
    ])
    .unwrap();
    assert_eq!(
        c.allowlist_ports,
        vec![Port::Single(8080), Port::Range(6881, 6889)]
    );
    assert_eq!(c.allowlist_subnets.len(), 3);

    assert!(!errors(&[("NORDVPN_ALLOWLIST_PORTS", "0")]).is_empty());
    assert!(!errors(&[("NORDVPN_ALLOWLIST_PORTS", "9-1")]).is_empty());
    assert!(!errors(&[("NORDVPN_ALLOWLIST_SUBNETS", "10.0.0.0/33")]).is_empty());
    assert!(!errors(&[("NORDVPN_ALLOWLIST_SUBNETS", "lan")]).is_empty());
}

#[test]
fn dns() {
    let c = parse(&[("NORDVPN_DNS", "1.1.1.1, 9.9.9.9")]).unwrap();
    assert_eq!(c.dns, vec!["1.1.1.1", "9.9.9.9"]);
    assert!(!errors(&[("NORDVPN_DNS", "1.1.1.1,2.2.2.2,3.3.3.3,4.4.4.4")]).is_empty());
    assert!(!errors(&[("NORDVPN_DNS", "dns.google")]).is_empty());
}

#[test]
fn set_passthrough() {
    let c = parse(&[("NORDVPN_SET", "post-quantum on; lan-discovery off ;")]).unwrap();
    assert_eq!(
        c.set,
        vec![vec!["post-quantum", "on"], vec!["lan-discovery", "off"]]
    );
    // Keys owned by dedicated variables are refused.
    let e = errors(&[("NORDVPN_SET", "killswitch on")]);
    assert!(e[0].contains("NORDVPN_KILLSWITCH"), "{e:?}");
    assert!(!errors(&[("NORDVPN_SET", "Meshnet on")]).is_empty());
    assert!(!errors(&[("NORDVPN_SET", "notify $(reboot)")]).is_empty());
}

#[test]
fn protocol_and_runtime() {
    assert_eq!(
        parse(&[("NORDVPN_PROTOCOL", "OpenVPN_TCP")])
            .unwrap()
            .protocol,
        "openvpn_tcp"
    );
    assert!(!errors(&[("NORDVPN_PROTOCOL", "ikev2")]).is_empty());
    assert!(!errors(&[("NORDMESH_RECONCILE_INTERVAL", "5")]).is_empty());
    assert!(!errors(&[("NORDMESH_LOG_LEVEL", "loud")]).is_empty());
    assert!(!errors(&[("NORDVPN_MESHNET", "maybe")]).is_empty());
}

#[test]
fn collects_all_errors() {
    let e = errors(&[
        ("NORDVPN_PROTOCOL", "x"),
        ("NORDVPN_DNS", "y"),
        ("NORDMESH_LOG_LEVEL", "z"),
    ]);
    assert_eq!(e.len(), 3, "{e:?}");
}
