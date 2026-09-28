// The top-bar button: the meter, an optional percentage, and the menu.

import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import St from 'gi://St';

import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import {gettext as _} from 'resource:///org/gnome/shell/extensions/extension.js';

import {MeterIcon} from './meterIcon.js';
import {PopupContent} from './popup.js';
import {formatPercent} from './format.js';

export const Indicator = GObject.registerClass(
class TokenStationIndicator extends PanelMenu.Button {
    /**
     * @param {object} options Wiring.
     * @param {import('./daemonProxy.js').DaemonProxy} options.proxy Daemon client.
     * @param {Gio.Settings} options.settings The extension's GSettings.
     * @param {Function} options.onOpenPreferences Opens the preferences window.
     */
    _init({proxy, settings, onOpenPreferences}) {
        super._init(0.5, _('Token Station'));

        this._proxy = proxy;
        this._settings = settings;

        const box = new St.BoxLayout({
            style_class: 'panel-status-menu-box token-station-panel-box',
            y_align: Clutter.ActorAlign.CENTER,
        });
        this._meter = new MeterIcon();
        box.add_child(this._meter);

        this._percentLabel = new St.Label({
            text: '',
            style_class: 'token-station-panel-percent',
            y_align: Clutter.ActorAlign.CENTER,
            visible: false,
        });
        box.add_child(this._percentLabel);
        this.add_child(box);

        this.menu.box.add_style_class_name('token-station-menu');
        this._content = new PopupContent(this.menu, {proxy, onOpenPreferences});

        this._openStateId = this.menu.connect('open-state-changed',
            (_menu, open) => open ? this._content.onOpened() : this._content.onClosed());
        this._proxyChangedId = proxy.connect('changed', () => this._sync());
        this._showPercentId = this._settings.connect('changed::show-percent',
            () => this._syncPanel(this._proxy.snapshot));

        this._sync();
    }

    _sync() {
        const snapshot = this._proxy.snapshot;
        this._syncPanel(snapshot);
        this._content.update(snapshot);
    }

    _syncPanel(snapshot) {
        const bars = snapshot?.meter?.bars ?? [];
        this._meter.setBars(bars.length > 0
            ? bars
            : [{percent: null, level: 'normal'}]);

        const level = snapshot?.meter?.level ?? 'normal';
        for (const name of ['token-station-warning', 'token-station-critical'])
            this._meter.remove_style_class_name(name);
        if (level === 'warning' || level === 'critical')
            this._meter.add_style_class_name(`token-station-${level}`);

        const percents = bars
            .map(bar => Number(bar.percent))
            .filter(value => Number.isFinite(value));
        const show = this._settings.get_boolean('show-percent') &&
            percents.length > 0;
        this._percentLabel.visible = show;
        if (show)
            this._percentLabel.text = formatPercent(Math.max(...percents));

        this._setAccessibleSummary(this._summary(snapshot));
    }

    /**
     * The headline numbers as one line: "Claude Code 96 %, Codex 34 %".
     *
     * @param {object|null} snapshot Latest snapshot.
     * @returns {string} Summary, or an empty string when there is nothing yet.
     */
    _summary(snapshot) {
        if (snapshot === null || snapshot === undefined)
            return _('the service is not running');
        const providers = snapshot.providers ?? [];
        const parts = [];
        for (const bar of snapshot.meter?.bars ?? []) {
            const provider = providers.find(p => p.id === bar.provider);
            const name = provider?.name ?? bar.provider ?? '';
            const percent = Number(bar.percent);
            if (!Number.isFinite(percent))
                continue;
            parts.push(`${name} ${formatPercent(percent)}`);
        }
        return parts.join(', ');
    }

    /**
     * Put the summary where a screen reader finds it: on the button's
     * description, or folded into its name when there is no Atk object to
     * describe (older Shell versions do not expose one from GJS).
     *
     * @param {string} summary Headline numbers.
     */
    _setAccessibleSummary(summary) {
        const title = _('Token Station');
        const accessible = typeof this.get_accessible === 'function'
            ? this.get_accessible() : null;
        if (accessible !== null && typeof accessible.set_description === 'function') {
            this.accessible_name = title;
            accessible.set_description(summary);
            return;
        }
        this.accessible_name = summary.length > 0
            ? `${title}: ${summary}` : title;
    }

    destroy() {
        if (this._openStateId) {
            this.menu.disconnect(this._openStateId);
            this._openStateId = 0;
        }
        if (this._proxyChangedId) {
            this._proxy.disconnect(this._proxyChangedId);
            this._proxyChangedId = 0;
        }
        if (this._showPercentId) {
            this._settings.disconnect(this._showPercentId);
            this._showPercentId = 0;
        }
        this._content?.destroy();
        this._content = null;
        super.destroy();
    }
});
