#!/usr/bin/env bash
# Assembles "Token Station.app" from the Swift front end and the Rust daemon.
#
# Xcode has to be installed; there is no Xcode project. Nothing here calls
# xcodebuild, and the whole package builds with `swift build` — but on the
# macOS 27 SDK, SwiftUI's property wrappers are macros, and the plugin that
# expands them (libSwiftUIMacros.dylib) ships inside Xcode's MacOSX platform
# directory rather than with the Command Line Tools. With only the Command Line
# Tools, `swift build` cannot expand `@State`. The script points DEVELOPER_DIR at
# an installed Xcode when the selected toolchain has no plugin, and says so when
# it cannot find one.
#
# The helper goes in Contents/MacOS beside the app binary, which is where
# Bundle.url(forAuxiliaryExecutable:) looks for it.
#
# Usage:
#   scripts/build-app.sh [--universal] [--zip]
#
# Environment:
#   CODESIGN_IDENTITY       Signing identity; "-" (ad hoc) by default.
#   TOKEN_STATION_HELPER_BIN Prebuilt daemon to bundle; skips cargo entirely.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
macos_dir="$(dirname "$here")"
repo_root="$(cd "$macos_dir/../.." && pwd)"
build_dir="$macos_dir/build"
app="$build_dir/Token Station.app"

universal=0
make_zip=0
for argument in "$@"; do
	case "$argument" in
	--universal) universal=1 ;;
	--zip) make_zip=1 ;;
	*)
		echo "build-app.sh: unknown option $argument" >&2
		exit 2
		;;
	esac
done

# --- toolchain -------------------------------------------------------------

swiftui_macro_plugin() {
	local platform
	platform="$(/usr/bin/xcrun --show-sdk-platform-path 2>/dev/null || true)"
	[[ -n "$platform" ]] || return 1
	local plugin="$platform/Developer/usr/lib/swift/host/plugins/libSwiftUIMacros.dylib"
	[[ -f "$plugin" ]] || return 1
}

if ! swiftui_macro_plugin; then
	if [[ -d /Applications/Xcode.app/Contents/Developer ]]; then
		export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
		echo "==> using Xcode's Swift plugins (the Command Line Tools ship no libSwiftUIMacros.dylib)"
	else
		echo "build-app.sh: this SDK expands SwiftUI's @State with libSwiftUIMacros.dylib," >&2
		echo "              which ships with Xcode. Install Xcode, or point DEVELOPER_DIR at" >&2
		echo "              a toolchain that has it." >&2
		exit 1
	fi
fi

version="$(
	awk '/^\[workspace\.package\]/ {inside=1; next}
	     /^\[/ {inside=0}
	     inside && /^version[[:space:]]*=/ {
	         gsub(/[^0-9A-Za-z.\-+]/, "", $3); print $3; exit
	     }' "$repo_root/Cargo.toml"
)"
if [[ -z "$version" ]]; then
	echo "build-app.sh: cannot read the version from Cargo.toml" >&2
	exit 1
fi
echo "==> Token Station $version"

# --- the Rust helper --------------------------------------------------------

helper_out="$build_dir/token-station"
rm -rf "$build_dir"
mkdir -p "$build_dir"

if [[ -n "${TOKEN_STATION_HELPER_BIN:-}" ]]; then
	echo "==> using the prebuilt helper at $TOKEN_STATION_HELPER_BIN"
	cp "$TOKEN_STATION_HELPER_BIN" "$helper_out"
elif [[ "$universal" -eq 1 ]]; then
	echo "==> cargo build (aarch64 + x86_64)"
	for target in aarch64-apple-darwin x86_64-apple-darwin; do
		if ! rustc --print target-list >/dev/null 2>&1 \
			|| ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
			echo "    adding the Rust target $target"
			rustup target add "$target"
		fi
	done
	(cd "$repo_root" && cargo build --release --locked -p token-station \
		--target aarch64-apple-darwin --target x86_64-apple-darwin)
	lipo -create -output "$helper_out" \
		"$repo_root/target/aarch64-apple-darwin/release/token-station" \
		"$repo_root/target/x86_64-apple-darwin/release/token-station"
