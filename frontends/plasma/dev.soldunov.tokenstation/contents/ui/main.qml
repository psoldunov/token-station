/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Token Station: Claude Code and Codex plan usage in the Plasma system tray.
    The daemon owns all state; this applet only renders the snapshot it publishes.
*/
pragma ComponentBehavior: Bound

import QtQuick

import org.kde.plasma.plasmoid
import org.kde.plasma.core as PlasmaCore
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

PlasmoidItem {
    id: root

    // First child, so the translated unit strings are in place before any sibling
    // or representation formats a number.
    QtObject {
        Component.onCompleted: Formatters.setLabels({
            "day": i18nc("@item:intext Duration unit, days", "d"),
            "hour": i18nc("@item:intext Duration unit, hours", "h"),
            "minute": i18nc("@item:intext Duration unit, minutes", "min"),
            "second": i18nc("@item:intext Duration unit, seconds", "s"),
            "thousand": i18nc("@item:intext Token count suffix, thousands", "K"),
            "million": i18nc("@item:intext Token count suffix, millions", "M"),
            "billion": i18nc("@item:intext Token count suffix, billions", "B"),
            "percent": i18nc("@item:intext A percentage, %1 is the number", "%1 %"),
            "now": i18nc("@item:intext A window that resets right now", "now"),
            "justNow": i18nc("@item:intext Something that happened seconds ago", "just now"),
            "ago": i18nc("@item:intext How long ago something happened, %1 is a duration", "%1 ago"),
            "resetsIn": i18nc("@item:intext When a usage window resets, %1 is a duration", "resets in %1"),
            "unknown": i18nc("@item:intext No value available", "—")
        })
    }

    readonly property var snapshot: daemon.snapshot
    readonly property var providers: snapshot && snapshot.providers ? snapshot.providers : []
    readonly property string meterLevel: snapshot && snapshot.meter && snapshot.meter.level ? snapshot.meter.level : "normal"
    readonly property var meterBars: snapshot && snapshot.meter && snapshot.meter.bars ? snapshot.meter.bars : []

    /*! Current Unix time in seconds, ticked only while something reads a countdown. */
    property real now: Date.now() / 1000
    /*! The pointer is over the tray meter, so the tooltip may be on screen. */
    property bool compactHovered: false

    function windowById(provider, windowId) {
        const windows = provider.windows || [];
        for (const usageWindow of windows) {
            if (usageWindow.id === windowId) {
                return usageWindow;
            }
        }
        return null;
    }

    function barFor(providerId) {
        for (const bar of root.meterBars) {
            if (bar.provider === providerId) {
                return bar;
            }
        }
        return null;
    }

    /*! "Claude Code · Session 34 % · resets in 2 h 14 min" */
    function providerSummary(provider) {
        if (provider.message) {
            return i18nc("@info:tooltip Provider name and why it has no numbers",
                         "%1 · %2", provider.name, provider.message);
        }
        const bar = root.barFor(provider.id);
        const usageWindow = bar && bar.windowId ? root.windowById(provider, bar.windowId) : null;
        if (!usageWindow) {
            return i18nc("@info:tooltip Provider with no reading yet",
                         "%1 · %2", provider.name, i18nc("@info:status", "No reading yet"));
        }
        const countdown = Formatters.resetsIn(usageWindow.resetsAt, root.now);
        const usage = i18nc("@info:tooltip Window name and how much of it is used, e.g. 'Session 34 %'",
                            "%1 %2", usageWindow.label, Formatters.percent(usageWindow.usedPercent));
        return countdown.length > 0
            ? i18nc("@info:tooltip Provider, window usage and reset countdown",
                    "%1 · %2 · %3", provider.name, usage, countdown)
            : i18nc("@info:tooltip Provider and window usage", "%1 · %2", provider.name, usage);
    }

    switchWidth: Kirigami.Units.gridUnit * 14
    switchHeight: Kirigami.Units.gridUnit * 14

    Plasmoid.status: {
        if (!daemon.daemonRunning || !root.snapshot) {
            return PlasmaCore.Types.PassiveStatus;
        }
        const live = root.providers.filter(provider => provider.state !== "disabled" && provider.state !== "not_installed");
        if (live.length === 0) {
            return PlasmaCore.Types.PassiveStatus;
        }
        if (root.meterLevel === "critical") {
            return PlasmaCore.Types.NeedsAttentionStatus;
        }
        return PlasmaCore.Types.ActiveStatus;
    }

    toolTipMainText: i18nc("@title Applet name", "Token Station")
    toolTipSubText: {
        if (!daemon.daemonRunning) {
            return i18nc("@info:tooltip", "The Token Station service is not running.");
        }
        if (!root.snapshot) {
            return i18nc("@info:tooltip", "Reading usage…");
        }
        return root.providers.map(provider => root.providerSummary(provider)).join("\n");
    }

    compactRepresentation: CompactRepresentation {
        snapshot: root.snapshot
        showPercentText: Plasmoid.configuration.showPercentText
        expandedState: root.expanded
        formFactor: Plasmoid.formFactor
        accessibleName: root.toolTipMainText
        accessibleDescription: root.toolTipSubText
        onToggleRequested: newExpanded => root.expanded = newExpanded
        onContainsMouseChanged: root.compactHovered = containsMouse
    }

    fullRepresentation: FullRepresentation {
        snapshot: root.snapshot
        daemonRunning: daemon.daemonRunning
        refreshing: daemon.refreshing
        now: root.now
        errorMessage: daemon.lastError
        title: root.toolTipMainText
        configureAction: Plasmoid.internalAction("configure")
        onRefreshRequested: daemon.refresh()
        onHistoryRequested: (provider, windowId, since, callback) => daemon.history(provider, windowId, since, callback)
    }

    DaemonClient {
        id: daemon
    }

    // Countdowns only need to tick while somebody can read them.
    Timer {
        interval: 30000
        repeat: true
        running: root.expanded || root.compactHovered
        triggeredOnStart: true
        onTriggered: root.now = Date.now() / 1000
    }

    // A fresh snapshot is also a fresh clock reading.
    onSnapshotChanged: root.now = Date.now() / 1000
}
