// D-Bus client for the Token Station daemon.
//
// Imports nothing from `resource:///org/gnome/shell/...` so that both the
// Shell side and the (separate) preferences process can use it.
//
// See docs/dbus-api.md: bus name `dev.soldunov.TokenStation`, one object,
// interface `dev.soldunov.TokenStation1`.

import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

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

        await this._ensureProxy();
        // Building the proxy deliberately does not auto-start the daemon, so
        // ask the bus to activate it when nothing owns the name yet.
        if (this._proxy !== null && this._proxy.g_name_owner === null)
            await this._activate();
    }

    /**
     * Build the proxy if there is none. Never throws: a daemon that cannot be
     * reached must not take the panel down with it.
     *
     * @returns {Promise<boolean>} Whether a proxy exists afterwards.
     */
    async _ensureProxy() {
        if (this._destroyed)
            return false;
        if (this._proxy !== null)
            return true;

        const Wrapper = Gio.DBusProxy.makeProxyWrapper(INTERFACE_XML);
        let proxy;
        try {
            proxy = await new Promise((resolve, reject) => {
                new Wrapper(
                    Gio.DBus.session, BUS_NAME, OBJECT_PATH,
                    (built, error) => error ? reject(error) : resolve(built),
                    null,
                    // Activation is requested explicitly instead, so that a
                    // daemon that fails to start leaves a usable proxy behind
                    // rather than none at all.
                    Gio.DBusProxyFlags.DO_NOT_AUTO_START);
            });
        } catch (e) {
            console.error(e, 'Token Station: cannot reach the daemon');
            return false;
        }
        if (this._destroyed)
            return false;

        this._proxy = proxy;
        this._propertiesChangedId = this._proxy.connect(
            'g-properties-changed', () => this._readSnapshot());
        this._readSnapshot();
        return true;
    }

    /** Ask the bus to start the daemon from its D-Bus service file. */
    async _activate() {
        try {
            await Gio.DBus.session.call(
                'org.freedesktop.DBus', '/org/freedesktop/DBus',
                'org.freedesktop.DBus', 'StartServiceByName',
                new GLib.Variant('(su)', [BUS_NAME, 0]),
                null, Gio.DBusCallFlags.NONE, -1, null);
        } catch (e) {
            console.error(e, 'Token Station: cannot start the daemon');
        }
    }

    /**
     * Retry everything the first attempt may have missed: a proxy that never
     * got built, and a daemon that is not running.
     */
    async retry() {
        await this._ensureProxy();
        if (this._proxy !== null && this._proxy.g_name_owner === null)
            await this._activate();
    }

    _onNameAppeared() {
        // A proxy that failed to build while the daemon was down gets its
        // second chance here; otherwise just re-read the cached properties.
        if (this._proxy === null) {
            this._ensureProxy().catch(e =>
                console.error(e, 'Token Station: cannot reach the daemon'));
            return;
        }
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

    /** Ask every provider to refresh now, starting the daemon if it is down. */
    async refresh() {
        await this.retry();
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
