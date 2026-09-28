// The panel glyph: one thin rounded vertical bar per provider, Claude first.

import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import St from 'gi://St';

import {roundedRect, setColor} from './draw.js';

const BAR_WIDTH = 4;
const BAR_GAP = 3;
const BAR_HEIGHT = 12;
/** Alpha of the unfilled part of a bar, against the panel foreground. */
const TRACK_ALPHA = 0.25;
/** Alpha of the outline drawn when a provider has no percentage yet. */
const OUTLINE_ALPHA = 0.5;

export const MeterIcon = GObject.registerClass(
class TokenStationMeterIcon extends St.DrawingArea {
    _init() {
        super._init({
            style_class: 'token-station-meter',
            y_align: Clutter.ActorAlign.CENTER,
            y_expand: true,
        });
        this._bars = [];
        this._normalColor = null;
        this._warningColor = null;
        this._criticalColor = null;
    }

    /**
     * Replace the bars.
     *
     * @param {Array<{percent: ?number, level: string}>} bars One entry per
     *   provider, in the order the daemon published them.
     */
    setBars(bars) {
        this._bars = Array.isArray(bars) ? bars : [];
        this.queue_relayout();
        this.queue_repaint();
    }

    vfunc_style_changed() {
        const themeNode = this.get_theme_node();
        this._normalColor = themeNode.get_foreground_color();
        this._warningColor = this._lookup(themeNode, '-token-station-warning-color');
        this._criticalColor = this._lookup(themeNode, '-token-station-critical-color');
        super.vfunc_style_changed();
    }

    /** Reads a custom colour property without warning when it is absent. */
    _lookup(themeNode, property) {
        const [found, color] = themeNode.lookup_color(property, false);
        return found ? color : null;
    }

    vfunc_get_preferred_width(_forHeight) {
        const count = Math.max(this._bars.length, 1);
        const width = count * BAR_WIDTH + (count - 1) * BAR_GAP;
        return this.get_theme_node().adjust_preferred_width(width, width);
    }

    vfunc_get_preferred_height(_forWidth) {
        const themeNode = this.get_theme_node();
        return themeNode.adjust_preferred_height(BAR_HEIGHT, BAR_HEIGHT);
    }

    _colorFor(level) {
        if (level === 'critical')
            return this._criticalColor ?? this._normalColor;
        if (level === 'warning')
            return this._warningColor ?? this._normalColor;
        return this._normalColor;
    }

    vfunc_repaint() {
        const cr = this.get_context();
        try {
            const [width, height] = this.get_surface_size();
            if (this._bars.length === 0)
                return;

            const count = this._bars.length;
            const totalWidth = count * BAR_WIDTH + (count - 1) * BAR_GAP;
            const left = Math.round((width - totalWidth) / 2);
            const top = Math.round((height - BAR_HEIGHT) / 2);
            const radius = BAR_WIDTH / 2;

            this._bars.forEach((bar, index) => {
                const x = left + index * (BAR_WIDTH + BAR_GAP);
                const percent = Number(bar?.percent);

                if (!Number.isFinite(percent)) {
                    // Unknown: an empty outline, inset by half a pixel so the
                    // 1px stroke lands on whole pixels.
                    roundedRect(cr, x + 0.5, top + 0.5,
                        BAR_WIDTH - 1, BAR_HEIGHT - 1, radius);
                    setColor(cr, this._normalColor, OUTLINE_ALPHA);
                    cr.setLineWidth(1);
                    cr.stroke();
                    return;
                }

                roundedRect(cr, x, top, BAR_WIDTH, BAR_HEIGHT, radius);
                setColor(cr, this._normalColor, TRACK_ALPHA);
                cr.fillPreserve();

                // Clip to the rounded track, then fill it from the bottom up
                // so the fill keeps the bar's rounded ends.
                const clamped = Math.min(Math.max(percent, 0), 100);
                const filled = Math.round(BAR_HEIGHT * clamped / 100);
                cr.save();
                cr.clip();
                if (filled > 0) {
                    cr.rectangle(x, top + BAR_HEIGHT - filled, BAR_WIDTH, filled);
                    setColor(cr, this._colorFor(bar?.level));
                    cr.fill();
                }
                cr.restore();
            });
        } finally {
            cr.$dispose();
        }
    }
});
