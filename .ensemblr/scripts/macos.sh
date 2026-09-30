#!/usr/bin/env bash
# macOS counterparts for the Linux-only Nix development shell.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
macos="$repo/frontends/macos"

preview_pid=""
preview_dir=""
analysis_dir=""

cleanup() {
    if [[ -n "$preview_pid" ]]; then
        kill "$preview_pid" 2>/dev/null || :
        wait "$preview_pid" 2>/dev/null || :
    fi
    [[ -z "$preview_dir" ]] || rm -rf "$preview_dir"
    [[ -z "$analysis_dir" ]] || rm -rf "$analysis_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

require_tool() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "macos.sh: $1 is required" >&2
        exit 127
    fi
}

has_swiftui_plugin() {
    local platform
    platform="$(/usr/bin/xcrun --show-sdk-platform-path 2>/dev/null)" || return 1
    [[ -f "$platform/Developer/usr/lib/swift/host/plugins/libSwiftUIMacros.dylib" ]]
}

use_xcode() {
    if has_swiftui_plugin; then
        return
    fi
    if [[ -d /Applications/Xcode.app/Contents/Developer ]]; then
        export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
        if has_swiftui_plugin; then
            echo "==> using Xcode's Swift plugins"
            return
        fi
    fi
    echo "macos.sh: full Xcode is required; select it with xcode-select or DEVELOPER_DIR" >&2
    exit 1
}

setup_macos() {
    require_tool cargo
    require_tool swift
    use_xcode
    (cd "$repo" && cargo fetch --locked)
    (cd "$macos" && swift package resolve)
}

preview_macos() {
    require_tool cargo
    require_tool swift
    use_xcode

    local fixture="${1:-$repo/data/fixtures/snapshot-near-limit.json}"
    preview_dir="$(mktemp -d "${TMPDIR:-/tmp}/token-station.XXXXXX")"
    export TOKEN_STATION_SOCKET="$preview_dir/daemon.sock"

    (cd "$repo" && cargo build --locked -q -p token-station)
    "$repo/target/debug/token-station" daemon --fixture "$fixture" &
    preview_pid=$!

    until [[ -S "$TOKEN_STATION_SOCKET" ]]; do
        if ! kill -0 "$preview_pid" 2>/dev/null; then
            if wait "$preview_pid"; then
                return 1
            else
                return $?
            fi
        fi
        sleep 0.2
    done

    echo "==> serving ${fixture##*/} on a private socket; quit the app to stop"
    (cd "$macos" && swift run TokenStation)
}

test_macos() {
    require_tool cargo
    require_tool swift
    use_xcode
    (cd "$repo" && cargo test --workspace --locked)
    (cd "$macos" && swift test)
}

lint_macos() {
    require_tool cargo

    (cd "$repo" && cargo fmt --all --check)
    (cd "$repo" && cargo clippy --workspace --all-targets --locked -- --deny warnings)
    lint_swift
}

# The Swift half on its own, for a review that only needs the front end's gates:
# it touches nothing outside frontends/macos, so a copy of that directory and
# this script is enough to run it on another Mac.
lint_swift() {
    require_tool swift
    require_tool swiftlint
    require_tool periphery
    use_xcode

    analysis_dir="$(mktemp -d "${TMPDIR:-/tmp}/token-station-analyze.XXXXXX")"
    (
        cd "$macos"
        swiftlint lint --strict
        swift build --build-tests --build-system native --scratch-path "$analysis_dir/build" -v \
            > "$analysis_dir/swiftbuild.log"
        swiftlint analyze --strict --compiler-log-path "$analysis_dir/swiftbuild.log"
        periphery scan --strict
    )
    rm -rf "$analysis_dir"
    analysis_dir=""
}

build_macos() {
    use_xcode
    (cd "$repo" && frontends/macos/scripts/build-app.sh "$@")
}

case "${1:-}" in
setup)
    setup_macos
    ;;
preview)
    shift
    preview_macos "$@"
    ;;
test)
    test_macos
    ;;
lint)
    lint_macos
    ;;
swift-lint)
    lint_swift
    ;;
build)
    shift
    build_macos "$@"
    ;;
check)
    test_macos
    lint_macos
    build_macos --zip
    ;;
*)
    echo "usage: $0 setup|preview [fixture]|test|lint|swift-lint|build [--universal] [--zip]|check" >&2
    exit 2
    ;;
esac
