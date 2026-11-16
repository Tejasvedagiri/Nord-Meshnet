# nordmesh

A Docker image that runs the official NordVPN Linux client with the **VPN and
Meshnet configured entirely from environment variables**, including **Meshnet
peer permissions**. Other containers can route their traffic through it with
`network_mode: service:nordmesh`. Nothing to `docker exec` after deploying.

- Current official client (pinned, bumped through reviewed PRs)
- VPN connect, protocol, DNS, allowlist, and a kill switch that is **only
  enabled after a successful connect**
- Meshnet with nickname and declarative peer permissions (routing, local
  network, file sharing, remote access), re-applied every minute
- Runs unprivileged (`NET_ADMIN`, `NET_RAW`, `/dev/net/tun`), never needs the
  Docker socket
- `linux/amd64` and `linux/arm64`
- Built-in healthcheck that matches what you configured

## Quick start

```yaml
services:
  nordmesh:
    image: ghcr.io/tejasvedagiri/nordmesh:1
    container_name: nordmesh
    hostname: nordmesh-node             # keep stable: Meshnet identity depends on it
    cap_add: [NET_ADMIN, NET_RAW]
    devices: [/dev/net/tun:/dev/net/tun]
    security_opt: [no-new-privileges:true]
    sysctls:
      - net.ipv6.conf.all.disable_ipv6=1
      - net.ipv4.conf.all.arp_ignore=1
      - net.ipv4.conf.all.rp_filter=2
    volumes:
      - ./nordvpn:/var/lib/nordvpn        # login + Meshnet identity
    environment:
      - NORDVPN_TOKEN=${NORDVPN_TOKEN}
      # - NORDVPN_CONNECT=                # empty: recommended server; e.g. Germany
      # - NORDVPN_ALLOWLIST_SUBNETS=      # e.g. 192.168.1.0/24
      - NORDVPN_MESHNET=on
      # - NORDVPN_NICKNAME=               # e.g. home-node (must differ from hostname)
      # - NORDVPN_ALLOW_PEER_ROUTING=     # e.g. tejas.nord,laptop
      # - NORDVPN_ALLOW_PEER_LOCAL=       # e.g. tejas.nord
    restart: unless-stopped
```

Get a token from Nord Account → NordVPN → Manual setup → Access token.
Uncomment and fill in what you need. Peers can be named by nickname, Meshnet
hostname (e.g. `tejas.nord`) or Meshnet IP.

More examples: [bridge](examples/docker-compose.yml),
[macvlan](examples/docker-compose.macvlan.yml),
[Docker secrets](examples/docker-compose.secrets.yml).

### Required pieces, and why

| Setting | Why |
|---|---|
| `cap_add: [NET_ADMIN, NET_RAW]`, `/dev/net/tun` | Creating the tunnel and firewall rules. No `privileged`. |
| `sysctls` (all three lines) | nordvpnd writes these on connect. `/proc/sys` is read-only without `privileged`, so they're preset and the image's `sysctl` shim reports success when the value already matches. |
| `hostname` | Meshnet identity. A changing hostname registers a new device on every recreate (the account limit is 10). |
| `/var/lib/nordvpn` volume | Keeps the login and Meshnet registration across recreates. |

## Configuration

Lists are comma-separated; whitespace around entries is ignored. Invalid
values stop the container at startup with a message naming the variable.
Run `docker run --rm --env-file .env ghcr.io/tejasvedagiri/nordmesh:1 check-config` to validate
without starting anything.

### Auth

| Variable | Default | Description |
|---|---|---|
| `NORDVPN_TOKEN` | – | Access token. Required unless the volume already holds a login. |
| `NORDVPN_TOKEN_FILE` | – | File holding the token (Docker secrets). Takes precedence. |

### VPN

| Variable | Default | Description |
|---|---|---|
| `NORDVPN_CONNECT` | unset | Unset: no VPN tunnel (Meshnet only). Empty: recommended server. Otherwise passed to `nordvpn connect`, e.g. `Germany`, `Germany Berlin`, `--group P2P Netherlands`. |
| `NORDVPN_PROTOCOL` | `nordlynx` | `nordlynx`, `openvpn_udp`, `openvpn_tcp`, `nordwhisper`. |
| `NORDVPN_KILLSWITCH` | `on` if `CONNECT` is set | Enabled only after the first successful connect. |
| `NORDVPN_ALLOWLIST_SUBNETS` | – | e.g. `192.168.1.0/24`. Keeps the LAN reachable through the kill switch. |
| `NORDVPN_ALLOWLIST_PORTS` | – | e.g. `8080,6881-6889`. |
| `NORDVPN_DNS` | – | Up to 3 DNS servers. Unset resets to NordVPN's DNS. |
| `NORDVPN_SET` | – | `;`-separated `nordvpn set` arguments, e.g. `post-quantum on; lan-discovery off`. Keys that have their own variable (`killswitch`, `meshnet`, `protocol`, `dns`, `analytics`) are refused. |

### Meshnet

