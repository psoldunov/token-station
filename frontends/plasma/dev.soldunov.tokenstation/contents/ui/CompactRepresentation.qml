/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
    SPDX-License-Identifier: MIT

    Tray representation: one thin vertical rounded bar per provider, Claude first.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.workspace.components as WorkspaceComponents
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

MouseArea {
    id: root

    /*! Parsed daemon snapshot, or null while nothing has arrived. */
    property var snapshot: null
    /*! Badge the worst bar's percentage below the meter, as the battery applet does. */
    property bool showPercentText: false
    /*! Mirrors PlasmoidItem.expanded so the popup toggles instead of re-opening. */
    property bool expandedState: false
    /*! One of PlasmaCore.Types.Horizontal / Vertical / Planar. */
    property int formFactor: PlasmaCore.Types.Horizontal
    property string accessibleName: ""
    property string accessibleDescription: ""

    signal toggleRequested(bool newExpanded)

    readonly property var bars: snapshot && snapshot.meter && snapshot.meter.bars ? snapshot.meter.bars : []
    readonly property bool isVertical: formFactor === PlasmaCore.Types.Vertical

    // Size the meter the way Kirigami.Icon sizes the tray's other icons: the
    // largest standard icon size that fits the shorter side. The system tray
    // fixes both sides of its cell, and the long one is the panel's thickness,
    // so sizing from it alone draws a meter taller than its neighbours. A bare
    // panel fixes one side and this item copies it to the other, so neither
    // binding loops.
    readonly property int iconSize: Kirigami.Units.iconSizes.roundedIconSize(
        Math.max(Kirigami.Units.iconSizes.small, Math.min(root.width, root.height)))
    // Leave a margin inside the box, as an icon's glyph does inside its canvas.
    readonly property int gaugeHeight: Math.round(iconSize * 0.75)
    readonly property int barWidth: Math.max(3, Math.round(gaugeHeight / 4))
    readonly property int barSpacing: Math.max(2, Math.round(gaugeHeight / 6))

    /*! Highest percentage across the bars; what the optional badge shows. */
    readonly property var worstPercent: {
        let worst = null;
        for (const bar of root.bars) {
            if (typeof bar.percent === "number" && (worst === null || bar.percent > worst)) {
                worst = bar.percent;
            }
        }
        return worst;
    }

    function levelColor(level) {
        switch (level) {
        case "critical":
            return Kirigami.Theme.negativeTextColor;
        case "warning":
            return Kirigami.Theme.neutralTextColor;
        default:
            return Kirigami.Theme.textColor;
        }
    }

    activeFocusOnTab: true
    hoverEnabled: true

    Accessible.role: Accessible.Button
    Accessible.name: root.accessibleName
    Accessible.description: root.accessibleDescription

    // Square in a bare panel, like any icon applet. The system tray ignores these
    // and hands every item the same cell.
    Layout.minimumWidth: Kirigami.Units.iconSizes.small
    Layout.minimumHeight: Kirigami.Units.iconSizes.small
    Layout.preferredWidth: root.isVertical ? -1 : root.height
    Layout.preferredHeight: root.isVertical ? root.width : -1

    implicitWidth: Kirigami.Units.iconSizes.smallMedium
    implicitHeight: Kirigami.Units.iconSizes.smallMedium

    // A click while the popup is open first dismisses it, so the state at press
    // time is the one to invert.
    property bool wasExpanded: false
    onPressed: root.wasExpanded = root.expandedState
    onClicked: root.toggleRequested(!root.wasExpanded)
    Keys.onPressed: event => {
        if (event.key === Qt.Key_Space || event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
            root.toggleRequested(!root.expandedState);
            event.accepted = true;
        }
    }

    Item {
        id: meterBox

        anchors.centerIn: parent
        width: root.iconSize
        height: root.iconSize

        Row {
            id: gauge

            anchors.centerIn: parent
            spacing: root.barSpacing
            height: root.gaugeHeight

            Repeater {
                model: root.bars.length > 0 ? root.bars : [{ "percent": null, "level": "normal" }]

                delegate: Item {
                    id: barItem

                    required property var modelData

                    readonly property bool hasValue: typeof modelData.percent === "number"
                    readonly property real fraction: hasValue ? Math.max(0, Math.min(100, modelData.percent)) / 100 : 0
                    readonly property color barColor: root.levelColor(modelData.level)

                    width: root.barWidth
                    height: root.gaugeHeight

                    // Empty outline when the provider has no reading yet.
                    Rectangle {
                        id: track

                        anchors.fill: parent
                        radius: width / 2
                        color: barItem.hasValue ? Qt.alpha(barItem.barColor, 0.25) : "transparent"
                        border.width: barItem.hasValue ? 0 : Math.max(1, Math.round(root.barWidth / 4))
                        border.color: Qt.alpha(Kirigami.Theme.textColor, 0.5)
                        antialiasing: true
                    }

                    Rectangle {
                        anchors.left: parent.left
                        anchors.right: parent.right
                        anchors.bottom: parent.bottom
                        height: Math.round(barItem.fraction * barItem.height)
                        visible: barItem.hasValue && height > 0
                        radius: track.radius
                        color: barItem.barColor
                        antialiasing: true

                        Behavior on height {
                            NumberAnimation {
                                duration: Kirigami.Units.longDuration
                                easing.type: Easing.OutCubic
                            }
                        }
                    }
                }
            }
        }
    }

    // The tray gives every item a fixed cell, so text beside the meter would
    // spill over the neighbouring icons. Badge it the way the battery applet
    // badges its charge instead.
    WorkspaceComponents.BadgeOverlay {
        id: badge

        anchors.right: parent.right
        anchors.bottom: parent.bottom

        visible: root.showPercentText && root.worstPercent !== null
        text: root.worstPercent !== null ? Formatters.percent(root.worstPercent) : ""
        // Plasma 6.4 and 6.5 scale the badge font from this and 6.6 keeps the
        // badge inside it; 6.7 ignores it.
        icon: meterBox

        // Centre it when it is wider than the cell, as the battery badge does.
        states: [
            State {
                when: badge.width >= root.width
                AnchorChanges {
                    target: badge
                    anchors.right: undefined
                    anchors.horizontalCenter: parent.horizontalCenter
                }
            }
        ]
    }
}
