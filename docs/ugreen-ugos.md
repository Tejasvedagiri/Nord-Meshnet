# UGREEN NAS (UGOS Pro)

Tested target: DXP-series on an Intel N100, UGOS Pro, Docker app.

1. **Docker → Project → Create.** Paste a compose file (the macvlan example
   if you want the container to have its own LAN IP, otherwise the bridge
   example). The project form is needed: the container form can't set
   `sysctls`.
2. Put the token in the project's `.env` (`NORDVPN_TOKEN=...`) rather than in
   the YAML.
3. Volumes: use a path on your data pool, e.g.
   `/volume1/docker/nordmesh/nordvpn:/var/lib/nordvpn`.
4. `/dev/net/tun` exists on UGOS Pro; no module loading is needed.

## macvlan notes

- `parent:` is usually `eth0` on single-NIC models; on bonded or multi-NIC
  setups check `ip link` over SSH (e.g. `bond0`).
- The NAS host cannot reach a macvlan container directly (a Linux macvlan
  limitation). Reach it from another LAN device, or add a macvlan shim
  interface on the host.
- Pick an `ip_range` outside your router's DHCP pool.
- Allowlist the LAN subnet, or the kill switch will cut the container off
  from it.
