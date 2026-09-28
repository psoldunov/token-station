/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Token counters: this machine's local logs, plus the backend's all-device totals
    where the provider reports them.
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

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        visible: root.hasTokens && !!root.tokens.today
        wrapMode: Text.WordWrap
        text: root.hasTokens && root.tokens.today
            ? i18nc("@info Local token usage today, e.g. 'This device · today 43.3 M tokens · $31.42'",
                    "This device · today %1 tokens · %2",
                    Formatters.tokens(root.tokens.today.total),
                    Formatters.currency(root.tokens.today.costUsd, "USD"))
            : ""
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        visible: root.hasTokens && !!root.tokens.last7Days
        wrapMode: Text.WordWrap
        text: root.hasTokens && root.tokens.last7Days
            ? i18nc("@info Local token usage over the last seven days",
                    "Last 7 days · %1 tokens · %2",
                    Formatters.tokens(root.tokens.last7Days.total),
                    Formatters.currency(root.tokens.last7Days.costUsd, "USD"))
            : ""
    }

    PlasmaExtras.DescriptiveLabel {
        Layout.fillWidth: true
        visible: root.hasAccountTokens
        wrapMode: Text.WordWrap
        text: root.hasAccountTokens
            ? i18nc("@info Token usage the provider reports for the whole account",
                    "All devices · today %1 tokens",
                    Formatters.tokens(root.accountTokens.today))
            : ""
    }

    ModelBreakdown {
        Layout.fillWidth: true
        Layout.topMargin: Kirigami.Units.smallSpacing
        models: root.hasTokens && root.tokens.byModel ? root.tokens.byModel : []
    }
}
