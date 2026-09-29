/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
    SPDX-License-Identifier: MIT

    One page for both kinds of setting: the tray meter's own look (applet KConfig)
    and the daemon's behaviour (GetSettings/SetSettings over D-Bus).
*/
pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import QtQuick.Controls as QQC2

import org.kde.kcmutils as KCM
import org.kde.kirigami as Kirigami

KCM.SimpleKCM {
    id: page

    /*! Applet-local: badge the percentage on the meter. */
    property alias cfg_showPercentText: showPercentText.checked

    /*! Whole daemon config as last read, so unknown keys survive a write. */
    property var daemonSettings: null
    property bool settingsLoaded: false
    property string settingsError: ""

    readonly property int minimumLimitsInterval: 120

    signal configurationChanged

    function reloadSettings() {
        client.settings((settings, error) => {
            if (error.length > 0 || !settings) {
                page.settingsError = error.length > 0 ? error : i18nc("@info", "The Token Station settings could not be read.");
                page.settingsLoaded = false;
                return;
            }
            page.settingsError = "";
            page.daemonSettings = settings;
            meterWindowBox.currentIndex = meterWindowBox.indexOfValue(settings.meter ? settings.meter.window : "most_constrained");
            warningPercent.value = Math.round(settings.alerts ? settings.alerts.warning_percent : 80);
            criticalPercent.value = Math.round(settings.alerts ? settings.alerts.critical_percent : 95);
            notify.checked = settings.alerts ? settings.alerts.notify : true;
            notifyOnReset.checked = settings.alerts ? settings.alerts.notify_on_reset : false;
            claudeEnabled.checked = settings.claude ? settings.claude.enabled : true;
            codexEnabled.checked = settings.codex ? settings.codex.enabled : true;
            limitsInterval.value = Math.max(page.minimumLimitsInterval,
                                            settings.general ? settings.general.limits_interval_secs : 300);
            page.settingsLoaded = true;
        });
    }

    /*! Called by the Plasma config dialog when the user applies the page. */
    function saveConfig() {
        if (!page.settingsLoaded || !page.daemonSettings) {
            return;
        }
        // Deep copy so the daemon keeps every key it sent us; it rejects unknown ones.
        const next = JSON.parse(JSON.stringify(page.daemonSettings));
        next.meter = next.meter || {};
        next.meter.window = meterWindowBox.currentValue;
        next.alerts = next.alerts || {};
        next.alerts.warning_percent = warningPercent.value;
        next.alerts.critical_percent = criticalPercent.value;
        next.alerts.notify = notify.checked;
        next.alerts.notify_on_reset = notifyOnReset.checked;
        next.claude = next.claude || {};
        next.claude.enabled = claudeEnabled.checked;
        next.codex = next.codex || {};
        next.codex.enabled = codexEnabled.checked;
        next.general = next.general || {};
        next.general.limits_interval_secs = limitsInterval.value;

        client.setSettings(next, error => {
            page.settingsError = error;
            if (error.length === 0) {
                page.daemonSettings = next;
            }
        });
    }

    DaemonClient {
        id: client
    }

    Component.onCompleted: page.reloadSettings()

    header: ColumnLayout {
        spacing: 0

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Error
            text: page.settingsError
            visible: page.settingsError.length > 0
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Information
            text: i18nc("@info", "Waiting for the Token Station service. Its settings cannot be changed yet.")
            visible: !page.settingsLoaded && page.settingsError.length === 0
        }

        Kirigami.InlineMessage {
            Layout.fillWidth: true
            type: Kirigami.MessageType.Warning
            text: i18nc("@info", "The critical threshold must not be below the warning threshold.")
            visible: page.settingsLoaded && criticalPercent.value < warningPercent.value
        }
    }

    Kirigami.FormLayout {
        QQC2.ComboBox {
            id: meterWindowBox

            Kirigami.FormData.label: i18nc("@label:listbox", "Tray meter shows:")
            enabled: page.settingsLoaded
            textRole: "text"
            valueRole: "value"
            model: [
                {
                    "value": "most_constrained",
                    "text": i18nc("@item:inlistbox Meter window", "Most constrained window")
                },
                {
                    "value": "session",
                    "text": i18nc("@item:inlistbox Meter window", "Session window")
                },
                {
                    "value": "weekly",
                    "text": i18nc("@item:inlistbox Meter window", "Weekly window")
                }
            ]
            onActivated: page.configurationChanged()
        }

        QQC2.CheckBox {
            id: showPercentText

            Kirigami.FormData.label: i18nc("@label", "Tray meter:")
            text: i18nc("@option:check", "Show percentage on the meter")
        }

        Item {
            Kirigami.FormData.isSection: true
        }

        QQC2.SpinBox {
            id: warningPercent

            Kirigami.FormData.label: i18nc("@label:spinbox", "Warn at:")
            enabled: page.settingsLoaded
            from: 1
            to: 100
            stepSize: 1
            onValueModified: page.configurationChanged()
        }

        QQC2.SpinBox {
            id: criticalPercent

            Kirigami.FormData.label: i18nc("@label:spinbox", "Critical at:")
            enabled: page.settingsLoaded
            from: 1
            to: 100
            stepSize: 1
            onValueModified: page.configurationChanged()
        }

        QQC2.CheckBox {
            id: notify

            Kirigami.FormData.label: i18nc("@label", "Notifications:")
            enabled: page.settingsLoaded
            text: i18nc("@option:check", "Notify when a window crosses a threshold")
            onToggled: page.configurationChanged()
        }

        QQC2.CheckBox {
            id: notifyOnReset

            enabled: page.settingsLoaded
            text: i18nc("@option:check", "Notify when a window resets")
            onToggled: page.configurationChanged()
        }

        Item {
            Kirigami.FormData.isSection: true
        }

        QQC2.CheckBox {
            id: claudeEnabled

            Kirigami.FormData.label: i18nc("@label", "Providers:")
            enabled: page.settingsLoaded
            text: i18nc("@option:check", "Claude Code")
            onToggled: page.configurationChanged()
        }

        QQC2.CheckBox {
            id: codexEnabled

            enabled: page.settingsLoaded
            text: i18nc("@option:check", "Codex")
            onToggled: page.configurationChanged()
        }

        RowLayout {
            Kirigami.FormData.label: i18nc("@label:spinbox", "Check plan limits every:")
            spacing: Kirigami.Units.smallSpacing

            QQC2.SpinBox {
                id: limitsInterval

                enabled: page.settingsLoaded
                // The daemon refuses anything faster; the provider endpoints are rate limited.
                from: page.minimumLimitsInterval
                to: 24 * 60 * 60
                stepSize: 60
                onValueModified: page.configurationChanged()
            }

            QQC2.Label {
                text: i18ncp("@label Unit for the plan-limit poll interval", "second", "seconds", limitsInterval.value)
            }
        }
    }
}
