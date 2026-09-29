import AppKit
import TokenStationCore
import TokenStationUI

/// Draws the menu bar image on a menu-bar-coloured strip, at print size.
@MainActor
enum IconRenderer {
    private static let scale: CGFloat = 6
    private static let padding: CGFloat = 6

    static func png(
        meter: Meter?,
        percentageText: String?,
        appearance: NSAppearance
    ) -> Data? {
        let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        let image = MeterIcon.image(for: meter, percentageText: percentageText)

        let size = NSSize(
            width: (image.size.width + padding * 2) * scale,
            height: (MeterIcon.height + padding * 2) * scale)
        guard let rep = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: Int(size.width.rounded()),
            pixelsHigh: Int(size.height.rounded()),
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ) else { return nil }
        guard let context = NSGraphicsContext(bitmapImageRep: rep) else { return nil }

        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        context.imageInterpolation = .high

        // The menu bar's own two backgrounds, so the template tint can be judged.
        (isDark ? NSColor(white: 0.11, alpha: 1) : NSColor(white: 0.95, alpha: 1)).setFill()
        NSRect(origin: .zero, size: size).fill()

        let target = NSRect(
            x: padding * scale,
            y: padding * scale + (MeterIcon.height - image.size.height) / 2 * scale,
            width: image.size.width * scale,
            height: image.size.height * scale)

        // A template image is a mask; the menu bar fills it with its own label
        // colour, and this is what that looks like. A coloured one resolves its
        // outline while it draws, so it is drawn inside the menu bar's
        // appearance rather than the process's.
        appearance.performAsCurrentDrawingAppearance {
            let drawn = image.isTemplate
                ? tinted(image, with: isDark ? .white : .black)
                : image
            drawn.draw(in: target)
        }

        NSGraphicsContext.restoreGraphicsState()
        return rep.representation(using: .png, properties: [:])
    }

    /// Fills a template image's mask with one colour, on a transparent backing.
    private static func tinted(_ image: NSImage, with color: NSColor) -> NSImage {
        NSImage(size: image.size, flipped: false) { rect in
            image.draw(in: rect)
            color.set()
            rect.fill(using: .sourceAtop)
            return true
        }
    }
}
