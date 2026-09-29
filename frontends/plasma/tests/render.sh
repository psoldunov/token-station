#!/usr/bin/env bash
# Render every fixture through the applet's representations, offscreen, in both
# the light and the dark Breeze colour scheme, into /tmp/ts-plasma-shots.
#
#   nix develop -c frontends/plasma/tests/render.sh
#
# TS_SHOT_SCALE (default 1) renders at that device pixel ratio, e.g. 4 for the
# README's tray meter picture.
#
# Nothing here touches the running Plasma session: the QML runtime gets a
# throw-away XDG_CONFIG_HOME holding only a colour scheme and a plasmarc.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
source "${here}/qml-env.sh"

repo=$(cd "${here}/../../.." && pwd)
ui="${here}/../dev.soldunov.tokenstation/contents/ui"
out="${TS_SHOT_DIR:-/tmp/ts-plasma-shots}"
scratch="${out}/.config"

mkdir -p "${out}"
rm -rf "${scratch}"

mapfile -t fixtures < <(find "${repo}/data/fixtures" -name 'snapshot-*.json' | sort)
if [[ ${#fixtures[@]} -eq 0 ]]; then
    echo "no fixtures under ${repo}/data/fixtures" >&2
    exit 1
fi

schemes_dir=""
for candidate in /run/current-system/sw/share/color-schemes /usr/share/color-schemes; do
    [[ -d "${candidate}" ]] && schemes_dir="${candidate}" && break
done

render_one() {
    local variant=$1 scheme_file=$2 plasma_theme=$3
    local config="${scratch}/${variant}"
    mkdir -p "${config}"

    if [[ -n "${scheme_file}" && -f "${scheme_file}" ]]; then
        cp "${scheme_file}" "${config}/kdeglobals"
    fi
    printf '[Theme]\nname=%s\n' "${plasma_theme}" > "${config}/plasmarc"

    echo "== ${variant} =="
    XDG_CONFIG_HOME="${config}" \
    QT_QPA_PLATFORM=offscreen \
    QT_QUICK_BACKEND=software \
    QT_ASSUME_STDERR_HAS_CONSOLE=1 \
    QT_SCALE_FACTOR="${TS_SHOT_SCALE:-1}" \
    QML_XHR_ALLOW_FILE_READ=1 \
        qml "${here}/render.qml" -- "${ui}" "${out}" "${variant}" "${fixtures[@]}"
}

render_one light "${schemes_dir:+${schemes_dir}/BreezeLight.colors}" default
render_one dark "${schemes_dir:+${schemes_dir}/BreezeDark.colors}" breeze-dark

echo
echo "PNGs in ${out}:"
ls -1 "${out}"/*.png
