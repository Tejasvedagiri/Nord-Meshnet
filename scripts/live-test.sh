#!/usr/bin/env bash
# Live test against the real NordVPN service. Needs NORDVPN_TOKEN; optional
# PEER (a Meshnet peer on the test account to grant fileshare to) and
# PROTOCOL. Always logs out and deregisters on exit so CI runs never eat into
# the account's 10-device limit.
set -euo pipefail
image="${1:?usage: $0 <image>}"
: "${NORDVPN_TOKEN:?NORDVPN_TOKEN is required}"
protocol="${PROTOCOL:-nordlynx}"
name="nordmesh-live-$$"

peer_env=()
if [ -n "${PEER:-}" ]; then
  peer_env=(-e "NORDVPN_ALLOW_PEER_FILESHARE=${PEER}")
fi

cleanup() {
  echo "--- logs"; docker logs "$name" 2>&1 | tail -100 || true
  docker exec "$name" nordvpn set killswitch off >/dev/null 2>&1 || true
  docker exec "$name" nordvpn set meshnet off >/dev/null 2>&1 || true
  # --persist-token keeps the token valid for the next run.
  docker exec "$name" nordvpn logout --persist-token >/dev/null 2>&1 || true
  docker rm -f "$name" >/dev/null 2>&1 || true
}
trap cleanup EXIT

docker run -d --name "$name" --hostname "ci-${GITHUB_RUN_ID:-local}" \
  --cap-add NET_ADMIN --cap-add NET_RAW --device /dev/net/tun \
  --sysctl net.ipv6.conf.all.disable_ipv6=1 --sysctl net.ipv4.conf.all.arp_ignore=1 --sysctl net.ipv4.conf.all.rp_filter=2 \
  -e NORDVPN_TOKEN -e NORDVPN_CONNECT= -e "NORDVPN_PROTOCOL=${protocol}" \
  -e NORDVPN_MESHNET=on -e "NORDVPN_NICKNAME=ci-${GITHUB_RUN_ID:-local}-mesh" \
  -e NORDMESH_RECONCILE_INTERVAL=10 \
  ${peer_env[@]+"${peer_env[@]}"} \
  "$image"

echo "--- waiting for healthy"
for i in $(seq 1 60); do
  if docker exec "$name" /usr/local/bin/nordmesh-init healthcheck >/dev/null 2>&1; then break; fi
  [ "$i" = 60 ] && { echo "never became healthy"; exit 1; }
  sleep 5
done

echo "--- status"
docker exec "$name" nordvpn status | tee /tmp/status
grep -q "Status: Connected" /tmp/status
docker exec "$name" nordvpn settings | grep -E "Kill Switch: enabled"
docker exec "$name" nordvpn settings | grep -E "Meshnet: enabled"

echo "--- a sidecar sharing the namespace egresses through the tunnel"
docker run --rm --network "container:${name}" curlimages/curl:latest -fsS --max-time 15 https://ipinfo.io/json; echo

if [ -n "${PEER:-}" ]; then
  echo "--- peer permission applied"
  for i in $(seq 1 12); do
    if docker exec "$name" nordvpn meshnet peer list | grep -A20 -iE "(Nickname|Hostname|IP): ${PEER}$" \
         | grep -q "Allow Sending Files: enabled"; then break; fi
    [ "$i" = 12 ] && { echo "fileshare permission for ${PEER} not applied"; exit 1; }
    sleep 5
  done
fi

echo "--- kill switch blocks egress after a forced disconnect"
docker exec "$name" nordvpn disconnect >/dev/null
if docker exec "$name" timeout 5 bash -c 'exec 3<>/dev/tcp/1.1.1.1/443' 2>/dev/null; then
  echo "egress succeeded while disconnected: kill switch is not blocking"; exit 1
fi
echo "blocked, as expected"

echo "--- reconcile loop reconnects"
for i in $(seq 1 12); do
  if docker exec "$name" nordvpn status | grep -q "Status: Connected"; then echo ok; exit 0; fi
  sleep 5
done
echo "did not reconnect"; exit 1
