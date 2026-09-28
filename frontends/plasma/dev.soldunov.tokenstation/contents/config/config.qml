/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT
*/
import QtQuick

import org.kde.plasma.configuration

ConfigModel {
    ConfigCategory {
        name: i18nc("@title Applet configuration page", "General")
        icon: "configure"
        source: "ConfigGeneral.qml"
    }
}
