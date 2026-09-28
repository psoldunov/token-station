/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    One plan-limit window: label, meter, percentage and live reset countdown.

    Laid out like powerdevil's BatteryItem: a label row over an unstyled
    PlasmaComponents3.ProgressBar, secondary text as a dimmed smallFont label.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.components as PlasmaComponents3
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

ColumnLayout {
    id: root

    /*! One entry of `provider.windows`. */
    required property var usageWindow
    /*! Current Unix time in seconds; the popup ticks it so countdowns stay live. */
    required property real now
    /*! `[[unixSeconds, percent], …]` for this window, or an empty array. */
    property var historyPoints: []

    readonly property bool hasPercent: usageWindow && typeof usageWindow.usedPercent === "number"
    readonly property string levelName: usageWindow && usageWindow.level ? usageWindow.level : "normal"
    readonly property string countdownText: Formatters.resetsIn(usageWindow ? usageWindow.resetsAt : null, now)

    spacing: 0

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents3.Label {
            Layout.fillWidth: true
            text: root.usageWindow && root.usageWindow.label ? root.usageWindow.label : ""
            textFormat: Text.PlainText
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        PlasmaExtras.DescriptiveLabel {
            visible: root.countdownText.length > 0
            text: root.countdownText
            textFormat: Text.PlainText
        }

        PlasmaComponents3.Label {
            horizontalAlignment: Text.AlignRight
            visible: root.hasPercent
            // Warning and critical are the only states worth a colour; Breeze uses
            // the theme's neutral/negative text colours for exactly this.
            color: root.levelName === "critical" ? Kirigami.Theme.negativeTextColor
                 : root.levelName === "warning" ? Kirigami.Theme.neutralTextColor
                 : Kirigami.Theme.textColor
            text: root.hasPercent ? Formatters.percent(root.usageWindow.usedPercent) : ""
            textFormat: Text.PlainText
        }
    }

    PlasmaComponents3.ProgressBar {
        Layout.fillWidth: true

        from: 0
        to: 100
        value: root.hasPercent ? root.usageWindow.usedPercent : 0
        indeterminate: !root.hasPercent

        Accessible.name: root.usageWindow && root.usageWindow.label ? root.usageWindow.label : ""
        Accessible.description: root.countdownText
    }

    Sparkline {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        Layout.preferredHeight: Kirigami.Units.gridUnit

        points: root.historyPoints
    }
}
