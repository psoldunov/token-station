// Number, duration and money formatting for the panel and the menu.

import {gettext as _, ngettext} from 'resource:///org/gnome/shell/extensions/extension.js';

/** Narrow no-break space: keeps "43.3 M" and "34 %" on one line. */
const NNBSP = ' ';

const MINUTE = 60;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

/**
 * A number the way the session's locale writes it: its own digits, its own
 * decimal mark and its own grouping.
 *
 * @param {number} value Any finite number.
 * @param {number} digits Fraction digits, fixed.
 * @returns {string} Formatted number.
 */
export function formatNumber(value, digits = 0) {
    if (!Number.isFinite(value))
        return '—';
    return value.toLocaleString(undefined, {
        minimumFractionDigits: digits,
        maximumFractionDigits: digits,
    });
}

/** Unix seconds, as the daemon counts them. */
export function now() {
    return Math.floor(Date.now() / 1000);
}

/**
 * Short count with an SI-ish suffix: 43302460 becomes "43.3 M".
 *
 * @param {number} value Any finite number; negatives keep their sign.
 * @returns {string} Formatted count.
 */
export function formatCount(value) {
    if (!Number.isFinite(value))
        return '—';
    const sign = value < 0 ? '-' : '';
    const n = Math.abs(value);
    const scale = (divisor, suffix) => {
        const scaled = n / divisor;
        // One decimal while it still carries information; whole units above.
        const digits = scaled < 100 ? 1 : 0;
        return `${sign}${formatNumber(scaled, digits)}${NNBSP}${suffix}`;
    };
    if (n >= 1e12)
        return scale(1e12, _('T'));
    if (n >= 1e9)
        return scale(1e9, _('B'));
    if (n >= 1e6)
        return scale(1e6, _('M'));
    if (n >= 1e3)
        return scale(1e3, _('k'));
    return `${sign}${formatNumber(Math.round(n))}`;
}

/**
 * Money in the daemon's currency, falling back to a bare number when the
 * currency code is missing or unknown to ICU.
 *
 * @param {number} value Amount.
 * @param {string|null} currency ISO 4217 code, e.g. "USD".
 * @returns {string} Formatted amount.
 */
export function formatCurrency(value, currency) {
    if (!Number.isFinite(value))
        return '—';
    if (typeof currency === 'string' && /^[A-Za-z]{3}$/.test(currency)) {
        try {
            return new Intl.NumberFormat(undefined, {
                style: 'currency',
                currency: currency.toUpperCase(),
            }).format(value);
        } catch {
            // Fall through to the plain number.
        }
    }
    return formatNumber(value, 2);
}

/**
 * Percentage as GNOME writes it.
 *
 * @param {number|null} percent 0..100, or null when unknown.
 * @returns {string} Formatted percentage.
 */
export function formatPercent(percent) {
    if (!Number.isFinite(percent))
        return '—';
    return `${formatNumber(Math.round(percent))}${NNBSP}%`;
}

/**
 * Coarse duration: at most two units, largest first.
 *
 * @param {number} seconds Duration; anything below a minute collapses.
 * @returns {string} Formatted duration.
 */
export function formatDuration(seconds) {
    if (!Number.isFinite(seconds) || seconds < MINUTE)
        return _('under a minute');
    if (seconds < HOUR) {
        const minutes = Math.floor(seconds / MINUTE);
        return ngettext('%d min', '%d min', minutes).format(minutes);
    }
    if (seconds < DAY) {
        const hours = Math.floor(seconds / HOUR);
        const minutes = Math.floor((seconds % HOUR) / MINUTE);
        if (minutes === 0)
            return ngettext('%d h', '%d h', hours).format(hours);
        return _('%d h %d min').format(hours, minutes);
    }
    const days = Math.floor(seconds / DAY);
    const hours = Math.floor((seconds % DAY) / HOUR);
    if (hours === 0)
        return ngettext('%d day', '%d days', days).format(days);
    return _('%d d %d h').format(days, hours);
}

/**
 * Live countdown to a reset.
 *
 * @param {number|null} resetsAt Unix seconds, or null when the daemon has none.
 * @returns {string|null} "resets in 2 h 14 min", or null when there is nothing to say.
 */
export function formatResetsIn(resetsAt) {
    if (!Number.isFinite(resetsAt))
        return null;
    const remaining = resetsAt - now();
    if (remaining <= 0)
        return _('resetting now');
    return _('resets in %s').format(formatDuration(remaining));
}

/**
 * How long ago something happened.
 *
 * @param {number|null} timestamp Unix seconds.
 * @returns {string} "1 min ago", "just now", "—".
 */
export function formatAgo(timestamp) {
    if (!Number.isFinite(timestamp))
        return '—';
    const elapsed = now() - timestamp;
    if (elapsed < MINUTE)
        return _('just now');
    return _('%s ago').format(formatDuration(elapsed));
}

/**
 * One token summary line, e.g. "today 43.3 M tokens · $31.42".
 *
 * @param {string} prefix Leading label, already translated.
 * @param {object|null} usage A `TokenUsage` from the snapshot.
 * @returns {string|null} The line, or null when there is no usage.
 */
export function formatTokenUsage(prefix, usage) {
    if (usage === null || typeof usage !== 'object')
        return null;
    const total = Number(usage.total);
    if (!Number.isFinite(total))
        return null;
    const parts = [_('%s %s tokens').format(prefix, formatCount(total))];
    const cost = Number(usage.costUsd);
    if (Number.isFinite(cost) && cost > 0)
        parts.push(formatCurrency(cost, 'USD'));
    return joinDots(parts);
}

/** Joins parts with GNOME's middle dot separator. */
export function joinDots(parts) {
    return parts.filter(p => typeof p === 'string' && p.length > 0).join(' · ');
}
