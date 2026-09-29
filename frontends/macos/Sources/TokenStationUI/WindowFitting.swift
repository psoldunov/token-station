import AppKit
import Observation
import SwiftUI

extension View {
    /// Keeps the window this view is hosted in exactly as tall as the view,
    /// leaving the window's top edge where it is.
    ///
    /// A window-style `MenuBarExtra` sizes its window only by raising the
    /// window's minimum size to the content's. The window grows when the panel
    /// does, but nothing shrinks it again: close the Per Model list and the panel
    /// sits in the middle of a window still as tall as it was open, with a band
    /// of empty material above and below it. This sets the height both ways, and
    /// animates it on the panel's resize curve so the bottom edge moves with the
    /// rows above it.
    public func sizesWindowToFit() -> some View {
        modifier(WindowFitting())
    }
}

private struct WindowFitting: ViewModifier {
    @State private var fitter = WindowFitter()

    func body(content: Content) -> some View {
        content
            // Always laid out at its own height, whatever the window is at.
            .fixedSize(horizontal: false, vertical: true)
            .onGeometryChange(for: CGFloat.self) { $0.size.height } action: { height in
                fitter.fit(contentHeight: height)
            }
            .background(WindowReader { fitter.window = $0 })
            // Exactly as tall as the window is right now, with the content at
            // the top. The menu bar extra centres whatever size this reports, so
            // while the window is on its way to a new height this has to report
            // the height it has actually reached, or the whole panel slides.
            .frame(height: fitter.windowHeight, alignment: .top)
    }
}

/// Moves the window's bottom edge to wherever the content's ends.
@MainActor
@Observable
private final class WindowFitter {
    /// The window's content height as it stands; `nil` until there is one.
    private(set) var windowHeight: CGFloat?

    @ObservationIgnored weak var window: NSWindow? {
        didSet {
            guard window !== oldValue else { return }
            observe(window)
            // A window that has just appeared opens at the right height rather
            // than animating into it.
            hasFitted = false
            schedule()
        }
    }

    @ObservationIgnored private var contentHeight: CGFloat = 0
    @ObservationIgnored private var hasFitted = false
    @ObservationIgnored private var isScheduled = false
    @ObservationIgnored private var animationTarget: NSRect?
    @ObservationIgnored private var resizeObserver: (any NSObjectProtocol)?

    func fit(contentHeight: CGFloat) {
        self.contentHeight = contentHeight
        schedule()
    }

    /// This is called from inside a layout pass, and resizing the window lays
    /// the view out again, so the resize waits for the current pass to finish.
    private func schedule() {
        guard !isScheduled else { return }
        isScheduled = true
        Task { @MainActor [weak self] in
            self?.apply()
        }
    }

    private func apply() {
        isScheduled = false
        guard let window, contentHeight > 0 else { return }

        let current = window.contentRect(forFrameRect: window.frame)
        let target = window.frameRect(forContentRect: NSRect(
            x: current.minX,
            y: current.maxY - contentHeight,
            width: current.width,
            height: contentHeight
        ))
        guard target != animationTarget else { return }
        guard animationTarget != nil || target != window.frame else {
            track(window)
            hasFitted = true
            return
        }

        let animates = hasFitted
            && window.isVisible
            && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        hasFitted = true
        if animates {
            animate(window, to: target)
        } else {
            resize(window, to: target)
        }
    }

    /// Every step of the animation resizes the window, and each one reaches
    /// `windowHeight` through the resize notification.
    private func animate(_ window: NSWindow, to target: NSRect) {
        animationTarget = target
        NSAnimationContext.runAnimationGroup { context in
            context.duration = PanelMetrics.resizeDuration
            context.timingFunction = CAMediaTimingFunction(name: .easeOut)
            window.animator().setFrame(target, display: true)
        } completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.animationTarget == target else { return }
                self.animationTarget = nil
            }
        }
    }

    private func resize(_ window: NSWindow, to target: NSRect) {
        if animationTarget != nil {
            // Stops the animation still on its way to the previous height.
            NSAnimationContext.runAnimationGroup { context in
                context.duration = 0
                window.animator().setFrame(target, display: false)
            }
            animationTarget = nil
        }
        // The panel takes its new height first, so the window and the view it
        // hosts change size in the same pass.
        if windowHeight != contentHeight { windowHeight = contentHeight }
        window.setFrame(target, display: true)
    }

    private func observe(_ window: NSWindow?) {
        if let resizeObserver {
            NotificationCenter.default.removeObserver(resizeObserver)
        }
        resizeObserver = nil
        guard let window else {
            windowHeight = nil
            return
        }
        resizeObserver = NotificationCenter.default.addObserver(
            forName: NSWindow.didResizeNotification,
            object: window,
            queue: nil
        ) { [weak self, weak window] _ in
            MainActor.assumeIsolated {
                guard let self, let window else { return }
                self.track(window)
            }
        }
    }

    private func track(_ window: NSWindow) {
        let height = window.contentRect(forFrameRect: window.frame).height
        // Observation reports every assignment, equal or not.
        if windowHeight != height { windowHeight = height }
    }
}

/// Hands over the window a view has been put in.
private struct WindowReader: NSViewRepresentable {
    var onChange: @MainActor (NSWindow?) -> Void

    func makeNSView(context: Context) -> WindowReaderView {
        let view = WindowReaderView()
        view.onChange = onChange
        return view
    }

    func updateNSView(_ nsView: WindowReaderView, context: Context) {
        nsView.onChange = onChange
    }
}

private final class WindowReaderView: NSView {
    var onChange: (@MainActor (NSWindow?) -> Void)?

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        onChange?(window)
    }

    // A background, never a target: clicks belong to the panel above it.
    override func hitTest(_ point: NSPoint) -> NSView? { nil }
}
