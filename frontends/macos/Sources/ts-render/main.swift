import AppKit
import Foundation

// A development tool, never shipped: it draws the panel for every fixture in
// both appearances, and the menu bar icon in every state, so the look can be
// reviewed without a Mac in front of you.

let arguments = CommandLine.arguments
let fixturesDirectory = URL(
    filePath: value(of: "--fixtures", in: arguments) ?? defaultFixturesPath(),
    directoryHint: .isDirectory)
let outputDirectory = URL(
    filePath: value(of: "--out", in: arguments) ?? "renders", directoryHint: .isDirectory)

let application = NSApplication.shared
application.setActivationPolicy(.prohibited)
let delegate = RenderDelegate(fixtures: fixturesDirectory, output: outputDirectory)
application.delegate = delegate
application.run()

func value(of flag: String, in arguments: [String]) -> String? {
    guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else {
        return nil
    }
    return arguments[index + 1]
}

/// `data/fixtures` relative to this file, which is where the repository keeps it.
func defaultFixturesPath() -> String {
    URL(filePath: #filePath)
        .deletingLastPathComponent()  // ts-render
        .deletingLastPathComponent()  // Sources
        .deletingLastPathComponent()  // macos
        .deletingLastPathComponent()  // frontends
        .deletingLastPathComponent()  // repository root
        .appending(path: "data/fixtures", directoryHint: .isDirectory)
        .path(percentEncoded: false)
}
