use nordmesh::config::PeerDefault;
use nordmesh::peers::{self, Action, PeerRules, Perm, Resolution};
use nordmesh::status;

fn fixture(version: &str, name: &str) -> String {
    let path = format!(
        "{}/tests/fixtures/{version}/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

// ------------------------------------------------------------------ status

#[test]
fn status_connected_with_spinner_noise() {
    let s = status::parse_status(&fixture("6.0.2", "status_connected.txt"));
    assert!(s.connected);
    assert_eq!(s.country.as_deref(), Some("Germany"));
    assert_eq!(s.city.as_deref(), Some("Mumbai"));
}

#[test]
fn status_disconnected() {
    assert!(!status::parse_status(&fixture("6.0.2", "status_disconnected.txt")).connected);
}

#[test]
fn status_strips_ansi() {
    let s = status::parse_status("\x1b[1mStatus:\x1b[0m Connected\n");
    assert!(s.connected);
}

#[test]
fn settings() {
    let s = status::parse_settings(&fixture("6.0.2", "settings.txt"));
    assert_eq!(s.killswitch, Some(true));
    assert_eq!(s.meshnet, Some(true));
}

#[test]
fn account() {
    assert!(!status::logged_in(
        false,
        &fixture("6.0.2", "account_logged_out.txt")
    ));
    assert!(status::logged_in(
        true,
        "Account Information:\nEmail Address: x@y\n"
    ));
}

#[test]
fn already_done() {
    assert!(status::already_done("Meshnet is already enabled."));
    assert!(status::already_done(
        "Kill Switch is already set to 'disabled'."
    ));
    assert!(!status::already_done("You are not logged in."));
    assert!(!status::already_done(
        "The nickname is unavailable: A domain with this name already exists in your system."
    ));
}

// --------------------------------------------------------------- peer list

#[test]
fn peer_list_parses_sections() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    let me = l.this_device.as_ref().unwrap();
    assert_eq!(me.nickname.as_deref(), Some("home-node"));
    assert_eq!(me.hostname, "alpha-delta.nord");
    assert_eq!(l.peers.len(), 4);

    let mac = &l.peers[0];
    assert_eq!(mac.nickname.as_deref(), Some("laptop"));
    assert_eq!(mac.ip, "100.64.0.11");
    // "Allow X" (ours), not "Allows X" (theirs).
    assert_eq!(mac.perms[Perm::Remote.index()], Some(true));
    assert_eq!(mac.perms[Perm::Routing.index()], Some(false));
    assert_eq!(mac.perms[Perm::Local.index()], Some(false));
    assert_eq!(mac.perms[Perm::Fileshare.index()], Some(true));

    let linux = &l.peers[2];
    assert_eq!(linux.nickname, None, "'-' means no nickname");
    assert_eq!(linux.hostname, "delta-golf.nord");
}

#[test]
fn peer_list_without_peers() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list_empty.txt"));
    assert!(l.peers.is_empty());
    assert_eq!(l.this_device.unwrap().nickname, None);
}

#[test]
fn resolve_by_nickname_hostname_ip() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    for id in ["Laptop", "bravo-echo.nord", "bravo-echo", "100.64.0.11"] {
        match l.resolve(id) {
            Resolution::Found(p) => assert_eq!(p.hostname, "bravo-echo.nord", "{id}"),
            other => panic!("{id}: {other:?}"),
        }
    }
    assert_eq!(l.resolve("nobody"), Resolution::NotFound);
    // This device is never a peer.
    assert_eq!(l.resolve("home-node"), Resolution::NotFound);
}

// -------------------------------------------------------------------- diff

fn rules(allow: &[(Perm, &[&str])], deny: &[(Perm, &[&str])], default: PeerDefault) -> PeerRules {
    let mut r = PeerRules {
        default,
        ..Default::default()
    };
    for (perm, names) in allow {
        r.allow[perm.index()] = names.iter().map(|s| s.to_string()).collect();
    }
    for (perm, names) in deny {
        r.deny[perm.index()] = names.iter().map(|s| s.to_string()).collect();
    }
    r
}

fn summary(actions: &[Action]) -> Vec<String> {
    actions
        .iter()
        .map(|a| {
            format!(
                "{} {} {}",
                a.name,
                a.perm.cli_verb(),
                if a.allow { "allow" } else { "deny" }
            )
        })
        .collect()
}

#[test]
fn diff_only_changes_and_routing_before_local() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    let names: &[&str] = &["laptop", "desktop", "delta-golf.nord"];
    let r = rules(
        &[(Perm::Routing, names), (Perm::Local, names)],
        &[],
        PeerDefault::Keep,
    );
    let plan = peers::plan(&r, &l);
    assert!(plan.unresolved.is_empty());
    assert_eq!(
        summary(&plan.actions),
        vec![
            "laptop routing allow",
            "laptop local allow",
            // desktop already has both; delta-golf already routes.
            "delta-golf.nord local allow",
        ]
    );
    // Targets are public keys, which the CLI matches exactly.
    assert_eq!(
        plan.actions[0].target,
        "MACKEYAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
    );
    assert_eq!(
        plan.actions[0].args(),
        [
            "meshnet",
            "peer",
            "routing",
            "allow",
            "MACKEYAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="
        ]
    );
}

#[test]
fn diff_is_idempotent() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    let r = rules(
        &[(Perm::Routing, &["desktop"]), (Perm::Local, &["desktop"])],
        &[],
        PeerDefault::Keep,
    );
    assert!(peers::plan(&r, &l).actions.is_empty());
}

#[test]
fn allow_wins_over_deny() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    let r = rules(
        &[(Perm::Routing, &["laptop"])],
        &[(Perm::Routing, &["laptop", "desktop"])],
        PeerDefault::Keep,
    );
    assert_eq!(
        summary(&peers::plan(&r, &l).actions),
        vec!["desktop routing deny", "laptop routing allow"]
    );
}

#[test]
fn strict_mode_denies_unlisted_local_first() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list.txt"));
    let r = rules(
        &[(Perm::Routing, &["desktop"]), (Perm::Local, &["desktop"])],
        &[],
        PeerDefault::Deny,
    );
    assert_eq!(
        summary(&peers::plan(&r, &l).actions),
        vec![
            // laptop already denies both; nothing to do there.
            "delta-golf.nord routing deny",
            "friend-phone local deny",
            "friend-phone routing deny",
        ]
    );
}

#[test]
fn unresolved_peers_are_reported() {
    let l = peers::parse_peer_list(&fixture("6.0.2", "peer_list_empty.txt"));
    let r = rules(
        &[(Perm::Fileshare, &["later-device"])],
        &[],
        PeerDefault::Keep,
    );
    let plan = peers::plan(&r, &l);
    assert!(plan.actions.is_empty());
    assert_eq!(plan.unresolved, vec!["later-device"]);
}
