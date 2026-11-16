use std::fs;
use std::path::PathBuf;

use nordmesh::sysctl;

fn fake_proc(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("nordmesh-proc-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("net/ipv6/conf/all")).unwrap();
    fs::create_dir_all(root.join("net/ipv4/conf/all")).unwrap();
    fs::write(root.join("net/ipv6/conf/all/disable_ipv6"), "1\n").unwrap();
    fs::write(root.join("net/ipv4/conf/all/arp_ignore"), "1\n").unwrap();
    root
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn applied_when_values_match() {
    let root = fake_proc("match");
    assert!(sysctl::already_applied(
        &args(&[
            "-w",
            "net.ipv6.conf.all.disable_ipv6=1",
            "net.ipv4.conf.all.arp_ignore=1"
        ]),
        &root
    ));
}

#[test]
fn not_applied_when_a_value_differs() {
    let root = fake_proc("differs");
    assert!(!sysctl::already_applied(
        &args(&["-w", "net.ipv6.conf.all.disable_ipv6=0"]),
        &root
    ));
}

#[test]
fn not_applied_when_key_missing_or_nothing_to_compare() {
    let root = fake_proc("missing");
    assert!(!sysctl::already_applied(
        &args(&["-w", "net.core.nope=1"]),
        &root
    ));
    assert!(!sysctl::already_applied(
        &args(&["-n", "net.ipv4.conf.all.arp_ignore"]),
        &root
    ));
    assert!(!sysctl::already_applied(&args(&["..=1"]), &root));
}

#[cfg(unix)]
#[test]
fn shim_masks_failure_only_when_applied() {
    let root = fake_proc("shim");
    // /bin/false stands in for a real sysctl that can't write /proc/sys.
    assert_eq!(
        sysctl::shim(
            &args(&["-w", "net.ipv4.conf.all.arp_ignore=1"]),
            "/usr/bin/false",
            &root
        ),
        0
    );
    assert_ne!(
        sysctl::shim(
            &args(&["-w", "net.ipv4.conf.all.arp_ignore=2"]),
            "/usr/bin/false",
            &root
        ),
        0
    );
}
