/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
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
