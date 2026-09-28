// Preferences. Panel options live in GSettings; everything else belongs to
// the daemon and travels over D-Bus (GetSettings/SetSettings).

import Adw from 'gi://Adw';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Gtk from 'gi://Gtk';

import {
    ExtensionPreferences, gettext as _,
} from 'resource:///org/gnome/Shell/Extensions/js/extensions/prefs.js';

import {DaemonProxy} from './lib/daemonProxy.js';

/** Matches ts-core's MIN_LIMITS_INTERVAL_SECS. */
const MIN_LIMITS_INTERVAL_SECONDS = 120;
/** A day; the same ceiling the Plasma applet's settings page offers. */
const MAX_LIMITS_INTERVAL_SECONDS = 24 * 60 * 60;
/** Coalesce keystrokes on the spin rows before writing back to the daemon. */
const WRITE_DEBOUNCE_MS = 400;

export default class TokenStationPreferences extends ExtensionPreferences {
    async fillPreferencesWindow(window) {
        const settings = this.getSettings();
        const page = new Adw.PreferencesPage({
            title: _('Token Station'),
            icon_name: 'dev.soldunov.TokenStation-symbolic',
        });
        window.add(page);

        this._window = window;
        this._proxy = new DaemonProxy();
        this._daemonSettings = null;
        this._applying = false;
        this._writeId = 0;
        this._rows = [];

        page.add(this._buildPanelGroup(settings));

        this._banner = new Adw.PreferencesGroup();
        this._bannerRow = new Adw.ActionRow({
            title: _('The Token Station service is not running'),
            subtitle: _('Provider and alert settings belong to the service.'),
        });
        this._bannerRow.add_prefix(new Gtk.Image({
            icon_name: 'dialog-warning-symbolic',
        }));
        this._banner.add(this._bannerRow);
        this._banner.visible = false;
        page.add(this._banner);

        page.add(this._buildProvidersGroup());
        page.add(this._buildAlertsGroup());

        window.connect('destroy', () => this._onDestroy());

        await this._loadDaemonSettings();
    }

    _buildPanelGroup(settings) {
        const group = new Adw.PreferencesGroup({
            title: _('Panel'),
            description: _('How the indicator looks in the top bar.'),
        });
        const row = new Adw.SwitchRow({
            title: _('Show percentage'),
            subtitle: _('Print the highest plan percentage next to the meter.'),
        });
        settings.bind('show-percent', row, 'active',
            Gio.SettingsBindFlags.DEFAULT);
        group.add(row);
        return group;
    }

    _buildProvidersGroup() {
        const group = new Adw.PreferencesGroup({
            title: _('Providers'),
            description: _('Stored by the Token Station service.'),
            sensitive: false,
        });

        this._claudeRow = this._switchRow(group, _('Claude Code'),
            _('Read plan limits and local usage for Claude Code.'),
            s => s.claude.enabled, (s, v) => {
                s.claude.enabled = v;
            });
        this._codexRow = this._switchRow(group, _('Codex'),
            _('Read plan limits and local usage for Codex.'),
            s => s.codex.enabled, (s, v) => {
                s.codex.enabled = v;
            });
        this._intervalRow = this._spinRow(group, _('Refresh interval'),
            _('Seconds between plan-limit checks.'),
            MIN_LIMITS_INTERVAL_SECONDS, MAX_LIMITS_INTERVAL_SECONDS, 30,
            s => s.general.limits_interval_secs, (s, v) => {
                s.general.limits_interval_secs = v;
            });

        this._providersGroup = group;
        return group;
    }

