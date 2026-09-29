import AppKit
import TokenStationCore

/// The menu bar image: one capsule per provider, filling from the bottom.
///
/// While everything is normal the image is a template: the system fills its mask
/// with the menu bar's own label colour, which is the only way to stay right on
/// a menu bar whose appearance follows the wallpaper rather than the app.
///
/// Once a window crosses a threshold the image stops being a template and takes
/// the system orange or red, the way the battery icon does when it runs low. Its
/// outline then has to carry the label colour itself, so the drawing handler
/// reads `NSColor.labelColor` when it runs — inside whatever appearance the
/// status item is drawing with — and the image is never cached, so that handler
/// runs again when the menu bar changes underneath it.
public enum MeterIcon {
    /// Status item images are 18 pt tall, as in every first-party menu extra.
    public static let height: CGFloat = 18

    private static let capsuleWidth: CGFloat = 5
    private static let capsuleHeight: CGFloat = 15
    private static let capsuleGap: CGFloat = 3
    private static let textGap: CGFloat = 4
    private static let outlineWidth: CGFloat = 1
    private static let minimumFill: CGFloat = 2

    /// Draws the icon for a snapshot's meter.
    ///
    /// - Parameters:
    ///   - meter: The meter to draw; `nil` draws two empty capsules, which is
    ///     what the icon shows before the first snapshot arrives.
    ///   - percentageText: Appended to the right when the preference is on.
    public static func image(for meter: Meter?, percentageText: String?) -> NSImage {
        let published = meter?.bars ?? []
        let bars = published.isEmpty
            ? [MeterBar(provider: .claude, percent: nil, level: .normal, windowId: nil),
               MeterBar(provider: .codex, percent: nil, level: .normal, windowId: nil),
              ]
            : published
        let isTemplate = bars.allSatisfy { !$0.level.isElevated }

        let label = percentageText.map { text -> NSAttributedString in
            NSAttributedString(string: text, attributes: [
                .font: menuBarFont(),
                .foregroundColor: isTemplate ? NSColor.black : NSColor.labelColor,
            ])
        }
        let labelSize = label?.size() ?? .zero

        var width = CGFloat(bars.count) * capsuleWidth
            + CGFloat(max(bars.count - 1, 0)) * capsuleGap
        if label != nil { width += textGap + ceil(labelSize.width) }

        let image = NSImage(
            size: NSSize(width: width, height: height),
            flipped: false
        ) { _ in
            draw(bars: bars, isTemplate: isTemplate, label: label, labelSize: labelSize)
            return true
        }
        image.isTemplate = isTemplate
        // A coloured icon resolves `labelColor` while it draws, so it must draw
        // again when the menu bar's appearance changes rather than hand back the
        // bitmap it made under the old one.
        if !isTemplate { image.cacheMode = .never }
        image.accessibilityDescription = accessibilityDescription(for: bars)
        return image
    }

    /// The number the "Show Percentage in Menu Bar" preference appends: the
    /// highest reading across the bars, because that is the one about to bite.
    public static func percentageText(for meter: Meter?) -> String? {
        let percents = (meter?.bars ?? []).compactMap(\.percent).filter(\.isFinite)
        guard let highest = percents.max() else { return nil }
        return Formatting.percent(highest)
    }

    /// `Claude Code 34% used, Codex 12% used` for VoiceOver.
    public static func accessibilityDescription(for bars: [MeterBar]) -> String {
        let parts = bars.map { bar -> String in
            let name = bar.provider == .codex ? "Codex" : "Claude Code"
            guard let percent = bar.percent else { return "\(name) no data" }
            return "\(name) \(Formatting.percent(percent)) used"
        }
        return Formatting.joinSpoken(parts)
    }

    private static func draw(
        bars: [MeterBar],
        isTemplate: Bool,
        label: NSAttributedString?,
        labelSize: NSSize
    ) {
        let outline = isTemplate
            ? NSColor.black.withAlphaComponent(0.4)
            : NSColor.labelColor.withAlphaComponent(0.45)
        let bottom = (height - capsuleHeight) / 2

        for (index, bar) in bars.enumerated() {
            let x = CGFloat(index) * (capsuleWidth + capsuleGap)
            let frame = NSRect(
                x: x, y: bottom, width: capsuleWidth, height: capsuleHeight)
            let path = NSBezierPath(
                roundedRect: frame.insetBy(dx: outlineWidth / 2, dy: outlineWidth / 2),
                xRadius: (capsuleWidth - outlineWidth) / 2,
                yRadius: (capsuleWidth - outlineWidth) / 2)
            path.lineWidth = outlineWidth
            outline.setStroke()
            path.stroke()

            guard let percent = bar.percent, percent.isFinite, percent > 0 else { continue }
            let fraction = min(max(percent, 0), 100) / 100
            // A sliver rather than nothing at 1 %, but small enough that 12 %
            // and 34 % are still told apart.
            let fillHeight = max(capsuleHeight * fraction, minimumFill)

            NSGraphicsContext.saveGraphicsState()
            NSBezierPath(
                roundedRect: frame,
                xRadius: capsuleWidth / 2,
                yRadius: capsuleWidth / 2
            ).addClip()
            fillColor(for: bar.level, isTemplate: isTemplate).setFill()
            NSRect(x: x, y: bottom, width: capsuleWidth, height: fillHeight).fill()
            NSGraphicsContext.restoreGraphicsState()
        }

        guard let label else { return }
        let x = CGFloat(bars.count) * capsuleWidth
            + CGFloat(max(bars.count - 1, 0)) * capsuleGap + textGap
        label.draw(at: NSPoint(x: x, y: (height - labelSize.height) / 2))
    }

    private static func fillColor(for level: Level, isTemplate: Bool) -> NSColor {
        switch level {
        case .warning: .systemOrange
        case .critical: .systemRed
        case .normal, .unknown: isTemplate ? .black : .labelColor
        }
    }

    /// The menu bar's own font, with digits that do not jump as the number changes.
    private static func menuBarFont() -> NSFont {
        let base = NSFont.menuBarFont(ofSize: 0)
        return NSFont.monospacedDigitSystemFont(
            ofSize: base.pointSize - 1, weight: .regular)
    }
}
