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

## Development against a daemon

```sh
export TOKEN_STATION_SOCKET=/tmp/ts-dev.sock     # keep off the real daemon
export TOKEN_STATION_HELPER=/path/to/token-station
swift run TokenStation
```
