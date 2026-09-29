#!/usr/bin/env bash
# Regenerate Resources/AppIcon.icns from Resources/AppIcon.svg.
#
# Runs on macOS (iconutil ships with the OS) and needs rsvg-convert, from
# `brew install librsvg` or `nix shell nixpkgs#librsvg`. The .icns is committed,
# so building the app never needs either tool.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
svg="$here/Resources/AppIcon.svg"
icns="$here/Resources/AppIcon.icns"

command -v rsvg-convert >/dev/null || { echo "rsvg-convert not found (brew install librsvg)" >&2; exit 1; }
command -v iconutil >/dev/null || { echo "iconutil not found; run this on macOS" >&2; exit 1; }

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
iconset="$work/AppIcon.iconset"
mkdir "$iconset"

# Every size macOS asks an app icon for, at 1x and 2x.
for size in 16 32 128 256 512; do
  rsvg-convert -w "$size" -h "$size" "$svg" -o "$iconset/icon_${size}x${size}.png"
  double=$((size * 2))
  rsvg-convert -w "$double" -h "$double" "$svg" -o "$iconset/icon_${size}x${size}@2x.png"
done

iconutil --convert icns --output "$icns" "$iconset"
echo "wrote $icns"
