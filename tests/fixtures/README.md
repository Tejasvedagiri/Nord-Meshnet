# CLI output fixtures

One directory per NordVPN client version. Each file is the stdout of the
named command.

`6.0.2/` was reconstructed from the client's formatting code
(`cli/cli_mesh_peers.go` in NordSecurity/nordvpn-linux), not captured from a
live node. Replace it with real captures (`nordvpn meshnet peer list >
peer_list.txt`, etc.) when you have them. Keep the nicknames, but scrub
public keys and IPs.
