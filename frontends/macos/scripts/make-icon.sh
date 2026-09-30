#!/usr/bin/env bash
# Regenerate Resources/Assets.car and Resources/AppIcon.icns from Resources/AppIcon.icon.
#
# AppIcon.icon is an Icon Composer document: a background fill and two groups of
# SVG layers (the tubes, and the Claude and Codex levels) that macOS 26 and later
# draw as Liquid Glass, in the default, dark, clear and tinted appearances. Open it
# in Icon Composer (Xcode > Open Developer Tool) or edit it by hand. actool
# compiles it the way Xcode does for an app target:
#
#   Assets.car    The layered icon for macOS 26 and later, plus a flat rendition
#                 at every app icon size, which macOS 14 and 15 find through
#                 CFBundleIconName.
#   AppIcon.icns  The CFBundleIconFile fallback for readers that ignore the
#                 asset catalog.
#
# Runs on macOS with Xcode 26 or later; an older actool cannot read an .icon
# document. Both outputs are committed, so building the app never needs this.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
resources="$here/Resources"
icon="$resources/AppIcon.icon"

[[ "$(uname -s)" == Darwin ]] || { echo "make-icon.sh: run this on macOS" >&2; exit 1; }

# actool ships with Xcode, not with the Command Line Tools.
if ! /usr/bin/xcrun --find actool >/dev/null 2>&1; then
  if [[ -d /Applications/Xcode.app/Contents/Developer ]]; then
    export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
  else
    echo "make-icon.sh: actool not found; install Xcode 26 or later" >&2
    exit 1
  fi
fi

# The flat renditions have to reach back as far as the app itself does.
minimum="$(/usr/libexec/PlistBuddy -c 'Print :LSMinimumSystemVersion' "$resources/Info.plist")"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

/usr/bin/xcrun actool "$icon" \
  --compile "$work" \
  --app-icon AppIcon \
  --platform macosx \
  --target-device mac \
  --minimum-deployment-target "$minimum" \
  --development-region en \
  --enable-on-demand-resources NO \
  --output-partial-info-plist "$work/partial.plist" \
  --output-format human-readable-text --errors --warnings

# actool succeeds without writing an icon when --app-icon names none in the
# document, so make sure both outputs exist before replacing the committed ones.
for output in Assets.car AppIcon.icns; do
  [[ -s "$work/$output" ]] || { echo "make-icon.sh: actool wrote no $output" >&2; exit 1; }
done

cp "$work/Assets.car" "$work/AppIcon.icns" "$resources/"
echo "wrote $resources/Assets.car and $resources/AppIcon.icns (macOS $minimum and later)"
