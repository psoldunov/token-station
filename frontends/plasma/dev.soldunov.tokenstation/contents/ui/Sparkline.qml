/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Percentage-over-time trace for one usage window, as returned by GetHistory().
*/
pragma ComponentBehavior: Bound

import QtQuick

import org.kde.kirigami as Kirigami

Canvas {
    id: root

    /*! `[[unixSeconds, percent], …]`, oldest first. */
    property var points: []
    property color lineColor: Kirigami.Theme.highlightColor
    /*! Percentage axis always spans the whole window, so traces stay comparable. */
    readonly property real maxPercent: 100

    readonly property bool hasData: Array.isArray(points) && points.length >= 2

    implicitHeight: Kirigami.Units.gridUnit * 2
    visible: hasData
    antialiasing: true

    onPointsChanged: requestPaint()
    onLineColorChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()

    onPaint: {
        const context = getContext("2d");
        context.reset();
        if (!root.hasData || width <= 0 || height <= 0) {
            return;
        }

        const firstTimestamp = root.points[0][0];
        const lastTimestamp = root.points[root.points.length - 1][0];
        const span = lastTimestamp - firstTimestamp;
        const strokeWidth = Math.max(1, Math.round(Kirigami.Units.smallSpacing / 2));
        const usableHeight = Math.max(1, height - strokeWidth);

        const xAt = timestamp => span > 0 ? ((timestamp - firstTimestamp) / span) * width : width;
        const yAt = percent => {
            const clamped = Math.max(0, Math.min(root.maxPercent, percent));
            return strokeWidth / 2 + usableHeight - (clamped / root.maxPercent) * usableHeight;
        };

        context.beginPath();
        context.moveTo(0, height);
        for (const point of root.points) {
            context.lineTo(xAt(point[0]), yAt(point[1]));
        }
        context.lineTo(width, height);
        context.closePath();
        context.fillStyle = Qt.alpha(root.lineColor, 0.2);
        context.fill();

        context.beginPath();
        for (let index = 0; index < root.points.length; ++index) {
            const point = root.points[index];
            const x = xAt(point[0]);
            const y = yAt(point[1]);
            if (index === 0) {
                context.moveTo(x, y);
            } else {
                context.lineTo(x, y);
            }
        }
        context.lineWidth = strokeWidth;
        context.lineJoin = "round";
        context.lineCap = "round";
        context.strokeStyle = root.lineColor;
        context.stroke();
    }
}
