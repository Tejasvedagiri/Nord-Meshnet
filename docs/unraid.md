# Unraid

Use the Docker Compose Manager plugin and paste the bridge example, or a
template with these settings:

- **Extra Parameters:**
  `--cap-add=NET_ADMIN --cap-add=NET_RAW --device=/dev/net/tun --sysctl net.ipv6.conf.all.disable_ipv6=1 --sysctl net.ipv4.conf.all.arp_ignore=1 --sysctl net.ipv4.conf.all.rp_filter=2 --hostname=nordmesh-node`
- **Path:** `/mnt/user/appdata/nordmesh` → `/var/lib/nordvpn`
- **Variables:** `NORDVPN_TOKEN`, `NORDVPN_CONNECT`, `NORDVPN_MESHNET`, ...

Route another container through it with **Network Type: None** and Extra
Parameters `--net=container:nordmesh`, and publish that container's ports on
`nordmesh` instead.
