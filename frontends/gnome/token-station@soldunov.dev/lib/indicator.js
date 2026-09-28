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
