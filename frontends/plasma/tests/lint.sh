#!/usr/bin/env bash
# qmllint over every QML file of the applet.
#
#   nix develop -c frontends/plasma/tests/lint.sh
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "${here}/qml-env.sh"

applet="${here}/../dev.soldunov.tokenstation"
import_args=()
while IFS= read -r path; do
    import_args+=(-I "${path}")
done < <(tr ':' '\n' <<< "${TS_QML_IMPORT_PATH}")

# The applet's own components resolve from contents/ui; the config page lives
# there too so that it can reuse DaemonClient.
import_args+=(-I "${applet}/contents/ui")

mapfile -t files < <(find "${applet}" "${here}" -name '*.qml' | sort)

qmllint \
    "${import_args[@]}" \
    "${files[@]}"
