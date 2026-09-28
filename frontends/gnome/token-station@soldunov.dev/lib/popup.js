// Menu contents: one section per provider, plus the footer.

import Clutter from 'gi://Clutter';
import GLib from 'gi://GLib';
import St from 'gi://St';

import * as BarLevel from 'resource:///org/gnome/shell/ui/barLevel.js';
import * as PopupMenu from 'resource:///org/gnome/shell/ui/popupMenu.js';
import {gettext as _} from 'resource:///org/gnome/shell/extensions/extension.js';

import {Sparkline} from './sparkline.js';
import {
    formatAgo, formatCount, formatCurrency, formatPercent, formatResetsIn,
    formatTokenUsage, joinDots, now,
} from './format.js';

/** How often the "resets in …" countdowns tick while the menu is open. */
const COUNTDOWN_INTERVAL_SECONDS = 30;
/** Floor between two GetHistory rounds while the menu stays open. */
const HISTORY_MIN_INTERVAL_SECONDS = 60;
const DAY = 24 * 60 * 60;
/** History window for short-lived plan windows and for weekly ones. */
const HISTORY_SPAN = {session: DAY, weekly: 7 * DAY};

const STATE_ICONS = {
    loading: 'content-loading-symbolic',
    stale: 'dialog-warning-symbolic',
    unauthenticated: 'dialog-password-symbolic',
    not_installed: 'dialog-information-symbolic',
    disabled: 'dialog-information-symbolic',
    error: 'dialog-error-symbolic',
};

/** Text for a non-ok state the daemon left without a message. */
function stateFallbackMessage(state) {
    switch (state) {
    case 'loading':
        return _('Loading…');
    case 'disabled':
        return _('Turned off');
    case 'error':
        return _('Something went wrong');
    default:
        return null;
    }
}

/**
 * Secondary text, the way the Shell's own menu sections draw it: the theme's
 * own foreground, faded. Using opacity rather than a colour keeps light, dark
 * and high-contrast themes right without naming a single colour.
 *
 * @see data/theme/gnome-shell-sass/widgets/_calendar.scss, which fades
 *   `.world-clocks-timezone` to $card_insensitive_fg_color.
 */
const DIM_OPACITY = 150;

function dimLabel(text, params = {}) {
    return new St.Label({
        text,
        style_class: 'token-station-caption',
        opacity: DIM_OPACITY,
        x_expand: true,
        ...params,
    });
}

/** A full-width, non-interactive row holding a vertical stack of children. */
function stackItem(children, styleClass) {
    const item = new PopupMenu.PopupBaseMenuItem({
        reactive: false,
        can_focus: false,
        style_class: styleClass,
    });
    const box = new St.BoxLayout({
        vertical: true,
        x_expand: true,
        style_class: 'token-station-stack',
    });
    for (const child of children)
        box.add_child(child);
    item.add_child(box);
    return item;
}

/** A single dim line of text, as a menu row; the label stays addressable. */
function textItem(text) {
    const label = dimLabel(text);
    const item = stackItem([label], 'token-station-detail-item');
    item.tokenStationLabel = label;
    return item;
}

/** Joins parts the way a screen reader reads a list. */
function joinCommas(parts) {
    return parts.filter(p => typeof p === 'string' && p.length > 0).join(', ');
}

export class PopupContent {
    /**
     * @param {PopupMenu.PopupMenu} menu The indicator's menu.
     * @param {object} options Wiring.
     * @param {import('./daemonProxy.js').DaemonProxy} options.proxy Daemon client.
     * @param {Function} options.onOpenPreferences Called by the Settings item.
     */
    constructor(menu, {proxy, onOpenPreferences}) {
        this._menu = menu;
        this._proxy = proxy;
        this._onOpenPreferences = onOpenPreferences;
        this._snapshot = null;
        this._countdowns = [];
        this._sparklines = [];
        this._timeoutId = 0;
        this._historyGeneration = 0;
        this._lastHistoryFetch = 0;
        this._structure = null;
        this._updaters = [];
        this._destroyed = false;

        this._providerSection = new PopupMenu.PopupMenuSection();
        menu.addMenuItem(this._providerSection);

        menu.addMenuItem(new PopupMenu.PopupSeparatorMenuItem());

        this._refreshItem = new PopupMenu.PopupMenuItem(_('Refresh Now'));
        this._refreshItem.connect('activate', () => this._refresh());
        menu.addMenuItem(this._refreshItem);

        const settingsItem = new PopupMenu.PopupMenuItem(_('Settings'));
        settingsItem.connect('activate', () => this._onOpenPreferences());
        menu.addMenuItem(settingsItem);

        this._updatedLabel = dimLabel('');
        menu.addMenuItem(stackItem([this._updatedLabel], 'token-station-footer-item'));
    }

