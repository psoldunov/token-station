# Token Station for macOS

The menu bar front end. It renders what the daemon publishes and never reads a
provider itself: the Rust daemon (`crates/token-station`) does that and serves
one JSON snapshot over a Unix socket, described in [docs/socket-api.md](../../docs/socket-api.md).

## What is in here

| Target | What it is |
|--------|------------|
| `TokenStationCore` | Foundation only. The snapshot model, the JSON-RPC client, socket-path resolution, and every string the panel shows. |
| `TokenStationUI` | The SwiftUI panel, the menu bar image, the settings form, and the observable model behind them. No `@main`. |
| `TokenStation` | The app: `MenuBarExtra`, the daemon supervisor, notifications, the login item. |
| `ts-render` | A development tool. Draws the panel for every fixture in light and dark, plus the menu bar icon states, to PNGs. Never shipped. |

## Building

**Xcode has to be installed. There is no Xcode project.**

From the macOS 27 SDK on, SwiftUI's `@State` and its siblings are macros, and
the plugin that expands them — `libSwiftUIMacros.dylib` — ships inside Xcode's
`MacOSX.platform`, not with the Command Line Tools. A machine with only the
Command Line Tools can build `TokenStationCore` and nothing else.

That is the only thing Xcode is for: nothing here calls `xcodebuild`, there is
no `.xcodeproj` or `.xcworkspace`, and the whole package builds with
`swift build`. `scripts/build-app.sh` locates Xcode itself — it probes
`xcode-select`'s current toolchain for the plugin and, if it is missing, exports
`DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer` and says so. Set
`DEVELOPER_DIR` yourself to use a different Xcode.

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer   # when CLT is selected
swift build
swift test
```

```sh
scripts/build-app.sh                 # host architecture
scripts/build-app.sh --universal     # arm64 + x86_64, lipo'd
scripts/build-app.sh --zip           # also writes build/TokenStation-macOS.zip

CODESIGN_IDENTITY="Developer ID Application: …" scripts/build-app.sh
TOKEN_STATION_HELPER_BIN=/path/to/token-station scripts/build-app.sh   # skip cargo
```

It builds the Rust helper, builds the app, assembles
`build/Token Station.app` with the helper in `Contents/MacOS`, stamps the
version from the Cargo workspace, and signs the helper before the bundle.
`--universal` adds the two Rust targets with `rustup target add` when they are
missing. `--zip` always writes `build/TokenStation-macOS.zip`; the name carries
no version because CI and the release job upload exactly that path.

Local builds use an ad-hoc signature unless `CODESIGN_IDENTITY` names an
installed Developer ID Application certificate. On a `v*` tag, the release
workflow builds the universal app, imports that certificate, then signs and
notarizes it with App Store Connect API credentials. It staples the ticket and
publishes both `TokenStation-macOS.dmg` and `TokenStation-macOS.zip`. The zip
is recreated after stapling the app; zip archives cannot themselves be stapled.

The workflow reads five repository secrets: `APPLE_CERTIFICATE_BASE64` (the
Developer ID certificate and private key exported as a base64-encoded `.p12`),
`APPLE_CERTIFICATE_PASSWORD`, and the raw PEM `APPLE_API_KEY` plus its
`APPLE_API_KEY_ID` and team `APPLE_API_ISSUER`. The `.p12` must contain a
Developer ID Application certificate, not a development or installer identity.

## Checks

```sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer

swift build                                    # zero warnings expected
swift test                                     # swift-testing

swiftlint lint --strict                        # .swiftlint.yml

# A clean scratch path and --build-tests, so every file the analyzer rules judge
# appears in the log: an incremental build only logs what it recompiled.
swift build --build-tests --build-system native \
  --scratch-path /tmp/ts-analyze-build -v > /tmp/swiftbuild.log
swiftlint analyze --strict --compiler-log-path /tmp/swiftbuild.log

periphery scan --strict                        # .periphery.yml
```

`periphery` and `swiftlint analyze` both need the *native* SwiftPM build system,
because the Xcode build system writes its index somewhere neither tool looks;
`.periphery.yml` passes `--build-system native` for you. SwiftPM has begun
warning that the flag is deprecated, so both lines come out the day the tools
read the newer index.

## Renders

```sh
swift run ts-render --out /tmp/ts-renders
swift run ts-render --fixtures ../../data/fixtures --out /tmp/ts-renders
```

Renders are a review aid and are not committed.

## App icon

`Resources/AppIcon.icon` is an Icon Composer document: a background gradient,
a glass group for the two tubes, and a glass group for the Claude and Codex
levels inside them, each layer an SVG in `Assets/`. macOS 26 and later render it
live as Liquid Glass, in the default, dark, clear and tinted appearances. Edit
it in Icon Composer (Xcode > Open Developer Tool) or by hand, then compile it:

```sh
scripts/make-icon.sh    # Xcode 26 or later
```

That writes `Resources/Assets.car` — the layered icon, plus the flat renditions
macOS 14 and 15 read through `CFBundleIconName` — and `Resources/AppIcon.icns`,
the `CFBundleIconFile` fallback. Both are committed; `build-app.sh` only copies
them. To look at one appearance without building the app:

```sh
ictool="/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool"
"$ictool" Resources/AppIcon.icon --export-image --output-file /tmp/icon-dark.png \
  --platform macOS --rendition Dark --width 512 --height 512 --scale 1
```

The renditions are `Default`, `Dark`, `TintedLight`, `TintedDark`, `ClearLight`
and `ClearDark`; the tinted ones also take `--tint-color` and `--tint-strength`.

The Linux icon, `data/icons/hicolor/scalable/apps/dev.soldunov.TokenStation.svg`,
paints the same design flat, so change the two together.

## Development against a daemon

```sh
export TOKEN_STATION_SOCKET=/tmp/ts-dev.sock     # keep off the real daemon
export TOKEN_STATION_HELPER=/path/to/token-station
swift run TokenStation
```
