// Shared Cairo helpers for the meter and the sparkline.

/**
 * Theme colours travel as Clutter.Color (byte components) up to GNOME 46 and
 * as Cogl.Color (which switched to floats along the way) after it. Normalise
 * to 0..1 so the same drawing code works across the supported Shell versions.
 *
 * @param {object} color A Clutter.Color or Cogl.Color.
 * @returns {[number, number, number, number]} Red, green, blue, alpha in 0..1.
 */
export function toRgba(color) {
    if (color === null || color === undefined)
        return [0.5, 0.5, 0.5, 1];
    const {red = 0, green = 0, blue = 0, alpha = 0} = color;
    const bytes = red > 1 || green > 1 || blue > 1 || alpha > 1;
    const scale = bytes ? 1 / 255 : 1;
    return [red * scale, green * scale, blue * scale, alpha * scale];
}

/**
 * Set the source colour, optionally dimmed.
 *
 * @param {object} cr Cairo context.
 * @param {object} color A Clutter.Color or Cogl.Color.
 * @param {number} [alphaFactor] Multiplier applied to the colour's own alpha.
 */
export function setColor(cr, color, alphaFactor = 1) {
    const [r, g, b, a] = toRgba(color);
    cr.setSourceRGBA(r, g, b, a * alphaFactor);
}

/**
 * Append a rounded rectangle to the current path.
 *
 * @param {object} cr Cairo context.
 * @param {number} x Left edge.
 * @param {number} y Top edge.
 * @param {number} width Width in pixels.
 * @param {number} height Height in pixels.
 * @param {number} radius Corner radius; clamped to half the shorter side.
 */
export function roundedRect(cr, x, y, width, height, radius) {
    const r = Math.max(0, Math.min(radius, width / 2, height / 2));
    const halfPi = Math.PI / 2;
    cr.newSubPath();
    cr.arc(x + width - r, y + r, r, -halfPi, 0);
    cr.arc(x + width - r, y + height - r, r, 0, halfPi);
    cr.arc(x + r, y + height - r, r, halfPi, Math.PI);
    cr.arc(x + r, y + r, r, Math.PI, 3 * halfPi);
    cr.closePath();
}