    async _refresh() {
        try {
            await this._proxy.refresh();
        } catch (e) {
            console.error(e, 'Token Station: refresh failed');
        }
    }

    /**
     * Take a new snapshot. Rows are updated where they are, so an open menu
     * keeps its focus and its open submenus; only a snapshot that changes which
     * rows exist rebuilds the section.
     *
     * @param {object|null} snapshot Latest snapshot, or null when the daemon
     *   is unreachable.
     */
    update(snapshot) {
        this._snapshot = snapshot;

        const structure = this._structureKey(snapshot);
        if (structure === this._structure) {
            for (const updater of this._updaters)
                updater(snapshot);
        } else {
            this._structure = structure;
            this._rebuild(snapshot);
        }

        // Always usable: with no daemon on the bus this is what starts it.
        this._refreshItem.setSensitive(true);
        this._syncCountdowns();
        this._syncFootnote();
        if (this._menu.isOpen)
            this._fetchHistory();
    }

    /**
     * Everything about a snapshot that decides which rows the menu holds. Two
     * snapshots with the same key differ only in values, which the updaters
     * registered during the build can write into the existing rows.
     *
     * @param {object|null} snapshot Latest snapshot.
     * @returns {string} Structural fingerprint.
     */
    _structureKey(snapshot) {
        if (snapshot === null)
            return 'no-daemon';
        const providers = snapshot.providers ?? [];
        if (providers.length === 0)
            return 'no-providers';
        return JSON.stringify(providers.map(provider => [
            provider.id ?? null,
            provider.name ?? null,
            provider.plan ?? null,
            provider.state ?? null,
            (provider.windows ?? []).map(w => [w.id ?? null, w.label ?? null]),
            this._creditsLine(provider.credits) !== null,
            this._breakdownLine(provider.breakdown) !== null,
            this._tokenLines(provider).length,
            (provider.tokens?.byModel ?? []).map(row => row.model ?? null),
            this._historyWindow(provider, snapshot)?.id ?? null,
        ]));
    }

    _rebuild(snapshot) {
        this._countdowns = [];
        this._sparklines = [];
        this._updaters = [];
        // Fresh sparklines have nothing in them, so the throttle starts over.
        this._lastHistoryFetch = 0;
        this._providerSection.removeAll();

        const providers = snapshot?.providers ?? [];
        if (providers.length === 0) {
            this._providerSection.addMenuItem(this._buildPlaceholder(snapshot));
            return;
        }
        for (let index = 0; index < providers.length; index++) {
            for (const item of this._buildProvider(providers[index], snapshot, index))
                this._providerSection.addMenuItem(item);
        }
    }

    /**
     * Register a value updater for one provider's rows.
     *
     * @param {number} index Position of the provider in `snapshot.providers`.
     * @param {Function} apply Called with the provider from the new snapshot.
     */
    _onProviderUpdate(index, apply) {
        this._updaters.push(snapshot => {
            const provider = (snapshot?.providers ?? [])[index];
            if (provider !== undefined)
                apply(provider, snapshot);
        });
    }

    _buildPlaceholder(snapshot) {
        const text = snapshot === null
            ? _('The Token Station service is not running.')
            : _('No providers are turned on.');
        const box = new St.BoxLayout({vertical: true, x_expand: true});
        box.add_child(new St.Label({
            text,
            style_class: 'token-station-placeholder',
            x_expand: true,
        }));
        if (snapshot === null) {
            box.add_child(dimLabel(
                _('Start it with: systemctl --user start token-station')));
        }
        return stackItem([box], 'token-station-placeholder-item');
    }

