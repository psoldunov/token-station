/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Popup: optional heading, one scrollable section per provider, "updated" footer.

    Structure follows plasma-workspace's device notifier
    (applets/devicenotifier/qml/FullRepresentation.qml): a PlasmoidHeading that
    only carries the applet's own actions, a ScrollView body with largeSpacing
    margins, and a PlaceholderMessage centred over it for the empty states. The
    system tray draws the title and the configure button itself
    (applets/systemtray/qml/ExpandedRepresentation.qml), so this header hides both
    when the containment says it draws the heading.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.components as PlasmaComponents3
import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

PlasmaExtras.Representation {
    id: root

    /*! Parsed daemon snapshot, or null while nothing has arrived. */
    property var snapshot: null
    /*! The daemon owns its bus name right now. */
    property bool daemonRunning: true
    /*! A Refresh() call is in flight. */
    property bool refreshing: false
    /*! Current Unix time in seconds; the popup ticks it so countdowns stay live. */
    property real now: Date.now() / 1000
    /*! The applet's own configure action, or null outside a running Plasma shell. */
    property var configureAction: null
    /*! The containment already shows the applet title and configure button. */
    property bool containmentDrawsHeading: false
    property string appletTitle: i18nc("@title Applet name", "Token Station")
    /*! Transport-level problem to surface above the provider sections. */
    property string errorMessage: ""

    signal refreshRequested
    /*! `callback(points, errorMessage)`; see DaemonClient.history(). */
    signal historyRequested(string provider, string windowId, real since, var callback)

    readonly property var providers: snapshot && snapshot.providers ? snapshot.providers : []
    readonly property bool loading: providers.length > 0 && providers.every(provider => provider.state === "loading")
    readonly property bool hasContent: daemonRunning && providers.length > 0 && !loading

    /*! `"<providerId>/<windowId>"` to `[[unixSeconds, percent], …]`. */
    property var historyCache: ({})
    property real lastHistoryFetch: 0

    readonly property int sessionHistorySeconds: 24 * 60 * 60
    readonly property int weeklyHistorySeconds: 7 * 24 * 60 * 60

    function historyFor(providerId) {
        const perWindow = {};
        for (const key in root.historyCache) {
            const separator = key.indexOf("/");
            if (key.substring(0, separator) === providerId) {
                perWindow[key.substring(separator + 1)] = root.historyCache[key];
            }
        }
        return perWindow;
    }

    /*! Ask the daemon for every window the popup currently draws. */
    function loadHistory() {
        for (const provider of root.providers) {
            for (const usageWindow of provider.windows || []) {
                if (usageWindow.kind !== "session" && usageWindow.kind !== "weekly") {
                    continue;
                }
                const span = usageWindow.kind === "weekly" ? root.weeklyHistorySeconds : root.sessionHistorySeconds;
                const key = provider.id + "/" + usageWindow.id;
                root.historyRequested(provider.id, usageWindow.id, Math.floor(root.now - span), (points, error) => {
                    if (error.length > 0 || !points || points.length === undefined) {
                        return;
                    }
                    // Replace rather than mutate, so bindings on historyCache re-run.
                    const updated = Object.assign({}, root.historyCache);
                    updated[key] = points;
                    root.historyCache = updated;
                });
            }
        }
    }

    collapseMarginsHint: true
    focus: true

    Layout.minimumWidth: Kirigami.Units.gridUnit * 18
    Layout.minimumHeight: Kirigami.Units.gridUnit * 18
    Layout.preferredWidth: Kirigami.Units.gridUnit * 22
    Layout.preferredHeight: Kirigami.Units.gridUnit * 28

    header: PlasmaExtras.PlasmoidHeading {
        RowLayout {
            anchors.fill: parent
            spacing: Kirigami.Units.smallSpacing

            Kirigami.Heading {
                Layout.fillWidth: true
                visible: !root.containmentDrawsHeading
                level: 1
                text: root.appletTitle
                textFormat: Text.PlainText
                elide: Text.ElideRight
                maximumLineCount: 1
            }

            Item {
                Layout.fillWidth: root.containmentDrawsHeading
            }

            PlasmaComponents3.BusyIndicator {
                Layout.preferredWidth: Kirigami.Units.iconSizes.smallMedium
                Layout.preferredHeight: Kirigami.Units.iconSizes.smallMedium
                visible: root.refreshing
                running: visible
            }

            PlasmaComponents3.ToolButton {
                icon.name: "view-refresh"
                text: i18nc("@action:button", "Refresh Now")
                display: PlasmaComponents3.AbstractButton.IconOnly
                enabled: !root.refreshing
                Accessible.description: i18nc("@info:tooltip", "Check plan limits again now")
                onClicked: root.refreshRequested()

                PlasmaComponents3.ToolTip {
                    text: parent.Accessible.description
                }
            }

            // Plasmoid.internalAction() hands back a QAction, which QQC2's
            // `action` property will not accept; the system tray's own configure
            // button drives it the same way, by hand.
            PlasmaComponents3.ToolButton {
                visible: !root.containmentDrawsHeading && !!root.configureAction
                icon.name: "configure"
                text: root.configureAction ? root.configureAction.text : ""
                display: PlasmaComponents3.AbstractButton.IconOnly
                onClicked: root.configureAction.trigger()

                PlasmaComponents3.ToolTip {
                    text: parent.text
                }
            }
        }
    }

    contentItem: Item {
        PlasmaComponents3.ScrollView {
            id: scrollView

            anchors.fill: parent
            contentWidth: availableWidth
            visible: root.hasContent
            focus: true

            PlasmaComponents3.ScrollBar.horizontal.policy: PlasmaComponents3.ScrollBar.AlwaysOff

            Flickable {
                contentHeight: sections.implicitHeight
                contentWidth: scrollView.availableWidth
                flickableDirection: Flickable.VerticalFlick

                ColumnLayout {
                    id: sections

                    width: scrollView.availableWidth
                    spacing: 0

                    Kirigami.InlineMessage {
                        Layout.fillWidth: true
                        Layout.leftMargin: Kirigami.Units.largeSpacing
                        Layout.rightMargin: Kirigami.Units.largeSpacing
                        Layout.topMargin: Kirigami.Units.smallSpacing
                        type: Kirigami.MessageType.Error
                        text: root.errorMessage
                        visible: root.errorMessage.length > 0
                    }

                    Repeater {
                        model: root.providers

                        delegate: ProviderSection {
                            id: section

                            required property var modelData

                            Layout.fillWidth: true

                            provider: section.modelData
                            now: root.now
                            historyByWindow: root.historyFor(section.modelData.id)
                        }
                    }

                    Item {
                        Layout.fillWidth: true
                        Layout.preferredHeight: Kirigami.Units.largeSpacing
                    }
                }
            }
        }

        PlasmaExtras.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 4

            visible: !root.daemonRunning
            iconName: "state-offline"
            text: i18nc("@info:placeholder", "Token Station is not running")
            explanation: i18nc("@info:placeholder", "Start the service to see Claude Code and Codex usage.")

            helpfulAction: Kirigami.Action {
                // The bus name is D-Bus activatable, so any call starts the daemon.
                icon.name: "system-run"
                text: i18nc("@action:button", "Start")
                onTriggered: root.refreshRequested()
            }
        }

        PlasmaExtras.PlaceholderMessage {
            anchors.centerIn: parent
            width: parent.width - Kirigami.Units.gridUnit * 4

            visible: root.daemonRunning && (!root.snapshot || root.loading)
            iconName: "view-refresh"
            text: i18nc("@info:placeholder", "Reading usage…")
        }
    }

    footer: PlasmaExtras.PlasmoidHeading {
        position: PlasmaComponents3.ToolBar.Footer
        visible: root.hasContent && !!root.snapshot

        contentItem: PlasmaExtras.DescriptiveLabel {
            text: root.snapshot
                ? i18nc("@info:status When the shown numbers were produced, e.g. 'Updated 1 min ago'",
                        "Updated %1", Formatters.timeAgo(root.snapshot.generatedAt, root.now))
                : ""
            textFormat: Text.PlainText
            elide: Text.ElideRight
        }
    }

    onNowChanged: {
        // Re-fetch at most once a minute while the popup is open.
        if (root.hasContent && root.now - root.lastHistoryFetch > 60) {
            root.lastHistoryFetch = root.now;
            root.loadHistory();
        }
    }

    Component.onCompleted: {
        if (root.hasContent) {
            root.lastHistoryFetch = root.now;
            root.loadHistory();
        }
    }
}
