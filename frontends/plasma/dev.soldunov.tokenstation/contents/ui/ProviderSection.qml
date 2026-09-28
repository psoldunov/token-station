/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Everything the popup shows about one provider.
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

    /*! One entry of `snapshot.providers`. */
    required property var provider
    /*! Current Unix time in seconds, ticked by the popup. */
    required property real now
    /*! Window id to `[[unixSeconds, percent], …]`, filled in as GetHistory answers. */
    property var historyByWindow: ({})

    readonly property string providerState: provider && provider.state ? provider.state : "loading"
    readonly property bool hasMessage: !!provider && !!provider.message && providerState !== "ok"
    readonly property var windows: provider && provider.windows ? provider.windows : []
    readonly property var breakdown: provider && provider.breakdown ? provider.breakdown : []
    readonly property var credits: provider ? provider.credits : null

    spacing: Kirigami.Units.smallSpacing

    function messageType(state) {
        switch (state) {
        case "error":
            return Kirigami.MessageType.Error;
        case "unauthenticated":
        case "stale":
            return Kirigami.MessageType.Warning;
        default:
            return Kirigami.MessageType.Information;
        }
    }

    RowLayout {
        Layout.fillWidth: true
        spacing: Kirigami.Units.smallSpacing

        Kirigami.Heading {
            Layout.fillWidth: true
            level: 4
            text: root.provider && root.provider.name ? root.provider.name : ""
            elide: Text.ElideRight
            maximumLineCount: 1
        }

        // Plan chip, in the same spirit as the tag pills used across Plasma popups.
        Rectangle {
            visible: !!root.provider && !!root.provider.plan
            radius: height / 2
            color: Kirigami.Theme.alternateBackgroundColor
            implicitWidth: planLabel.implicitWidth + Kirigami.Units.smallSpacing * 3
            implicitHeight: planLabel.implicitHeight + Kirigami.Units.smallSpacing

            PlasmaComponents3.Label {
                id: planLabel

                anchors.centerIn: parent
                text: root.provider && root.provider.plan ? root.provider.plan : ""
                font: Kirigami.Theme.smallFont
                color: Kirigami.Theme.textColor
            }
        }
    }

    Kirigami.InlineMessage {
        Layout.fillWidth: true
        type: root.messageType(root.providerState)
        text: root.hasMessage ? root.provider.message : ""
        visible: root.hasMessage
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        visible: root.windows.length === 0 && !root.hasMessage
        wrapMode: Text.WordWrap
        text: root.providerState === "loading"
            ? i18nc("@info:status", "Reading plan limits…")
            : i18nc("@info:status", "No plan limits reported.")
    }

    Repeater {
        model: root.windows

        delegate: UsageWindowRow {
            id: windowRow

            required property var modelData

            Layout.fillWidth: true
            Layout.topMargin: Kirigami.Units.smallSpacing

            usageWindow: modelData
            now: root.now
            historyPoints: root.historyByWindow[modelData.id] !== undefined ? root.historyByWindow[modelData.id] : []
        }
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        visible: !!root.credits
        wrapMode: Text.WordWrap
        text: {
            if (!root.credits) {
                return "";
            }
            const label = root.credits.label ? root.credits.label : i18nc("@label Fallback name for extra paid usage", "Credits");
            if (typeof root.credits.used === "number") {
                const spent = Formatters.currency(root.credits.used, root.credits.currency);
                if (typeof root.credits.limit === "number") {
                    return i18nc("@info Credit spend against a cap, e.g. 'Extra usage · $4.20 of $50.00'",
                                 "%1 · %2 of %3", label, spent,
                                 Formatters.currency(root.credits.limit, root.credits.currency));
                }
                return i18nc("@info Credit spend with no cap", "%1 · %2", label, spent);
            }
            return root.credits.detail
                ? i18nc("@info Credit state described by the provider", "%1 · %2", label, root.credits.detail)
                : label;
        }
    }

    // Claude reports which surface consumed the session window.
    ColumnLayout {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        visible: root.breakdown.length > 0
        spacing: 0

        PlasmaExtras.DescriptiveLabel {
            Layout.fillWidth: true
            text: i18nc("@title:group Which surfaces used the plan window", "Used by")
        }

        Repeater {
            model: root.breakdown

            delegate: RowLayout {
                id: breakdownRow

                required property var modelData

                Layout.fillWidth: true
                Layout.leftMargin: Kirigami.Units.gridUnit
                spacing: Kirigami.Units.smallSpacing

                PlasmaExtras.DescriptiveLabel {
                    Layout.fillWidth: true
                    text: breakdownRow.modelData.label
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                PlasmaExtras.DescriptiveLabel {
                    text: Formatters.percent(breakdownRow.modelData.percent)
                    horizontalAlignment: Text.AlignRight
                }
            }
        }
    }

    TokenSummary {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing

        tokens: root.provider ? root.provider.tokens : null
        accountTokens: root.provider ? root.provider.accountTokens : null
    }
}
