/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
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

    // A high-priority action is what the system tray puts in its own header,
    // beside configure and pin (applets/systemtray/qml/ExpandedRepresentation.qml);
    // elsewhere it sits in the applet's context menu.
    Plasmoid.contextualActions: [
        PlasmaCore.Action {
            text: i18nc("@action:button", "Refresh Now")
            icon.name: "view-refresh"
            priority: PlasmaCore.Action.HighPriority
            enabled: !daemon.refreshing
            onTriggered: daemon.refresh()
        }
    ]

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
        appletTitle: root.toolTipMainText
        configureAction: Plasmoid.internalAction("configure")
        // The system tray draws the applet title, the refresh action and the
        // configure button itself.
        containmentDrawsHeading: (Plasmoid.containmentDisplayHints & PlasmaCore.Types.ContainmentDrawsPlasmoidHeading) !== 0
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
