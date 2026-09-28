#!/usr/bin/env bash
# Live check against a fixture-serving daemon:
#   1. DaemonClient round-trips Snapshot, GetSettings and GetHistory (dbus-smoke.qml)
#   2. plasmoidviewer loads the whole package offscreen; its stderr must be clean
#
#   nix develop -c frontends/plasma/tests/live.sh [fixture.json]
#
# Only ever runs the daemon with --fixture, and only renders offscreen, so the
# user's Plasma session and real credentials are untouched.
set -uo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "${here}/qml-env.sh"

repo=$(cd "${here}/../../.." && pwd)
fixture=${1:-${repo}/data/fixtures/snapshot-ok.json}
applet="${here}/../dev.soldunov.tokenstation"

cd "${repo}"
cargo run -q -p token-station -- daemon --fixture "${fixture}" > /tmp/ts-daemon.log 2>&1 &
daemon_pid=$!
trap 'kill "${daemon_pid}" 2>/dev/null; wait "${daemon_pid}" 2>/dev/null' EXIT

# Wait for the bus name rather than guessing how long the build takes.
for _ in $(seq 1 120); do
    busctl --user status dev.soldunov.TokenStation > /dev/null 2>&1 && break
    sleep 1
done

status=0

echo "== DaemonClient round trip (${fixture##*/}) =="
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_ASSUME_STDERR_HAS_CONSOLE=1 \
    qml "${here}/dbus-smoke.qml" 2>&1 | grep -vE '^(qt\.|kf\.)' || status=1

echo
echo "== plasmoidviewer =="
QT_QPA_PLATFORM=offscreen QT_QUICK_BACKEND=software QT_ASSUME_STDERR_HAS_CONSOLE=1 \
    timeout 25 plasmoidviewer -a "${applet}" > /tmp/ts-viewer.log 2>&1

# Anything from our own package is a real problem; the desktop containment's own
# noise under plasmoidviewer is not.
if grep -F "dev.soldunov.tokenstation" /tmp/ts-viewer.log; then
    echo "QML warnings from the applet (see /tmp/ts-viewer.log)"
    status=1
else
    echo "no QML warnings from the applet"
fi

exit "${status}"