    _buildProvider(provider, snapshot, index) {
        const items = [];

        // A labelled separator is the Shell's own section header; appMenu.js
        // builds its "Open Windows" heading exactly this way.
        const header = new PopupMenu.PopupSeparatorMenuItem(
            provider.name ?? provider.id ?? _('Unknown'));
        if (provider.plan) {
            header.add_child(dimLabel(provider.plan, {
                x_expand: false,
                y_align: Clutter.ActorAlign.CENTER,
            }));
        }
        items.push(header);

        const stateItem = this._buildState(provider);
        if (stateItem !== null) {
            items.push(stateItem);
            this._onProviderUpdate(index, updated => {
                stateItem.tokenStationLabel.text = this._stateMessage(updated);
            });
        }

        const windows = provider.windows ?? [];
        for (let position = 0; position < windows.length; position++) {
            items.push(this._buildWindow(windows[position]));
            const entry = this._countdowns[this._countdowns.length - 1];
            this._onProviderUpdate(index, updated => {
                const window = (updated.windows ?? [])[position];
                if (window !== undefined)
                    this._applyWindow(entry, window);
            });
        }

        const creditsLine = this._creditsLine(provider.credits);
        if (creditsLine !== null) {
            const item = textItem(creditsLine);
            items.push(item);
            this._onProviderUpdate(index, updated => {
                item.tokenStationLabel.text =
                    this._creditsLine(updated.credits) ?? '';
            });
        }

        const breakdownLine = this._breakdownLine(provider.breakdown);
        if (breakdownLine !== null) {
            const item = textItem(breakdownLine);
            items.push(item);
            this._onProviderUpdate(index, updated => {
                item.tokenStationLabel.text =
                    this._breakdownLine(updated.breakdown) ?? '';
            });
        }

        const tokenItems = this._tokenLines(provider).map(line => textItem(line));
        for (const item of tokenItems)
            items.push(item);
        if (tokenItems.length > 0) {
            this._onProviderUpdate(index, updated => {
                const lines = this._tokenLines(updated);
                for (let position = 0; position < tokenItems.length; position++)
                    tokenItems[position].tokenStationLabel.text = lines[position] ?? '';
            });
        }

        const byModel = provider.tokens?.byModel ?? [];
        if (byModel.length > 0) {
            const {item, labels} = this._buildByModel(byModel);
            items.push(item);
            this._onProviderUpdate(index, updated => {
                const rows = updated.tokens?.byModel ?? [];
                for (let position = 0; position < labels.length; position++)
                    labels[position].text = this._modelDetail(rows[position]);
            });
        }

        const sparkline = this._buildSparkline(provider, snapshot);
        if (sparkline !== null) {
            items.push(sparkline);
            const entry = this._sparklines[this._sparklines.length - 1];
            this._onProviderUpdate(index, (updated, snapshotNow) => {
                const window = this._historyWindow(updated, snapshotNow);
                this._applyLevel(entry.sparkline, window?.level);
            });
        }

        return items;
    }

    /** What a non-ok provider says for itself. */
    _stateMessage(provider) {
        return provider.message ||
            stateFallbackMessage(provider.state) || provider.state;
    }

    _buildState(provider) {
        if (provider.state === 'ok')
            return null;
        const row = new St.BoxLayout({x_expand: true});
        row.add_child(new St.Icon({
            icon_name: STATE_ICONS[provider.state] ?? 'dialog-information-symbolic',
            style_class: 'token-station-state-icon',
            y_align: Clutter.ActorAlign.START,
        }));
        const label = new St.Label({
            text: this._stateMessage(provider),
            x_expand: true,
        });
        label.clutter_text.line_wrap = true;
        row.add_child(label);
        const item = stackItem([row], 'token-station-state-item');
        item.tokenStationLabel = label;
        if (provider.state === 'error' || provider.state === 'stale')
            item.add_style_class_name('token-station-warning');
        return item;
    }

    _buildWindow(window) {
        const topRow = new St.BoxLayout({x_expand: true});
        topRow.add_child(new St.Label({
            text: window.label ?? window.id ?? '',
            x_expand: true,
            y_align: Clutter.ActorAlign.CENTER,
        }));
        const detail = dimLabel('', {
            x_expand: false,
            y_align: Clutter.ActorAlign.CENTER,
        });
        topRow.add_child(detail);

        // `slider` is the Shell's own trough-and-fill style, the one the volume
        // and brightness sliders in Quick Settings use, so the normal fill is
        // the session's accent colour rather than a colour we picked.
        const level = new BarLevel.BarLevel({
            style_class: 'slider token-station-level',
            value: 0,
            maximumValue: 1,
            x_expand: true,
        });
        const item = stackItem([topRow, level], 'token-station-window-item');
        const entry = {label: detail, level, item, window};
        this._countdowns.push(entry);
        this._applyWindow(entry, window);
        return item;
    }

