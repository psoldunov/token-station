#!/usr/bin/env bash
# Open the applet in plasmoidviewer against a fixture-serving daemon, on a
# private session bus, so it runs beside the daemon you already have installed.
#
#   nix develop -c frontends/plasma/tests/preview.sh [fixture.json]
#
# The installed daemon owns dev.soldunov.TokenStation on the real session bus,
# and a second daemon refuses to start there. This script re-runs itself under
# dbus-run-session with a bus that has no service directories, so nothing on it
# can activate the installed daemon either. Only the fixture daemon runs: no
# credentials, no network, no writes to your config.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

if [[ -z "${TS_PREVIEW_BUS_DIR:-}" ]]; then
    bus_dir=$(mktemp -d)
    trap 'rm -rf "${bus_dir}"' EXIT
    cat > "${bus_dir}/bus.conf" <<EOF
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-BUS Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=${bus_dir}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
EOF
    status=0
    TS_PREVIEW_BUS_DIR="${bus_dir}" \
        dbus-run-session --config-file="${bus_dir}/bus.conf" -- "${here}/preview.sh" "$@" || status=$?
    exit "${status}"
fi

source "${here}/qml-env.sh"

repo=$(cd "${here}/../../.." && pwd)
fixture=${1:-${repo}/data/fixtures/snapshot-near-limit.json}
applet="${here}/../dev.soldunov.tokenstation"

cd "${repo}"
# Build in the foreground so a compile error shows up here, not in a log.
cargo build -q -p token-station
cargo run -q -p token-station -- daemon --fixture "${fixture}" &
daemon_pid=$!
trap 'kill "${daemon_pid}" 2>/dev/null; wait "${daemon_pid}" 2>/dev/null' EXIT

until busctl --user status dev.soldunov.TokenStation > /dev/null 2>&1; do
    if ! kill -0 "${daemon_pid}" 2>/dev/null; then
        echo "the fixture daemon exited before it claimed the bus name" >&2
        exit 1
    fi
    sleep 0.2
done

echo "serving ${fixture##*/} on a private bus; close the viewer to stop"
plasmoidviewer -a "${applet}"
