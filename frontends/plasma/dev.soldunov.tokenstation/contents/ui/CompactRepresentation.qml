/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
    SPDX-License-Identifier: MIT

    Tray representation: one thin vertical rounded bar per provider, Claude first.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.core as PlasmaCore
import org.kde.plasma.components as PlasmaComponents3
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

MouseArea {
    id: root

    /*! Parsed daemon snapshot, or null while nothing has arrived. */
    property var snapshot: null
    /*! Draw the worst bar's percentage next to the meter. */
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

    // The panel fixes one axis; derive the meter from that axis only, never from
    // the axis this item asks the panel for, or the size binding would loop.
    readonly property int thickness: Math.max(Kirigami.Units.iconSizes.small, isVertical ? root.width : root.height)
    readonly property int gaugeHeight: Math.max(Kirigami.Units.iconSizes.small,
                                                Math.min(Kirigami.Units.iconSizes.medium, Math.round(thickness * 0.8)))
    readonly property int barWidth: Math.max(3, Math.round(gaugeHeight / 4))
    readonly property int barSpacing: Math.max(2, Math.round(gaugeHeight / 6))

    /*! Highest percentage across the bars; what the optional percent text shows. */
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

    Layout.minimumWidth: root.isVertical ? Kirigami.Units.iconSizes.small : content.implicitWidth
    Layout.minimumHeight: root.isVertical ? content.implicitHeight : Kirigami.Units.iconSizes.small
    Layout.preferredWidth: root.isVertical ? -1 : content.implicitWidth
    Layout.preferredHeight: root.isVertical ? content.implicitHeight : -1

    implicitWidth: content.implicitWidth
    implicitHeight: content.implicitHeight

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

    Grid {
        id: content

        anchors.centerIn: parent
        columns: root.isVertical ? 1 : 2
        spacing: percentLabel.visible ? Kirigami.Units.smallSpacing : 0
        horizontalItemAlignment: Grid.AlignHCenter
        verticalItemAlignment: Grid.AlignVCenter

        Row {
            id: gauge

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

        PlasmaComponents3.Label {
            id: percentLabel

            visible: root.showPercentText && root.worstPercent !== null
            text: root.worstPercent !== null ? Formatters.percent(root.worstPercent) : ""
            color: root.levelColor(root.snapshot && root.snapshot.meter ? root.snapshot.meter.level : "normal")
            font: Kirigami.Theme.smallFont
            verticalAlignment: Text.AlignVCenter
            horizontalAlignment: Text.AlignHCenter
        }
    }
}