| Variable | Default | Description |
|---|---|---|
| `NORDVPN_MESHNET` | `off` | `on` to enable. |
| `NORDVPN_NICKNAME` | – | This node's Meshnet nickname. |
| `NORDVPN_ALLOW_PEER_ROUTING` | – | Peers that may use this node as an exit node. |
| `NORDVPN_ALLOW_PEER_LOCAL` | – | Peers that may reach this node's LAN. Each must also be in `ALLOW_PEER_ROUTING`. |
| `NORDVPN_ALLOW_PEER_FILESHARE` | – | Peers that may send files. |
| `NORDVPN_ALLOW_PEER_REMOTE` | – | Peers allowed remote access (incoming traffic) to this node. |
| `NORDVPN_DENY_PEER_{ROUTING,LOCAL,FILESHARE,REMOTE}` | – | The opposite. |
| `NORDVPN_PEER_DEFAULT` | `keep` | `keep`: leave unlisted peers alone. `deny`: deny routing and local access to every peer not in the matching ALLOW list. |

Peers can be named by nickname, Meshnet hostname (`xxx.nord`, or without
`.nord`) or Meshnet IP. DENY lists apply first, then ALLOW lists, so ALLOW
wins. Only differences are sent to the client, so the loop is quiet once
everything matches. Peers added to your account later are picked up on the
next pass.

> **Permissions are per node.** The app's *Manage devices* screen shows what
> each peer may do *on the phone*, not on this node. This node's permissions
> can only be set on this node, which is what these variables do.

### Runtime

| Variable | Default | Description |
|---|---|---|
| `NORDMESH_RECONCILE_INTERVAL` | `60` | Seconds between reconcile passes (min 10). |
| `NORDMESH_LOG_LEVEL` | `info` | `debug`, `info`, `warn`, `error`. Also sets nordvpnd's level. |
| `NORDMESH_ANALYTICS` | `off` | Client analytics, opt-in only. |

## What happens at startup

1. Validate the environment; print the effective config with the token redacted.
2. Start `nordvpnd` and wait for its socket.
3. `set analytics off` (the first-run consent prompt otherwise blocks the CLI).
4. Log in if needed (retried 5× with backoff).
5. **Force the kill switch off**: a persisted volume may hold `on`, and a kill
   switch without a tunnel blocks login and Meshnet registration too.
6. Apply protocol, DNS, allowlist, then `NORDVPN_SET`.
7. Connect, with retries.
8. **Only after `Status: Connected`:** enable the kill switch.
9. Enable Meshnet, set the nickname, apply peer permissions (waiting for peers
   to sync).
10. Every `NORDMESH_RECONCILE_INTERVAL`: reconnect if needed, re-assert the
    nickname, re-apply peer permissions.

Steps 1–6 are fatal: the container exits non-zero and `restart:` tries again.
From step 7 on, failures are logged and retried.

The healthcheck is unhealthy when `CONNECT` is set and the VPN is down, or
`MESHNET=on` and Meshnet is off.

## Debugging

The image keeps a shell, so the usual commands work:

```sh
docker exec nordmesh nordvpn status
docker exec nordmesh nordvpn meshnet peer list
docker exec nordmesh nordvpn settings
docker logs nordmesh
```

See [troubleshooting](docs/troubleshooting.md), and the platform notes for
[UGREEN UGOS](docs/ugreen-ugos.md), [Synology](docs/synology.md) and
[Unraid](docs/unraid.md).

## Security

- No privileged mode and no Docker socket.
- Environment values are split into argv and never pass through a shell.
  `NORDVPN_SET` entries only ever reach `nordvpn set`; peer names are
  validated.
- The token is redacted from all logs and login output is never printed.
  It is passed as a `nordvpn login --token` argument, so it is briefly visible
  to `ps` *inside* the container.
- The NordVPN apt repo is GPG-verified; base images and Actions are pinned by
  digest/SHA; release images carry SLSA provenance and an SBOM:

  ```sh
  gh attestation verify oci://ghcr.io/tejasvedagiri/nordmesh:1 --owner tejasvedagiri
  ```

Report vulnerabilities privately; see [SECURITY.md](SECURITY.md).

## Building

```sh
docker build -t nordmesh .
docker build --build-arg NORDVPN_VERSION=6.0.2 -t nordmesh .
cargo test     # unit tests, no Docker or NordVPN account needed
```

`nordmesh-init` is a dependency-free Rust binary (std only, statically
linked). It is PID 1, the healthcheck, and the `sysctl` shim (chosen by
`argv[0]`).

## Credits

- [drumandbytes/nordvpn](https://github.com/drumandbytes/nordvpn) (MIT): the
  sysctl shim, startup quirks and live-test design.
- [MattsTechInfo/Meshnet](https://github.com/MattsTechInfo/Meshnet) (MIT):
  peer-permission variable naming.

Not affiliated with Nord Security. NordVPN is a trademark of Nord Security.
The NordVPN client inside the image is distributed under its own terms.

## License

MIT, for the code in this repository.
