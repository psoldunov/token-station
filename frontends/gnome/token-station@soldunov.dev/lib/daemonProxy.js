// D-Bus client for the Token Station daemon.
//
// Imports nothing from `resource:///org/gnome/shell/...` so that both the
// Shell side and the (separate) preferences process can use it.
//
// See docs/dbus-api.md: bus name `dev.soldunov.TokenStation`, one object,
// interface `dev.soldunov.TokenStation1`.

import Gio from 'gi://Gio';

export const BUS_NAME = 'dev.soldunov.TokenStation';
export const OBJECT_PATH = '/dev/soldunov/TokenStation';
export const INTERFACE_NAME = 'dev.soldunov.TokenStation1';

const INTERFACE_XML = `
<node>
  <interface name="dev.soldunov.TokenStation1">
    <method name="Refresh"/>
    <method name="GetHistory">
      <arg type="s" direction="in" name="provider"/>
      <arg type="s" direction="in" name="window_id"/>
      <arg type="t" direction="in" name="since"/>
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="GetSettings">
      <arg type="s" direction="out" name="json"/>
    </method>
    <method name="SetSettings">
      <arg type="s" direction="in" name="json"/>
    </method>
    <method name="IngestClaudeStatusline">
      <arg type="s" direction="in" name="json"/>
    </method>
    <property name="Snapshot" type="s" access="read"/>
    <property name="Revision" type="t" access="read"/>
  </interface>
</node>`;

/** Minimal synchronous emitter; GJS has no ESM-friendly one outside the Shell. */
class Emitter {
    constructor() {
        this._handlers = new Map();
        this._nextId = 1;
    }

    connect(name, callback) {
        const id = this._nextId++;
        if (!this._handlers.has(name))
            this._handlers.set(name, new Map());
        this._handlers.get(name).set(id, callback);
        return id;
    }

    disconnect(id) {
        for (const byId of this._handlers.values())
            byId.delete(id);
    }

    emit(name, ...args) {
        const byId = this._handlers.get(name);
        if (!byId)
            return;
        for (const callback of [...byId.values()]) {
            try {
                callback(this, ...args);
            } catch (e) {
                console.error(e, `Token Station: handler for "${name}" failed`);
            }
        }
    }

    disconnectAll() {
        this._handlers.clear();
    }
}

/**
 * Parse a snapshot payload. Never throws: a daemon that publishes garbage
 * must not take the panel down with it.
 *
 * @param {string} text JSON as published on the `Snapshot` property.
 * @returns {object|null} The snapshot, or null when it is unusable.
 */
export function parseSnapshot(text) {
    if (typeof text !== 'string' || text.length === 0)
        return null;
    let value;
    try {
        value = JSON.parse(text);
    } catch (e) {
        console.error(e, 'Token Station: cannot parse the snapshot');
        return null;
    }
    if (value === null || typeof value !== 'object' || !Array.isArray(value.providers)) {
        console.error('Token Station: snapshot has an unexpected shape');
        return null;
    }
    return value;
}

/**
 * Tracks the daemon: whether it is on the bus, its latest snapshot, and the
 * method calls the front end makes. Everything is asynchronous.
 *
 * Signals: `changed` (availability or snapshot moved).
 */
export class DaemonProxy extends Emitter {
    constructor() {
        super();
        this._proxy = null;
        this._propertiesChangedId = 0;
        this._watchId = 0;
        this._available = false;
        this._snapshot = null;
        this._destroyed = false;
    }

    /** True once the daemon owns the bus name and a snapshot has arrived. */
    get available() {
        return this._available;
    }

    /** Latest parsed snapshot, or null. */
    get snapshot() {
        return this._snapshot;
    }

    /** Start watching. Resolves once the proxy exists (or failed to build). */
    async start() {
        this._watchId = Gio.bus_watch_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameWatcherFlags.NONE,
            () => this._onNameAppeared(),
            () => this._onNameVanished());

