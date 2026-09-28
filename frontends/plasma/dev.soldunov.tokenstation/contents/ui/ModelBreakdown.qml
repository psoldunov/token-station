/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Collapsible per-model token and cost list (`provider.tokens.byModel`).
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

    /*! `provider.tokens.byModel`, largest first as the daemon sends it. */
    property var models: []

    readonly property bool hasModels: Array.isArray(models) && models.length > 0

    property bool showModels: false

    visible: hasModels
    spacing: 0

    PlasmaComponents3.ToolButton {
        // Left-aligned so the disclosure lines up with the section body.
        Layout.alignment: Qt.AlignLeft

        checkable: true
        checked: root.showModels
        flat: true
        display: PlasmaComponents3.AbstractButton.TextBesideIcon
        icon.name: root.showModels ? "collapse-all-symbolic" : "expand-all-symbolic"
        text: i18ncp("@action:button Expand the per-model token list",
                     "Per model (%1 model)", "Per model (%1 models)",
                     root.hasModels ? root.models.length : 0)
        onToggled: root.showModels = checked
    }

    ColumnLayout {
        Layout.fillWidth: true
        Layout.leftMargin: Kirigami.Units.gridUnit

        visible: root.showModels
        spacing: 0

        Repeater {
            model: root.hasModels ? root.models : []

            delegate: RowLayout {
                id: modelRow

                required property var modelData

                Layout.fillWidth: true
                spacing: Kirigami.Units.smallSpacing

                PlasmaExtras.DescriptiveLabel {
                    Layout.fillWidth: true
                    text: modelRow.modelData.model
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                PlasmaExtras.DescriptiveLabel {
                    text: i18nc("@info Tokens and money spent on one model, e.g. '272.7 M · $201.33'", "%1 · %2",
                                Formatters.tokens(modelRow.modelData.total),
                                Formatters.currency(modelRow.modelData.costUsd, "USD"))
                    horizontalAlignment: Text.AlignRight
                }
            }
        }
    }
}
