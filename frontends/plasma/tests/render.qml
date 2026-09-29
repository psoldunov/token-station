/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
    SPDX-License-Identifier: MIT

    Offscreen render harness: loads the applet's representations with a fixture
    snapshot injected and writes one PNG per view.

    Run through render.sh, which sets the QML import path and the colour scheme.
    Arguments after `--`: <appletUiDir> <outputDir> <fixture.json> [more fixtures…]
*/
pragma ComponentBehavior: Bound

import QtQuick

import org.kde.kirigami as Kirigami

Item {
    id: harness

    property string uiDir: ""
    property string outputDir: ""
    property var fixturePaths: []
    property string suffix: ""

    /*! One entry per PNG still to be produced. */
    property var queue: []
    property int queueIndex: 0

    width: 1
    height: 1

    function readFile(path) {
        const request = new XMLHttpRequest();
        request.open("GET", "file://" + path, false);
        request.send(null);
        return request.responseText;
    }

    /*!
       Fallback for i18n()/i18nc() when no KLocalizedContext is installed. A real
       context wins over these, because QML resolves context properties before the
       global object; they only keep the harness rendering if KI18n is absent.
    */
    function installI18nShims() {
        const globalObject = (new Function("return this"))();
        const substitute = (message, args) => {
            let text = String(message);
            for (let index = 0; index < args.length; ++index) {
                text = text.replace("%" + (index + 1), args[index]);
            }
            return text;
        };
        globalObject.i18n = (message, ...args) => substitute(message, args);
        globalObject.i18nc = (context, message, ...args) => substitute(message, args);
        globalObject.i18np = (singular, plural, ...args) =>
            substitute(args[0] === 1 ? singular : plural, args);
        globalObject.i18ncp = (context, singular, plural, ...args) =>
            substitute(args[0] === 1 ? singular : plural, args);
    }

    /*!
       Fixtures carry fixed timestamps. Shift them so the popup shows live
       countdowns instead of "now", exactly as the daemon's --fixture mode does.
    */
    function shiftTimestamps(snapshot) {
        const nowSeconds = Math.floor(Date.now() / 1000);
        const delta = nowSeconds - snapshot.generatedAt;
        const shift = value => typeof value === "number" ? value + delta : value;

        snapshot.generatedAt = nowSeconds;
        for (const provider of snapshot.providers || []) {
            provider.updatedAt = shift(provider.updatedAt);
            for (const usageWindow of provider.windows || []) {
                usageWindow.resetsAt = shift(usageWindow.resetsAt);
                usageWindow.observedAt = shift(usageWindow.observedAt);
            }
            if (provider.tokens) {
                provider.tokens.updatedAt = shift(provider.tokens.updatedAt);
            }
            if (provider.accountTokens) {
                provider.accountTokens.updatedAt = shift(provider.accountTokens.updatedAt);
            }
        }
        return snapshot;
    }

    /*! Synthetic history so the sparkline has something to draw. */
    function fakeHistory(spanSeconds, seed) {
        const points = [];
        const nowSeconds = Math.floor(Date.now() / 1000);
        let value = 5 + (seed % 7) * 3;
        for (let index = 0; index < 40; ++index) {
            value = Math.max(0, Math.min(100, value + ((index * 7 + seed) % 11) - 3));
            points.push([nowSeconds - spanSeconds + Math.round((index / 39) * spanSeconds), value]);
        }
        return points;
    }

    function buildQueue() {
        const jobs = [];
        for (const path of harness.fixturePaths) {
            const name = String(path).split("/").pop().replace(/\.json$/, "");
            const snapshot = harness.shiftTimestamps(JSON.parse(harness.readFile(path)));
            jobs.push({ "name": name, "view": "full", "snapshot": snapshot, "daemonRunning": true, "percentText": false });
            jobs.push({ "name": name, "view": "compact", "snapshot": snapshot, "daemonRunning": true, "percentText": false });
            jobs.push({ "name": name, "view": "compact-percent", "snapshot": snapshot, "daemonRunning": true, "percentText": true });
            // The per-model list starts collapsed in the popup, so render it on
            // its own to check the expanded state.
            const byModel = snapshot.providers[0].tokens ? snapshot.providers[0].tokens.byModel : null;
            if (byModel && byModel.length > 0) {
                jobs.push({ "name": name, "view": "models", "models": byModel });
            }
        }
        // The daemon-missing placeholder has no fixture of its own.
        jobs.push({ "name": "daemon-missing", "view": "full", "snapshot": null, "daemonRunning": false, "percentText": false });
        return jobs;
    }

    function runNext() {
        if (harness.queueIndex >= harness.queue.length) {
            Qt.callLater(Qt.quit);
            return;
        }
        const job = harness.queue[harness.queueIndex];
        harness.queueIndex += 1;

        const componentPath = job.view === "full" ? "FullRepresentation.qml"
                            : job.view === "models" ? "ModelBreakdown.qml"
                            : "CompactRepresentation.qml";
        const component = Qt.createComponent(harness.uiDir + "/" + componentPath, Component.PreferSynchronous);
        if (component.status !== Component.Ready) {
            console.warn("FAILED to load", componentPath, component.errorString());
            Qt.callLater(harness.runNext);
            return;
        }

        const properties = job.view === "models"
            ? {
                "models": job.models,
                "showModels": true,
                "width": 320
            }
            : job.view === "full"
            ? {
                "snapshot": job.snapshot,
                "daemonRunning": job.daemonRunning,
                "now": Math.floor(Date.now() / 1000),
                "width": 360,
                "height": 620
            }
            : {
                "snapshot": job.snapshot,
                "showPercentText": job.percentText,
                "width": job.percentText ? 64 : 32,
                "height": 32
            };

        // The real popup gets its background from the Plasma dialog; paint the
        // theme background here so dark-scheme text is not white on white.
        const backdrop = backdropComponent.createObject(stage, {});
        const item = component.createObject(backdrop, properties);
        if (!item) {
            backdrop.destroy();
            console.warn("FAILED to instantiate", componentPath);
            Qt.callLater(harness.runNext);
            return;
        }

        if (job.view === "full" && job.snapshot) {
            // Feed the sparklines without a bus.
            item.historyRequested.connect((provider, windowId, since, callback) => {
                callback(harness.fakeHistory(Math.floor(Date.now() / 1000) - since, windowId.length), "");
            });
            item.loadHistory();
        }

        const target = harness.outputDir + "/" + job.name + "-" + job.view + harness.suffix + ".png";
        // One frame for layout, one for the Canvas sparkline repaint.
        Qt.callLater(() => Qt.callLater(() => {
            backdrop.width = item.width;
            backdrop.height = item.height;
            const grabbed = backdrop.grabToImage(result => {
                if (!result.saveToFile(target)) {
                    console.warn("FAILED to save", target);
                } else {
                    console.warn("wrote", target);
                }
                backdrop.destroy();
                Qt.callLater(harness.runNext);
            }, Qt.size(backdrop.width, backdrop.height));
            if (!grabbed) {
                console.warn("FAILED to grab", target);
                backdrop.destroy();
                Qt.callLater(harness.runNext);
            }
        }));
    }

    Component {
        id: backdropComponent

        Rectangle {
            color: Kirigami.Theme.backgroundColor

            Kirigami.Theme.colorSet: Kirigami.Theme.Window
            Kirigami.Theme.inherit: false
        }
    }

    Item {
        id: stage

        width: 400
        height: 640
    }

    Component.onCompleted: {
        const arguments_ = Qt.application.arguments;
        const separator = arguments_.indexOf("--");
        const rest = separator >= 0 ? arguments_.slice(separator + 1) : arguments_.slice(2);
        if (rest.length < 3) {
            console.warn("usage: render.qml -- <uiDir> <outputDir> <suffix> <fixture.json…>");
            Qt.callLater(Qt.quit);
            return;
        }
        harness.uiDir = rest[0];
        harness.outputDir = rest[1];
        harness.suffix = rest[2] === "-" ? "" : "-" + rest[2];
        harness.fixturePaths = rest.slice(3);

        harness.installI18nShims();
        harness.queue = harness.buildQueue();
        harness.runNext();
    }
}
