import CoreGraphics

/// The measurements the panel is built from.
///
/// They come from Apple's own menu bar extras: a 320 pt panel, 14 pt side
/// margins, an 11 pt secondary line, a 13 pt menu row. Keeping them in one place
/// is what stops the panel drifting into looking hand-made.
enum PanelMetrics {
    static let width: CGFloat = 320
    /// How tall the scrolling part of the panel may grow before it scrolls.
    static let maximumContentHeight: CGFloat = 560

    static let horizontalPadding: CGFloat = 14
    static let topPadding: CGFloat = 10
    static let bottomPadding: CGFloat = 7

    /// Gap between two rows inside one provider section.
    static let rowSpacing: CGFloat = 6
    /// Gap between two provider sections, either side of the divider.
    static let sectionSpacing: CGFloat = 10

    static let titleSize: CGFloat = 13
    static let sectionHeaderSize: CGFloat = 11
    static let rowSize: CGFloat = 13
    static let secondarySize: CGFloat = 11
    static let menuRowSize: CGFloat = 13

    static let barHeight: CGFloat = 6
    static let chartHeight: CGFloat = 34

    static let menuRowCornerRadius: CGFloat = 5
    static let menuRowVerticalPadding: CGFloat = 4
    static let menuRowHorizontalPadding: CGFloat = 8
}
