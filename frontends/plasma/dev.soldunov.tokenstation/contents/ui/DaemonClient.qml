/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    The only place in the applet that talks D-Bus. Everything else renders the
    parsed `snapshot` object it exposes.

    Contract: docs/dbus-api.md.
*/
pragma ComponentBehavior: Bound

import QtQuick

import org.kde.plasma.workspace.dbus as DBus

Item {
    id: client

    readonly property string service: "dev.soldunov.TokenStation"
    readonly property string objectPath: "/dev/soldunov/TokenStation"
    readonly property string ifaceName: "dev.soldunov.TokenStation1"

    /*! Parsed Snapshot JSON, or null while nothing has arrived yet. */
    property var snapshot: null
    /*! Monotonic counter from the daemon; changes together with `snapshot`. */
    property int revision: 0
    /*! The daemon owns the bus name right now. */
    readonly property bool daemonRunning: serviceWatcher.registered
    /*! A Refresh() call is in flight. */
    property bool refreshing: false
    /*! Last transport or parse failure, empty when the last exchange succeeded. */
    property string lastError: ""

    visible: false
    implicitWidth: 0
    implicitHeight: 0

    function buildMessage(member, args, signature) {
        return {
            "service": client.service,
            "path": client.objectPath,
            "iface": client.ifaceName,
            "member": member,
            "arguments": args || [],
            "signature": signature || ""
        };
    }

    function errorText(reply) {
        if (!reply) {
            return i18n("The Token Station service did not answer.");
        }
        if (reply.error && reply.error.isValid && reply.error.message) {
            return reply.error.message;
        }
        return i18n("The Token Station service did not answer.");
    }

    /*!
       Ask the daemon to poll every provider now. The bus name is D-Bus activatable,
       so this also starts the daemon when it is not running.
    */
    function refresh() {
        if (client.refreshing) {
            return;
        }
        client.refreshing = true;
        DBus.SessionBus.asyncCall(buildMessage("Refresh"), reply => {
            client.refreshing = false;
            client.lastError = "";
            // The daemon emits PropertiesChanged on its own; this only covers the
            // case where the applet started before the daemon did.
            properties.updateAll();
        }, reply => {
            client.refreshing = false;
            client.lastError = client.errorText(reply);
        });
    }

    /*!
       Recorded samples for one usage window.
       `callback(points, errorMessage)` receives `[[unixSeconds, percent], …]`.
    */
    function history(provider, windowId, since, callback) {
        // The signature is deduced from the DBus.uint64 wrapper; an explicit one
        // has to be the parenthesised argument list, which is easy to get wrong.
        const message = buildMessage("GetHistory", [String(provider), String(windowId), new DBus.uint64(since)]);
        DBus.SessionBus.asyncCall(message, reply => {
            let points = [];
            try {
                points = JSON.parse(String(reply.value));
            } catch (error) {
                callback([], i18n("The usage history could not be read."));
                return;
            }
            callback(Array.isArray(points) ? points : [], "");
        }, reply => {
            callback([], client.errorText(reply));
        });
    }

    /*! `callback(settings, errorMessage)` with the daemon config as a plain object. */
    function settings(callback) {
        DBus.SessionBus.asyncCall(buildMessage("GetSettings"), reply => {
            let parsed = null;
            try {
                parsed = JSON.parse(String(reply.value));
            } catch (error) {
                callback(null, i18n("The Token Station settings could not be read."));
                return;
            }
            callback(parsed, "");
        }, reply => {
            callback(null, client.errorText(reply));
        });
    }

    /*!
       Replace the daemon config. The daemon rejects unknown keys, so always send a
       whole object obtained from settings() with the wanted fields changed.
       `callback(errorMessage)` is called with an empty string on success.
    */
    function setSettings(newSettings, callback) {
        const message = buildMessage("SetSettings", [JSON.stringify(newSettings)]);
        DBus.SessionBus.asyncCall(message, reply => {
            if (callback) {
                callback("");
            }
        }, reply => {
            if (callback) {
                callback(client.errorText(reply));
            }
        });
    }

    function applySnapshot() {
        const raw = properties.properties.Snapshot;
        if (raw === undefined || raw === null || String(raw).length === 0) {
            return;
        }
        try {
            const parsed = JSON.parse(String(raw));
            client.snapshot = parsed;
            const revision = properties.properties.Revision;
            client.revision = Number(revision !== undefined && revision !== null ? revision : (parsed.revision || 0));
            client.lastError = "";
        } catch (error) {
            client.lastError = i18n("The Token Station service sent an unreadable snapshot.");
        }
    }

    DBus.DBusServiceWatcher {
        id: serviceWatcher

        busType: DBus.BusType.Session
        watchedService: client.service

        onRegisteredChanged: {
            if (registered) {
                properties.updateAll();
            } else {
                client.snapshot = null;
                client.revision = 0;
                client.refreshing = false;
            }
        }
    }

    DBus.Properties {
        id: properties

        busType: DBus.BusType.Session
        service: client.service
        path: client.objectPath
        iface: client.ifaceName

        onRefreshed: client.applySnapshot()
        onPropertiesChanged: client.applySnapshot()
    }
}
