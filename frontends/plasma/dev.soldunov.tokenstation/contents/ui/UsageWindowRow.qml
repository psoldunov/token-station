/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    One plan-limit window: label, meter, percentage and live reset countdown.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.components as PlasmaComponents3
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
    readonly property color levelColor: {
        switch (root.levelName) {
        case "critical":
            return Kirigami.Theme.negativeTextColor;
        case "warning":
            return Kirigami.Theme.neutralTextColor;
        default:
            return Kirigami.Theme.highlightColor;
        }
    }
    readonly property string statusText: {
        const parts = [];
        if (root.hasPercent) {
            parts.push(Formatters.percent(root.usageWindow.usedPercent));
        }
        const countdown = Formatters.resetsIn(root.usageWindow ? root.usageWindow.resetsAt : null, root.now);
        if (countdown.length > 0) {
            parts.push(countdown);
        }
        return parts.join(" · ");
    }

    spacing: 0

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        PlasmaComponents3.Label {
            Layout.fillWidth: true
            text: root.usageWindow && root.usageWindow.label ? root.usageWindow.label : ""
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        PlasmaComponents3.Label {
            text: root.statusText
            color: root.levelName === "normal" ? Kirigami.Theme.textColor : root.levelColor
            font: Kirigami.Theme.smallFont
            horizontalAlignment: Text.AlignRight
        }
    }

    PlasmaComponents3.ProgressBar {
        Layout.fillWidth: true
        Layout.topMargin: Math.round(Kirigami.Units.smallSpacing / 2)

        from: 0
        to: 100
        value: root.hasPercent ? root.usageWindow.usedPercent : 0
        indeterminate: !root.hasPercent

        // Plasma's ProgressBar paints its groove from the theme highlight colour,
        // so recolouring the theme is how a warning/critical bar is tinted.
        Kirigami.Theme.inherit: false
        Kirigami.Theme.highlightColor: root.levelColor

        Accessible.name: root.usageWindow && root.usageWindow.label ? root.usageWindow.label : ""
        Accessible.description: root.statusText
    }

    Sparkline {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        Layout.preferredHeight: Kirigami.Units.gridUnit * 2

        points: root.historyPoints
        lineColor: root.levelColor
    }
}
