/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <philipp@theswisscheese.com>
    SPDX-License-Identifier: MIT

    Pure formatting helpers shared by every representation.

    This is a `.pragma library`, so it has no access to the QML context and cannot
    call i18n() itself. The applet calls setLabels() once at start-up with the
    translated unit strings; the English fallbacks below keep the offscreen render
    harness (which has no KLocalizedContext) working unchanged.
*/
.pragma library

var DEFAULT_LABELS = {
    day: "d",
    hour: "h",
    minute: "min",
    second: "s",
    thousand: "K",
    million: "M",
    billion: "B",
    // %1 is the already-formatted number.
    percent: "%1 %",
    now: "now",
    justNow: "just now",
    // %1 is an already-formatted duration such as "2 h 14 min".
    ago: "%1 ago",
    resetsIn: "resets in %1",
    unknown: "—"
};

var labels = DEFAULT_LABELS;

/*! Install translated unit strings. Missing keys keep their English default. */
function setLabels(overrides) {
    var merged = {};
    for (var key in DEFAULT_LABELS) {
        merged[key] = DEFAULT_LABELS[key];
    }
    for (var override in overrides) {
        if (overrides[override] !== undefined && overrides[override] !== null) {
            merged[override] = overrides[override];
        }
    }
    labels = merged;
}

function fill(pattern, value) {
    return String(pattern).replace("%1", value);
}

function isNumber(value) {
    return typeof value === "number" && isFinite(value);
}

/*! "34 %" for 34.0; the unknown placeholder for null/undefined. */
function percent(value, decimals) {
    if (!isNumber(value)) {
        return labels.unknown;
    }
    var digits = isNumber(decimals) ? decimals : 0;
    return fill(labels.percent, value.toFixed(digits));
}

/*!
   A coarse duration: at most the two largest non-zero units, so countdowns stay
   short enough for a tray tooltip ("2 h 14 min", "3 d 4 h", "45 s").
*/
function duration(seconds) {
    if (!isNumber(seconds) || seconds < 0) {
        return labels.unknown;
    }
    var total = Math.round(seconds);
    if (total < 60) {
        return total + " " + labels.second;
    }
    var minutes = Math.floor(total / 60);
    if (minutes < 60) {
        return minutes + " " + labels.minute;
    }
    var hours = Math.floor(minutes / 60);
    if (hours < 24) {
        var restMinutes = minutes % 60;
        var hourPart = hours + " " + labels.hour;
        return restMinutes > 0 ? hourPart + " " + restMinutes + " " + labels.minute : hourPart;
    }
    var days = Math.floor(hours / 24);
    var restHours = hours % 24;
    var dayPart = days + " " + labels.day;
    return restHours > 0 ? dayPart + " " + restHours + " " + labels.hour : dayPart;
}

/*! "resets in 2 h 14 min", or "resets in now" collapsed to just "now". */
function resetsIn(resetsAt, nowSeconds) {
    if (!isNumber(resetsAt)) {
        return "";
    }
    var remaining = resetsAt - nowSeconds;
    if (remaining <= 0) {
        return labels.now;
    }
    return fill(labels.resetsIn, duration(remaining));
}

/*! "1 min ago" for a Unix-second timestamp in the past. */
function timeAgo(timestamp, nowSeconds) {
    if (!isNumber(timestamp)) {
        return labels.unknown;
    }
    var elapsed = nowSeconds - timestamp;
    if (elapsed < 45) {
        return labels.justNow;
    }
    return fill(labels.ago, duration(elapsed));
}

/*! "43.3 M", "680.4 K", "412". */
function tokens(value) {
    if (!isNumber(value)) {
        return labels.unknown;
    }
    var abs = Math.abs(value);
    if (abs < 1000) {
        return String(Math.round(value));
    }
    if (abs < 1e6) {
        return (value / 1e3).toFixed(1) + " " + labels.thousand;
    }
    if (abs < 1e9) {
        return (value / 1e6).toFixed(1) + " " + labels.million;
    }
    return (value / 1e9).toFixed(2) + " " + labels.billion;
}

var CURRENCY_SYMBOLS = {
    USD: "$",
    EUR: "€",
    GBP: "£",
    JPY: "¥"
};

/*! "$31.42"; unknown currency codes fall back to "CHF 31.42". */
function currency(amount, code) {
    if (!isNumber(amount)) {
        return labels.unknown;
    }
    var upper = String(code || "USD").toUpperCase();
    var symbol = CURRENCY_SYMBOLS[upper];
    var digits = amount !== 0 && Math.abs(amount) < 0.01 ? 4 : 2;
    return symbol ? symbol + amount.toFixed(digits) : upper + " " + amount.toFixed(digits);
}

/*! Short label for a snapshot timestamp, used by the popup footer. */
function updatedAt(timestamp, nowSeconds) {
    return timeAgo(timestamp, nowSeconds);
}
