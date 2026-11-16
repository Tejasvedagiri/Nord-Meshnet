# Troubleshooting

Start with `docker logs nordmesh`. Every action and failure is logged with a
`[nordmesh]` prefix; set `NORDMESH_LOG_LEVEL=debug` to see every `nordvpn`
call and its output (the token is redacted).

## The container exits immediately with "invalid configuration"

A variable has a bad value; the message names it. Check the config without
starting anything:

```sh
docker run --rm --env-file .env ghcr.io/tejasvedagiri/nordmesh:1 check-config
```

## `connect` fails with a sysctl / permission error

The `sysctls:` block is missing or a value differs. Both lines are required:

```yaml
sysctls:
  - net.ipv6.conf.all.disable_ipv6=1
  - net.ipv4.conf.all.arp_ignore=1
  - net.ipv4.conf.all.rp_filter=2
```

Some NAS UIs can't set sysctls. Use a compose/project file instead of the
container form.

## Login fails or times out

- Check the token (Nord Account → NordVPN → Manual setup). Tokens can expire.
- `dial tcp ... i/o timeout`: the container has no egress. A kill switch left
  on from a previous run is turned off at startup, so look at the host
  network, DNS, or a macvlan misconfiguration instead.

## The LAN can't reach the container (web UIs time out)

The kill switch blocks anything not allowlisted. Add your LAN:
`NORDVPN_ALLOWLIST_SUBNETS=192.168.1.0/24`, and published ports:
`NORDVPN_ALLOWLIST_PORTS=8080`.

## "peer X not found in the Meshnet peer list yet"

- Right after Meshnet starts, the peer list can be empty for a few seconds.
  This warning repeats only if the peer stays missing.
- Check the name against `docker exec nordmesh nordvpn meshnet peer list`.
  Nicknames, hostnames (with or without `.nord`) and Meshnet IPs all work.
- The peer must be on the same Nord account or have accepted an invite.

## "matches more than one peer"

A nickname equals another peer's hostname. Use the hostname or IP.

## Peers show the wrong permissions in the phone app

The app's *Manage devices* screen shows what each peer may do *on the
phone*. This node's permissions are visible with
`docker exec nordmesh nordvpn meshnet peer list` (`Allow ...` lines are this
node's grants; `Allows ...` lines are what the peer grants this node).

## Every recreate shows up as a new device

Set a fixed `hostname:` and persist `/var/lib/nordvpn`. Remove stale devices
in the app (Meshnet → Manage devices); the account limit is 10.

## The container is unhealthy

`docker inspect --format '{{json .State.Health}}' nordmesh` shows the
healthcheck output, e.g. `unhealthy: VPN not connected`. The reconcile loop
keeps retrying; the logs say why the last attempt failed.
