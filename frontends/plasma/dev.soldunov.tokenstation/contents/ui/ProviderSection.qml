/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Everything the popup shows about one provider.

    The heading is PlasmaExtras.ListSectionHeader with the plan as trailing
    content, the same component the device notifier uses to separate groups. The
    body rows follow powerdevil's BatteryItem: primary PlasmaComponents3.Label
    with a dimmed smallFont label beside it, no custom colours or frames.
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

    spacing: Kirigami.Units.smallSpacing

    PlasmaExtras.ListSectionHeader {
        Layout.fillWidth: true
        label: root.provider && root.provider.name ? root.provider.name : ""

        PlasmaExtras.DescriptiveLabel {
            visible: !!root.provider && !!root.provider.plan
            text: root.provider && root.provider.plan ? root.provider.plan : ""
            textFormat: Text.PlainText
        }
    }

    Kirigami.InlineMessage {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.largeSpacing
        Layout.rightMargin: Kirigami.Units.largeSpacing
        type: root.messageType(root.providerState)
        text: root.hasMessage ? root.provider.message : ""
        visible: root.hasMessage
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.largeSpacing
        Layout.rightMargin: Kirigami.Units.largeSpacing
        visible: root.windows.length === 0 && !root.hasMessage
        wrapMode: Text.WordWrap
        textFormat: Text.PlainText
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
            Layout.leftMargin: Kirigami.Units.largeSpacing
            Layout.rightMargin: Kirigami.Units.largeSpacing

            usageWindow: windowRow.modelData
            now: root.now
            historyPoints: root.historyByWindow[windowRow.modelData.id] !== undefined
                ? root.historyByWindow[windowRow.modelData.id]
                : []
        }
    }

    // Claude reports which surface consumed the session window.
    ColumnLayout {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.largeSpacing
        Layout.rightMargin: Kirigami.Units.largeSpacing
        Layout.topMargin: Kirigami.Units.smallSpacing

        visible: root.breakdown.length > 0
        spacing: 0

        PlasmaComponents3.Label {
            Layout.fillWidth: true
            text: i18nc("@title:group Which surfaces used the plan window", "Used by")
            textFormat: Text.PlainText
        }

        Repeater {
            model: root.breakdown

            delegate: RowLayout {
                id: breakdownRow

                required property var modelData

                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                PlasmaExtras.DescriptiveLabel {
                    Layout.fillWidth: true
                    text: breakdownRow.modelData.label
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                    elide: Text.ElideRight
                }

                PlasmaExtras.DescriptiveLabel {
                    Layout.alignment: Qt.AlignRight
                    text: Formatters.percent(breakdownRow.modelData.percent)
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                }
            }
        }
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.largeSpacing
        Layout.rightMargin: Kirigami.Units.largeSpacing
        Layout.topMargin: Kirigami.Units.smallSpacing

        visible: !!root.credits
        wrapMode: Text.WordWrap
        textFormat: Text.PlainText
        font: Kirigami.Theme.smallFont
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

    TokenSummary {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.largeSpacing
        Layout.rightMargin: Kirigami.Units.largeSpacing
        Layout.topMargin: Kirigami.Units.smallSpacing

        tokens: root.provider ? root.provider.tokens : null
        accountTokens: root.provider ? root.provider.accountTokens : null
    }
}
