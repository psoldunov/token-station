/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Drives DaemonClient against a running daemon and prints what came back, so
    the D-Bus half can be checked without a Plasma shell.

    Run through dbus-smoke.sh, which starts `token-station daemon --fixture`.
*/
pragma ComponentBehavior: Bound

import QtQuick

import "../dev.soldunov.tokenstation/contents/ui" as TokenStation

Item {
    id: smoke

    property int pending: 3
    property int failures: 0

    function report(label, ok, detail) {
        console.warn((ok ? "ok   " : "FAIL ") + label + (detail ? ": " + detail : ""));
        if (!ok) {
            smoke.failures += 1;
        }
    }

    function finish() {
        smoke.pending -= 1;
        if (smoke.pending <= 0) {
            console.warn(smoke.failures === 0 ? "ALL CHECKS PASSED" : smoke.failures + " CHECK(S) FAILED");
            Qt.exit(smoke.failures === 0 ? 0 : 1);
        }
    }

    TokenStation.DaemonClient {
        id: client
    }

    Timer {
        // Give Properties.updateAll() a moment to round-trip.
        interval: 2000
        running: true
        repeat: false

        onTriggered: {
            smoke.report("service registered", client.daemonRunning);
            const snapshot = client.snapshot;
            smoke.report("snapshot parsed", !!snapshot && snapshot.schemaVersion === 1,
                         snapshot ? "revision " + client.revision : String(client.lastError));
            smoke.report("providers present", !!snapshot && !!snapshot.providers && snapshot.providers.length > 0,
                         snapshot && snapshot.providers ? snapshot.providers.map(p => p.id + "=" + p.state).join(",") : "");
            smoke.report("meter bars present", !!snapshot && !!snapshot.meter && snapshot.meter.bars.length > 0,
                         snapshot && snapshot.meter ? JSON.stringify(snapshot.meter.bars) : "");
            smoke.finish();

            client.settings((settings, error) => {
                smoke.report("GetSettings", error.length === 0 && !!settings && !!settings.general,
                             error.length > 0 ? error : "limits_interval_secs=" + settings.general.limits_interval_secs);
                smoke.finish();
            });

            const provider = snapshot && snapshot.providers.length > 0 ? snapshot.providers[0] : null;
            const usageWindow = provider && provider.windows.length > 0 ? provider.windows[0] : null;
            if (!usageWindow) {
                smoke.report("GetHistory", false, "no window to ask about");
                smoke.finish();
                return;
            }
            client.history(provider.id, usageWindow.id, Math.floor(Date.now() / 1000) - 86400, (points, error) => {
                smoke.report("GetHistory", error.length === 0, error.length > 0 ? error : points.length + " point(s)");
                smoke.finish();
            });
        }
    }

    Timer {
        interval: 20000
        running: true
        repeat: false
        onTriggered: {
            console.warn("FAIL timed out waiting for the daemon");
            Qt.exit(1);
        }
    }
}