        const Wrapper = Gio.DBusProxy.makeProxyWrapper(INTERFACE_XML);
        try {
            this._proxy = await new Promise((resolve, reject) => {
                new Wrapper(
                    Gio.DBus.session, BUS_NAME, OBJECT_PATH,
                    (proxy, error) => error ? reject(error) : resolve(proxy),
                    null,
                    // The bus name is activatable, so building the proxy also
                    // starts the daemon when it is not running yet.
                    Gio.DBusProxyFlags.NONE);
            });
        } catch (e) {
            console.error(e, 'Token Station: cannot reach the daemon');
            return;
        }
        if (this._destroyed) {
            this._proxy = null;
            return;
        }

        this._propertiesChangedId = this._proxy.connect(
            'g-properties-changed', () => this._readSnapshot());
        this._readSnapshot();
    }

    _onNameAppeared() {
        // The proxy caches properties; re-read once the owner is known.
        this._readSnapshot();
    }

    _onNameVanished() {
        if (this._available || this._snapshot !== null) {
            this._available = false;
            this._snapshot = null;
            this.emit('changed');
        }
    }

    _readSnapshot() {
        if (this._destroyed || this._proxy === null)
            return;
        const owned = this._proxy.g_name_owner !== null;
        const snapshot = owned ? parseSnapshot(this._proxy.Snapshot) : null;
        const available = owned && snapshot !== null;
        const revisionChanged =
            snapshot?.revision !== this._snapshot?.revision ||
            snapshot?.generatedAt !== this._snapshot?.generatedAt;
        if (available === this._available && !revisionChanged)
            return;
        this._available = available;
        this._snapshot = snapshot;
        this.emit('changed');
    }

    _call(method, args) {
        return new Promise((resolve, reject) => {
            if (this._proxy === null) {
                reject(new Error('the Token Station daemon is not reachable'));
                return;
            }
            this._proxy[method](...args, (result, error) =>
                error ? reject(error) : resolve(result));
        });
    }

    /** Ask every provider to refresh now. */
    async refresh() {
        await this._call('RefreshRemote', []);
    }

    /**
     * Recorded percentages for one window.
     *
     * @param {string} provider Provider id, `claude` or `codex`.
     * @param {string} windowId Window id within that provider.
     * @param {number} since Unix seconds; samples older than this are dropped.
     * @returns {Promise<Array<[number, number]>>} `[ts, percent]`, oldest first.
     */
    async getHistory(provider, windowId, since) {
        const [json] = await this._call('GetHistoryRemote',
            [provider, windowId, Math.max(0, Math.floor(since))]);
        let points;
        try {
            points = JSON.parse(json);
        } catch (e) {
            console.error(e, 'Token Station: cannot parse the history');
            return [];
        }
        if (!Array.isArray(points))
            return [];
        return points.filter(p =>
            Array.isArray(p) && p.length >= 2 &&
            Number.isFinite(p[0]) && Number.isFinite(p[1]));
    }

    /** Daemon configuration as a plain object (snake_case keys). */
    async getSettings() {
        const [json] = await this._call('GetSettingsRemote', []);
        return JSON.parse(json);
    }

    /**
     * Replace the daemon configuration.
     *
     * @param {object} settings Full configuration; missing keys take defaults.
     */
    async setSettings(settings) {
        await this._call('SetSettingsRemote', [JSON.stringify(settings)]);
    }

    destroy() {
        this._destroyed = true;
        if (this._watchId !== 0) {
            Gio.bus_unwatch_name(this._watchId);
            this._watchId = 0;
        }
        if (this._proxy !== null && this._propertiesChangedId !== 0)
            this._proxy.disconnect(this._propertiesChangedId);
        this._propertiesChangedId = 0;
        this._proxy = null;
        this._snapshot = null;
        this._available = false;
        this.disconnectAll();
    }
}
