// Token Station: Claude Code and Codex plan usage in the GNOME top bar.
//
// Everything is created in enable() and torn down in disable(); the module
// body only declares.

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

import {DaemonProxy} from './lib/daemonProxy.js';
import {Indicator} from './lib/indicator.js';

export default class TokenStationExtension extends Extension {
    enable() {
        this._settings = this.getSettings();
        this._proxy = new DaemonProxy();
        this._indicator = new Indicator({
            proxy: this._proxy,
            settings: this._settings,
            onOpenPreferences: () => this.openPreferences(),
        });
        Main.panel.addToStatusArea(this.uuid, this._indicator, 0, 'right');

        // Connecting to the bus is asynchronous; the indicator already shows
        // its placeholder until the first snapshot arrives.
        this._proxy.start().catch(e =>
            console.error(e, 'Token Station: cannot start the D-Bus client'));
    }

    disable() {
        this._indicator?.destroy();
        this._indicator = null;
        this._proxy?.destroy();
        this._proxy = null;
        this._settings = null;
    }
}
