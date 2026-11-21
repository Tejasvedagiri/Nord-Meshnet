#!/usr/bin/env bash
# Smoke test: every `nordvpn` verb nordmesh-init calls must still exist in the
# bundled client. Catches CLI changes (e.g. 6.0 removing `set technology`)
# before they reach users. Usage: scripts/cli-check.sh <image>
set -euo pipefail
image="${1:?usage: $0 <image>}"

verbs=(
  "status" "settings" "account" "login" "connect"
  "set analytics" "set killswitch" "set protocol" "set dns" "set meshnet"
  "allowlist remove all" "allowlist add subnet" "allowlist add port" "allowlist add ports"
  "meshnet set nickname" "meshnet peer list"
  "meshnet peer incoming allow" "meshnet peer incoming deny"
  "meshnet peer routing allow" "meshnet peer routing deny"
  "meshnet peer local allow" "meshnet peer local deny"
  "meshnet peer fileshare allow" "meshnet peer fileshare deny"
)

fail=0
for v in "${verbs[@]}"; do
  # shellcheck disable=SC2086
  if out=$(docker run --rm --entrypoint /usr/bin/nordvpn "$image" $v --help 2>&1) \
     && ! grep -qiE 'no help topic|command not found|unknown command' <<<"$out"; then
    echo "ok    nordvpn $v"
  else
    echo "FAIL  nordvpn $v"; while IFS= read -r line; do echo "      $line"; done <<<"$out"; fail=1
  fi
done

echo "--- init binary"
docker run --rm --entrypoint /usr/local/bin/nordmesh-init "$image" version
docker run --rm --entrypoint /usr/local/bin/nordmesh-init "$image" check-config >/dev/null
echo "--- sysctl shim is in place"
docker run --rm --entrypoint /bin/sh "$image" -c \
  'test "$(readlink -f /usr/sbin/sysctl)" = /usr/local/bin/nordmesh-init && test -x /usr/sbin/sysctl.real'
exit "$fail"
