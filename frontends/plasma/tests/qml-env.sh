#!/usr/bin/env bash
# Shared QML import path for the lint and render scripts.
#
# The Plasma QML modules are not all reachable from a single prefix on NixOS:
# plasmoidviewer's wrapper carries the full set, and plasma-workspace (which
# provides org.kde.plasma.workspace.dbus) is not among them. Derive both here
# rather than hard-coding store paths that change on every rebuild.
#
# Source this from inside `nix develop`, where plasmoidviewer is on PATH.
#
# Deliberately sets no shell options: this file is sourced, and turning on
# `errexit` here would turn it on in the caller too. live.sh runs commands that
# are expected to fail (it reports on them instead of dying).

viewer=$(command -v plasmoidviewer || true)
if [[ -z "${viewer}" ]]; then
    echo "plasmoidviewer not found; run this inside 'nix develop'." >&2
    exit 1
fi

# Every QML module directory the plasma-sdk wrapper injects.
mapfile -t viewer_paths < <(strings "${viewer}" | grep -E '^/.*/lib/qt-6/qml$' | sort -u)

workspace_qml=$(ls -d /nix/store/*-plasma-workspace-*/lib/qt-6/qml 2>/dev/null | head -n 1 || true)

paths=("${viewer_paths[@]}")
[[ -n "${workspace_qml}" ]] && paths+=("${workspace_qml}")
[[ -d /run/current-system/sw/lib/qt-6/qml ]] && paths+=(/run/current-system/sw/lib/qt-6/qml)

TS_QML_IMPORT_PATH=$(IFS=:; echo "${paths[*]}")
export TS_QML_IMPORT_PATH
export QML2_IMPORT_PATH="${TS_QML_IMPORT_PATH}"
export QML_IMPORT_PATH="${TS_QML_IMPORT_PATH}"
