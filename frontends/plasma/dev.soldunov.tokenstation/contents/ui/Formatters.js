/*
    SPDX-FileCopyrightText: 2026 Philipp Soldunov <69530789+psoldunov@users.noreply.github.com>
    SPDX-License-Identifier: MIT

    Formatting helpers shared by every representation.

    Deliberately not a `.pragma library`: a shared library is evaluated outside
    any QML context and could not call i18n(), while this file is evaluated in
    the context of the component importing it, so the strings below are the
    session's own and never depend on start-up ordering. Numbers go through
    Qt.locale() for the same reason.
*/

function isNumber(value) {
    return typeof value === "number" && isFinite(value);
}

/*! The em dash Breeze uses wherever a reading is missing. */
function unknown() {
    return i18nc("@item:intext No value available", "—");
}

/*! A number with a fixed number of decimals, in the session's locale. */
function number(value, decimals) {
    return Number(value).toLocaleString(Qt.locale(), "f", decimals);
}

/*! "34%" for 34.0; the unknown placeholder for null/undefined. */
function percent(value, decimals) {
    if (!isNumber(value)) {
        return unknown();
    }
    var digits = isNumber(decimals) ? decimals : 0;
    return i18nc("@item:intext A percentage, %1 is the number", "%1%", number(value, digits));
}

/*!
   A coarse duration: at most the two largest non-zero units, so countdowns stay
   short enough for a tray tooltip ("2 h 14 min", "3 d 4 h", "45 s").
*/
function duration(seconds) {
    if (!isNumber(seconds) || seconds < 0) {
        return unknown();
    }
    var total = Math.round(seconds);
    if (total < 60) {
        return i18ncp("@item:intext Duration in seconds", "%1 s", "%1 s", total);
    }
    var minutes = Math.floor(total / 60);
    if (minutes < 60) {
        return i18ncp("@item:intext Duration in minutes", "%1 min", "%1 min", minutes);
    }
    var hours = Math.floor(minutes / 60);
    if (hours < 24) {
        var restMinutes = minutes % 60;
        if (restMinutes > 0) {
            return i18nc("@item:intext Duration in hours and minutes, %1 hours and %2 minutes",
                         "%1 h %2 min", hours, restMinutes);
        }
        return i18ncp("@item:intext Duration in hours", "%1 h", "%1 h", hours);
    }
    var days = Math.floor(hours / 24);
    var restHours = hours % 24;
    if (restHours > 0) {
        return i18nc("@item:intext Duration in days and hours, %1 days and %2 hours",
                     "%1 d %2 h", days, restHours);
    }
    return i18ncp("@item:intext Duration in days", "%1 d", "%1 d", days);
}

/*! "resets in 2 h 14 min", or just "now" once the window is due. */
function resetsIn(resetsAt, nowSeconds) {
    if (!isNumber(resetsAt)) {
        return "";
    }
    var remaining = resetsAt - nowSeconds;
    if (remaining <= 0) {
        return i18nc("@item:intext A window that resets right now", "now");
    }
    return i18nc("@item:intext When a usage window resets, %1 is a duration",
                 "resets in %1", duration(remaining));
}

/*! "1 min ago" for a Unix-second timestamp in the past. */
function timeAgo(timestamp, nowSeconds) {
    if (!isNumber(timestamp)) {
        return unknown();
    }
    var elapsed = nowSeconds - timestamp;
    if (elapsed < 45) {
        return i18nc("@item:intext Something that happened seconds ago", "just now");
    }
    return i18nc("@item:intext How long ago something happened, %1 is a duration",
                 "%1 ago", duration(elapsed));
}

/*! "43.3 M", "680.4 k", "412", each written the way the locale writes numbers. */
function tokens(value) {
    if (!isNumber(value)) {
        return unknown();
    }
    var abs = Math.abs(value);
    if (abs < 1000) {
        return number(Math.round(value), 0);
    }
    if (abs < 1e6) {
        return i18nc("@item:intext Token count in thousands, %1 is the number",
                     "%1 k", number(value / 1e3, 1));
    }
    if (abs < 1e9) {
        return i18nc("@item:intext Token count in millions, %1 is the number",
                     "%1 M", number(value / 1e6, 1));
    }
    return i18nc("@item:intext Token count in billions, %1 is the number",
                 "%1 B", number(value / 1e9, 2));
}

var CURRENCY_SYMBOLS = {
    USD: "$",
    EUR: "€",
    GBP: "£",
    JPY: "¥"
};

/*!
   Money in the session's locale: "$31.42" in English, "31,42 $" in French.
   Unknown currency codes are written out ("CHF 31.42").
*/
function currency(amount, code) {
    if (!isNumber(amount)) {
        return unknown();
    }
    var upper = String(code || "USD").toUpperCase();
    var symbol = CURRENCY_SYMBOLS[upper] ? CURRENCY_SYMBOLS[upper] : upper;
    // Sub-cent amounts say nothing at two decimals, and no locale's currency
    // format carries four, so those are written as a plain number plus symbol.
    if (amount !== 0 && Math.abs(amount) < 0.01) {
        return i18nc("@item:intext A tiny amount of money, %1 is the currency, %2 the number",
                     "%1 %2", symbol, number(amount, 4));
    }
    return Number(amount).toLocaleCurrencyString(Qt.locale(), symbol);
}

/*! Short label for a snapshot timestamp, used by the popup footer. */
function updatedAt(timestamp, nowSeconds) {
    return timeAgo(timestamp, nowSeconds);
}