else
	echo "==> cargo build (host)"
	(cd "$repo_root" && cargo build --release --locked -p token-station)
	cp "$repo_root/target/release/token-station" "$helper_out"
fi
chmod +x "$helper_out"

# --- the Swift app ----------------------------------------------------------

app_out="$build_dir/TokenStation"
if [[ "$universal" -eq 1 ]]; then
	echo "==> swift build (aarch64 + x86_64)"
	(cd "$macos_dir" && swift build -c release \
		--arch arm64 --arch x86_64 --product TokenStation)
	cp "$(cd "$macos_dir" && swift build -c release --arch arm64 --arch x86_64 \
		--product TokenStation --show-bin-path)/TokenStation" "$app_out"
else
	echo "==> swift build"
	(cd "$macos_dir" && swift build -c release --product TokenStation)
	cp "$(cd "$macos_dir" && swift build -c release --show-bin-path)/TokenStation" "$app_out"
fi

# --- the bundle -------------------------------------------------------------

echo "==> assembling $app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
mv "$app_out" "$app/Contents/MacOS/TokenStation"
mv "$helper_out" "$app/Contents/MacOS/token-station"

# CFBundleVersion has to be one to three dot-separated integers, so a semver
# pre-release or build suffix ("0.2.0-rc.1", "0.2.0+7") is cut away. The full
# string stays in CFBundleShortVersionString, which accepts it, unless it is not
# a plain dotted number either — then both carry the numeric part.
numeric_version="${version%%-*}"
numeric_version="${numeric_version%%+*}"
if [[ ! "$numeric_version" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
	echo "build-app.sh: cannot make a bundle version out of '$version'" >&2
	exit 1
fi
if [[ "$version" =~ ^[0-9]+(\.[0-9]+){0,2}$ ]]; then
	short_version="$version"
else
	short_version="$numeric_version"
	echo "    (CFBundleShortVersionString trimmed to $short_version)"
fi

sed -e "s/@SHORT_VERSION@/$short_version/g" -e "s/@BUNDLE_VERSION@/$numeric_version/g" \
	"$macos_dir/Resources/Info.plist" >"$app/Contents/Info.plist"

if [[ -f "$macos_dir/Resources/AppIcon.icns" ]]; then
	cp "$macos_dir/Resources/AppIcon.icns" "$app/Contents/Resources/AppIcon.icns"
else
	echo "    (no Resources/AppIcon.icns yet; the bundle goes out without one)"
fi

printf 'APPL????' >"$app/Contents/PkgInfo"

# --- signing ----------------------------------------------------------------

identity="${CODESIGN_IDENTITY:--}"
# Notarization rejects a signature without a secure timestamp, and an ad-hoc
# signature cannot carry one.
if [[ "$identity" == "-" ]]; then
	timestamp_flag="--timestamp=none"
else
	timestamp_flag="--timestamp"
fi
echo "==> codesign (identity: $identity, $timestamp_flag)"
# The nested helper is signed first: a signature over the bundle has to cover a
# binary that is already sealed, or the outer one is invalidated at once.
codesign --force "$timestamp_flag" --options runtime \
	--sign "$identity" "$app/Contents/MacOS/token-station"
codesign --force "$timestamp_flag" --options runtime \
	--sign "$identity" "$app"
codesign --verify --deep --strict --verbose=2 "$app"

echo "==> built $app"

if [[ "$make_zip" -eq 1 ]]; then
	# The name CI and the release job upload; it does not carry the version.
	archive="$build_dir/TokenStation-macOS.zip"
	rm -f "$archive"
	ditto -c -k --keepParent "$app" "$archive"
	echo "==> zipped $archive"
fi