    _buildAlertsGroup() {
        const group = new Adw.PreferencesGroup({
            title: _('Alerts'),
            description: _('Stored by the Token Station service.'),
            sensitive: false,
        });

        this._warningRow = this._spinRow(group, _('Warning threshold'),
            _('Percent of a plan window that turns the meter yellow.'),
            1, 100, 1,
            s => s.alerts.warning_percent, (s, v) => {
                s.alerts.warning_percent = v;
            });
        this._criticalRow = this._spinRow(group, _('Critical threshold'),
            _('Percent of a plan window that turns the meter red.'),
            1, 100, 1,
            s => s.alerts.critical_percent, (s, v) => {
                s.alerts.critical_percent = v;
            });
        this._notifyRow = this._switchRow(group, _('Notifications'),
            _('Notify when a plan window crosses a threshold.'),
            s => s.alerts.notify, (s, v) => {
                s.alerts.notify = v;
            });
        this._notifyResetRow = this._switchRow(group, _('Notify on reset'),
            _('Notify when a plan window starts over.'),
            s => s.alerts.notify_on_reset, (s, v) => {
                s.alerts.notify_on_reset = v;
            });

        this._alertsGroup = group;
        return group;
    }

    _switchRow(group, title, subtitle, read, write) {
        const row = new Adw.SwitchRow({title, subtitle});
        group.add(row);
        this._rows.push({row, property: 'active', read, write});
        row.connect('notify::active', () => this._onRowChanged(row, 'active', write));
        return row;
    }

    _spinRow(group, title, subtitle, lower, upper, step, read, write) {
        const row = new Adw.SpinRow({
            title,
            subtitle,
            adjustment: new Gtk.Adjustment({
                lower, upper, step_increment: step, page_increment: step * 2,
            }),
        });
        group.add(row);
        this._rows.push({row, property: 'value', read, write});
        row.connect('notify::value', () => this._onRowChanged(row, 'value', write));
        return row;
    }

    async _loadDaemonSettings() {
        await this._proxy.start();
        try {
            this._daemonSettings = await this._proxy.getSettings();
        } catch (e) {
            console.error(e, 'Token Station: cannot read the service settings');
            this._banner.visible = true;
            return;
        }
        this._banner.visible = false;
        this._providersGroup.sensitive = true;
        this._alertsGroup.sensitive = true;
        this._applyToRows();
    }

    _applyToRows() {
        this._applying = true;
        try {
            for (const {row, property, read} of this._rows) {
                let value;
                try {
                    value = read(this._daemonSettings);
                } catch {
                    continue;
                }
                if (value !== undefined && value !== null)
                    row[property] = value;
            }
        } finally {
            this._applying = false;
        }
    }

    _onRowChanged(row, property, write) {
        if (this._applying || this._daemonSettings === null)
            return;
        // Intervals are unsigned integers on the daemon side, and whole
        // percentages are what the thresholds mean anyway.
        const value = property === 'value'
            ? Math.round(row[property]) : row[property];
        write(this._daemonSettings, value);
        this._scheduleWrite();
    }

    _scheduleWrite() {
        if (this._writeId !== 0)
            GLib.Source.remove(this._writeId);
        this._writeId = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT, WRITE_DEBOUNCE_MS, () => {
                this._writeId = 0;
                this._writeDaemonSettings();
                return GLib.SOURCE_REMOVE;
            });
    }

    async _writeDaemonSettings() {
        try {
            await this._proxy.setSettings(this._daemonSettings);
        } catch (e) {
            console.error(e, 'Token Station: cannot save the service settings');
            this._report(this._errorMessage(e));
            // Put the rows back in step with what the daemon actually holds.
            try {
                this._daemonSettings = await this._proxy.getSettings();
                this._applyToRows();
            } catch (readError) {
                console.error(readError,
                    'Token Station: cannot re-read the service settings');
            }
        }
    }

    /** D-Bus errors arrive prefixed with the remote error name; drop it. */
    _errorMessage(error) {
        const text = error?.message ?? String(error);
        const match = /^GDBus\.Error:[^:]*:\s*(.*)$/s.exec(text);
        return match ? match[1] : text;
    }

    _report(message) {
        if (typeof this._window?.add_toast === 'function') {
            this._window.add_toast(new Adw.Toast({
                title: message,
                timeout: 6,
            }));
            return;
        }
        this._bannerRow.title = _('The service rejected the change');
        this._bannerRow.subtitle = message;
        this._banner.visible = true;
    }

    _onDestroy() {
        if (this._writeId !== 0) {
            GLib.Source.remove(this._writeId);
            this._writeId = 0;
        }
        this._proxy?.destroy();
        this._proxy = null;
        this._rows = [];
    }
}
