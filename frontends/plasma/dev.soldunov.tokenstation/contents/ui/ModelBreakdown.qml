/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Collapsible per-model token and cost list (`provider.tokens.byModel`).

    Uses the "expand"/"collapse" icon pair that PlasmaExtras.ExpandableListItem
    uses for the same gesture, on a flat PlasmaComponents3.ToolButton.
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
    property bool showModels: false

    // A `var` property holding a JS array arrives back as a QVariantList, which
    // Array.isArray() rejects; length is the portable check.
    readonly property bool hasModels: !!models && models.length > 0

    visible: hasModels
    spacing: 0

    PlasmaComponents3.ToolButton {
        // Left-aligned so the disclosure lines up with the section body.
        Layout.alignment: Qt.AlignLeft
        Layout.leftMargin: -Kirigami.Units.smallSpacing

        checkable: true
        checked: root.showModels
        flat: true
        display: PlasmaComponents3.AbstractButton.TextBesideIcon
        icon.name: root.showModels ? "collapse" : "expand"
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
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }

                PlasmaExtras.DescriptiveLabel {
                    Layout.alignment: Qt.AlignRight
                    text: i18nc("@info Tokens and money spent on one model, e.g. '272.7 M · $201.33'", "%1 · %2",
                                Formatters.tokens(modelRow.modelData.total),
                                Formatters.currency(modelRow.modelData.costUsd, "USD"))
                    textFormat: Text.PlainText
                    font: Kirigami.Theme.smallFont
                }
            }
        }
    }
}