    /**
     * Put an actor into exactly one of the warning/critical style classes.
     *
     * @param {St.Widget} actor Actor carrying the level styling.
     * @param {string|undefined} level `warning`, `critical` or anything else.
     */
    _applyLevel(actor, level) {
        for (const name of ['token-station-warning', 'token-station-critical'])
            actor.remove_style_class_name(name);
        if (level === 'warning' || level === 'critical')
            actor.add_style_class_name(`token-station-${level}`);
    }

    /** Write one window's numbers into the row built for it. */
    _applyWindow(entry, window) {
        entry.window = window;
        const percent = Number(window.usedPercent);
        entry.level.value = Number.isFinite(percent)
            ? Math.min(Math.max(percent, 0), 100) / 100 : 0;
        this._applyLevel(entry.level, window.level);
        this._applyCountdown(entry);
    }

    /**
     * The row's trailing text and, for a screen reader, the whole row read as
     * one line: "Session, 34 % used, resets in 2 h".
     *
     * @param {object} entry One `_countdowns` entry.
     */
    _applyCountdown(entry) {
        const {label, level, item, window} = entry;
        const percent = Number(window.usedPercent);
        const percentText = Number.isFinite(percent) ? formatPercent(percent) : null;
        const resetText = formatResetsIn(Number(window.resetsAt));
        label.text = joinDots([percentText, resetText]);

        const name = joinCommas([
            window.label ?? window.id ?? '',
            percentText === null ? null : _('%s used').format(percentText),
            resetText,
        ]);
        item.accessible_name = name;
        level.accessible_name = name;
    }

    _creditsLine(credits) {
        if (credits === null || credits === undefined)
            return null;
        const label = credits.label || _('Credits');
        const parts = [];
        if (credits.enabled && Number.isFinite(Number(credits.used))) {
            const used = formatCurrency(Number(credits.used), credits.currency);
            parts.push(Number.isFinite(Number(credits.limit))
                ? _('%s of %s').format(used,
                    formatCurrency(Number(credits.limit), credits.currency))
                : used);
        }
        if (credits.detail)
            parts.push(credits.detail);
        if (parts.length === 0) {
            // Money is only shown while extra usage is switched on, so say so
            // rather than leaving a balance that reads as nothing spent.
            if (credits.enabled)
                return null;
            parts.push(_('Turned off'));
        }
        return `${label}: ${joinDots(parts)}`;
    }

    _breakdownLine(breakdown) {
        if (!Array.isArray(breakdown) || breakdown.length === 0)
            return null;
        const parts = breakdown
            .filter(row => Number(row?.percent) > 0)
            .map(row => `${row.label ?? row.key} ${formatPercent(Number(row.percent))}`);
        if (parts.length === 0)
            return null;
        return joinDots(parts);
    }

    _tokenLines(provider) {
        const lines = [];
        const tokens = provider.tokens;
        if (tokens) {
            const today = formatTokenUsage(_('today'), tokens.today);
            if (today !== null)
                lines.push(joinDots([_('This device'), today]));
            const week = formatTokenUsage(_('7 days'), tokens.last7Days);
            if (week !== null)
                lines.push(week);
        }
        const account = provider.accountTokens;
        if (account) {
            const parts = [];
            if (Number.isFinite(Number(account.today))) {
                parts.push(_('today %s tokens')
                    .format(formatCount(Number(account.today))));
            }
            if (Number.isFinite(Number(account.last7Days))) {
                parts.push(_('7 days %s')
                    .format(formatCount(Number(account.last7Days))));
            }
            if (parts.length > 0)
                lines.push(joinDots([_('All devices'), ...parts]));
        }
        return lines;
    }

    /** "272.7 M · $201.33" for one per-model row. */
    _modelDetail(row) {
        if (row === null || row === undefined)
            return '';
        return joinDots([
            formatCount(Number(row.total)),
            Number(row.costUsd) > 0
                ? formatCurrency(Number(row.costUsd), 'USD') : null,
        ]);
    }

