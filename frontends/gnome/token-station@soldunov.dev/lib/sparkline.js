// A small filled line chart of recorded percentages, drawn with Cairo.

import Cairo from 'gi://cairo';
import Clutter from 'gi://Clutter';
import GObject from 'gi://GObject';
import St from 'gi://St';

import {setColor, toRgba} from './draw.js';

const DEFAULT_HEIGHT = 32;
const LINE_WIDTH = 1.5;
const FILL_ALPHA = 0.18;
const BASELINE_ALPHA = 0.15;

export const Sparkline = GObject.registerClass(
class TokenStationSparkline extends St.DrawingArea {
    _init(params = {}) {
        super._init({
            style_class: 'token-station-sparkline',
            x_expand: true,
            height: DEFAULT_HEIGHT,
            ...params,
        });
        this._points = [];
        this._color = null;
    }

    /**
     * Replace the series.
     *
     * @param {Array<[number, number]>} points `[unixSeconds, percent]`, oldest
     *   first. Fewer than two points draws nothing.
     */
    setPoints(points) {
        this._points = Array.isArray(points) ? points : [];
        this.queue_repaint();
    }

    /** True when there is enough data to draw a line. */
    get hasData() {
        return this._points.length >= 2;
    }

    vfunc_style_changed() {
        this._color = this.get_theme_node().get_foreground_color();
        super.vfunc_style_changed();
    }

    vfunc_repaint() {
        const cr = this.get_context();
        try {
            const [width, height] = this.get_surface_size();
            if (width <= 0 || height <= 0)
                return;

            const rtl = this.get_text_direction() === Clutter.TextDirection.RTL;
            const inset = LINE_WIDTH;
            const top = inset;
            const bottom = height - inset;

            if (!this.hasData) {
                // Nothing recorded yet: a flat hint rather than empty space.
                setColor(cr, this._color, BASELINE_ALPHA);
                cr.setLineWidth(1);
                cr.moveTo(0, bottom + 0.5);
                cr.lineTo(width, bottom + 0.5);
                cr.stroke();
                return;
            }

            const times = this._points.map(p => p[0]);
            const first = times[0];
            const last = times[times.length - 1];
            const span = Math.max(last - first, 1);
            // Percentages are 0..100; keep the scale absolute so two providers
            // are comparable, but leave headroom above the highest sample.
            const peak = Math.max(...this._points.map(p => p[1]), 1);
            const scale = peak <= 100 ? 100 : peak;

            const xFor = ts => {
                const ratio = (ts - first) / span;
                return (rtl ? 1 - ratio : ratio) * width;
            };
            const yFor = percent => {
                const clamped = Math.min(Math.max(percent, 0), scale);
                return bottom - (bottom - top) * (clamped / scale);
            };

            const traceLine = () => {
                cr.moveTo(xFor(this._points[0][0]), yFor(this._points[0][1]));
                for (const [ts, percent] of this._points.slice(1))
                    cr.lineTo(xFor(ts), yFor(percent));
            };

            // Tint under the line, then draw the line over it.
            const [r, g, b, a] = toRgba(this._color);
            traceLine();
            cr.lineTo(xFor(last), bottom);
            cr.lineTo(xFor(first), bottom);
            cr.closePath();
            cr.setSourceRGBA(r, g, b, a * FILL_ALPHA);
            cr.fill();

            traceLine();
            cr.setLineWidth(LINE_WIDTH);
            cr.setLineJoin(Cairo.LineJoin.ROUND);
            cr.setLineCap(Cairo.LineCap.ROUND);
            setColor(cr, this._color);
            cr.stroke();
        } finally {
            cr.$dispose();
        }
    }
});
