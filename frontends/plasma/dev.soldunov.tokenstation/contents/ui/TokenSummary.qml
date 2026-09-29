/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Token counters: this machine's local logs, plus the backend's all-device
    totals where the provider reports them.

    Dimmed smallFont labels, as powerdevil's BatteryItem uses for its detail rows.
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts

import org.kde.plasma.extras as PlasmaExtras
import org.kde.kirigami as Kirigami

import "Formatters.js" as Formatters

ColumnLayout {
    id: root

    /*! `provider.tokens`, or null when this machine has no logs for the provider. */
    property var tokens: null
    /*! `provider.accountTokens`, or null when the backend reports no totals. */
    property var accountTokens: null

    readonly property bool hasTokens: !!tokens
    readonly property bool hasAccountTokens: !!accountTokens

    visible: hasTokens || hasAccountTokens
    spacing: 0

    component DetailLabel: PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        font: Kirigami.Theme.smallFont
        textFormat: Text.PlainText
        wrapMode: Text.WordWrap
    }

    DetailLabel {
        visible: root.hasTokens && !!root.tokens.today
        text: root.hasTokens && root.tokens.today
            ? i18nc("@info Local token usage today, e.g. 'This device · today 43.3 M tokens · $31.42'",
                    "This device · today %1 tokens · %2",
                    Formatters.tokens(root.tokens.today.total),
                    Formatters.currency(root.tokens.today.costUsd, "USD"))
            : ""
    }

    DetailLabel {
        visible: root.hasTokens && !!root.tokens.last7Days
        text: root.hasTokens && root.tokens.last7Days
            ? i18nc("@info Local token usage over the last seven days",
                    "Last 7 days · %1 tokens · %2",
                    Formatters.tokens(root.tokens.last7Days.total),
                    Formatters.currency(root.tokens.last7Days.costUsd, "USD"))
            : ""
    }

    DetailLabel {
        visible: root.hasAccountTokens
        text: root.hasAccountTokens
            ? i18nc("@info Token usage the provider reports for the whole account",
                    "All devices · today %1 tokens · 7 days %2",
                    Formatters.tokens(root.accountTokens.today),
                    Formatters.tokens(root.accountTokens.last7Days))
            : ""
    }

    ModelBreakdown {
        Layout.fillWidth: true
        models: root.hasTokens && root.tokens.byModel ? root.tokens.byModel : []
    }
}