    _buildByModel(byModel) {
        const item = new PopupMenu.PopupSubMenuMenuItem(_('Per model'));
        const labels = [];
        for (const row of byModel) {
            const line = new St.BoxLayout({x_expand: true});
            line.add_child(new St.Label({
                text: row.model ?? _('Unknown'),
                x_expand: true,
                y_align: Clutter.ActorAlign.CENTER,
            }));
            const detail = dimLabel(this._modelDetail(row), {
                x_expand: false,
                y_align: Clutter.ActorAlign.CENTER,
            });
            line.add_child(detail);
            labels.push(detail);
            const modelItem = stackItem([line], 'token-station-model-item');
            modelItem.accessible_name = joinCommas([
                row.model ?? _('Unknown'), this._modelDetail(row),
            ]);
            item.menu.addMenuItem(modelItem);
        }
        return {item, labels};
    }

    _buildSparkline(provider, snapshot) {
        const window = this._historyWindow(provider, snapshot);
        if (window === null)
            return null;
        const sparkline = new Sparkline();
        this._applyLevel(sparkline, window.level);
        const caption = dimLabel(window.kind === 'weekly'
            ? _('%s · last 7 days').format(window.label ?? '')
            : _('%s · last 24 hours').format(window.label ?? ''));
        this._sparklines.push({
            sparkline,
            provider: provider.id,
            windowId: window.id,
            span: HISTORY_SPAN[window.kind === 'weekly' ? 'weekly' : 'session'],
        });
        return stackItem([caption, sparkline], 'token-station-chart-item');
    }

    /** The window the panel meter shows for this provider, else the first one. */
    _historyWindow(provider, snapshot) {
        const windows = provider.windows ?? [];
        if (windows.length === 0)
            return null;
        const bar = (snapshot?.meter?.bars ?? [])
            .find(b => b.provider === provider.id);
        return windows.find(w => w.id === bar?.windowId) ?? windows[0];
    }

    /** Called when the menu opens: start the countdown and pull history. */
    onOpened() {
        this._syncCountdowns();
        this._syncFootnote();
        this._fetchHistory(true);
        if (this._timeoutId !== 0)
            return;
        this._timeoutId = GLib.timeout_add_seconds(
            GLib.PRIORITY_DEFAULT, COUNTDOWN_INTERVAL_SECONDS, () => {
                this._syncCountdowns();
                this._syncFootnote();
                return GLib.SOURCE_CONTINUE;
            });
    }

    /** Called when the menu closes: stop ticking. */
    onClosed() {
        this._stopTimer();
    }

    _stopTimer() {
        if (this._timeoutId !== 0) {
            GLib.Source.remove(this._timeoutId);
            this._timeoutId = 0;
        }
    }

    _syncCountdowns() {
        for (const entry of this._countdowns)
            this._applyCountdown(entry);
    }

    _syncFootnote() {
        const generatedAt = Number(this._snapshot?.generatedAt);
        this._updatedLabel.text = Number.isFinite(generatedAt)
            ? _('Updated %s').format(formatAgo(generatedAt))
            : _('Waiting for the Token Station service…');
    }

    /**
     * Pull the sparkline history.
     *
     * @param {boolean} force Fetch even if the last fetch was recent. The menu
     *   opening forces one; a new snapshot while it is open does not, so a
     *   chatty daemon cannot turn into a stream of GetHistory calls.
     */
    _fetchHistory(force) {
        if (this._sparklines.length === 0)
            return;
        if (!force && now() - this._lastHistoryFetch < HISTORY_MIN_INTERVAL_SECONDS)
            return;
        this._lastHistoryFetch = now();
        const generation = ++this._historyGeneration;
        const since = now();
        for (const entry of this._sparklines) {
            this._proxy.getHistory(entry.provider, entry.windowId, since - entry.span)
                .then(points => {
                    if (this._destroyed || generation !== this._historyGeneration)
                        return;
                    entry.sparkline.setPoints(points);
                })
                .catch(e => console.error(
                    e, `Token Station: no history for ${entry.provider}`));
        }
    }

    destroy() {
        this._destroyed = true;
        this._stopTimer();
        this._countdowns = [];
        this._sparklines = [];
        this._updaters = [];
        this._structure = null;
        this._snapshot = null;
    }
}
