import AppKit
import SwiftUI

/// Renders a SwiftUI view to a PNG without a screen.
///
/// `screencapture` needs a logged-in window server session, which a build over
/// SSH does not have. Hosting the view in a borderless window placed off screen
/// and caching its display does, and produces exactly the pixels the panel would
/// draw — except for materials, which have no desktop to sample and come out flat.
@MainActor
enum OffscreenRenderer {
    /// How long the run loop is pumped so SwiftUI finishes one round of layout.
    private static let settleInterval: TimeInterval = 0.25

    static func png(of view: some View, appearance: NSAppearance) -> Data? {
        let hosting = NSHostingView(
            rootView: AnyView(
                view
                    // Without this, every control draws in its inactive grey:
                    // an off-screen window is never the key window.
                    .environment(\.controlActiveState, .key)
            )
        )
        hosting.appearance = appearance

        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 600, height: 600),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.appearance = appearance
        window.contentView = hosting
        window.isOpaque = false
        window.backgroundColor = .clear
        window.setFrameOrigin(NSPoint(x: -20_000, y: -20_000))
        window.orderFront(nil)
        defer { window.orderOut(nil) }

        // Charts and disclosure rows settle over a couple of layout passes, and
        // the window has to be resized to what they ask for between them.
        for _ in 0..<3 {
            hosting.layoutSubtreeIfNeeded()
            RunLoop.current.run(until: Date().addingTimeInterval(settleInterval))
            let size = hosting.fittingSize
            guard size.width > 0, size.height > 0 else { return nil }
            window.setContentSize(size)
        }
        hosting.layoutSubtreeIfNeeded()

        guard let rep = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            return nil
        }
        hosting.cacheDisplay(in: hosting.bounds, to: rep)
        return rep.representation(using: .png, properties: [:])
    }
}
