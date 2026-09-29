//! The tray icon: a dual mini-meter drawn in pure Rust.
//!
//! One rounded vertical bar per provider (Claude first, then Codex), outlined in
//! the panel foreground and filled from the bottom to the used percentage. A bar
//! with no data keeps its outline and stays empty.

use ts_core::{Level, Meter, MeterBar};

use crate::tray::palette::{BarColors, ColorScheme, Rgba, bar_colors};

/// Pixmap sizes published to the `StatusNotifierItem` host.
pub const SIZES: [u32; 5] = [16, 22, 24, 32, 48];

/// Samples per axis; 4 × 4 per pixel is enough to hide the stair-steps at 16 px.
const SUBSAMPLES: u32 = 4;

/// One rendered icon: ARGB32, network byte order, `size` × `size`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pixmap {
    pub size: u32,
    pub argb: Vec<u8>,
}

/// What one bar should show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarSpec {
    /// 0–100, or `None` for "no data" (outline only).
    pub percent: Option<f64>,
    pub level: Level,
}

impl BarSpec {
    /// The fraction of the bar to fill, clamped to 0–1.
    fn fraction(self) -> f64 {
        self.percent.unwrap_or(0.0).clamp(0.0, 100.0) / 100.0
    }
}

/// The bars to draw for a meter: always at least two slots, so the icon keeps
/// its shape while the daemon is still loading.
#[must_use]
pub fn bar_specs(meter: Option<&Meter>) -> Vec<BarSpec> {
    let bars: &[MeterBar] = meter.map_or(&[], |m| m.bars.as_slice());
    if bars.is_empty() {
        return vec![
            BarSpec {
                percent: None,
                level: Level::Normal,
            };
            2
        ];
    }
    bars.iter()
        .map(|bar| BarSpec {
            percent: bar.percent,
            level: bar.level,
        })
        .collect()
}

/// An axis-aligned rectangle in pixel coordinates.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Rect {
    fn inset(self, by: f64) -> Rect {
        Rect {
            x0: self.x0 + by,
            y0: self.y0 + by,
            x1: self.x1 - by,
            y1: self.y1 - by,
        }
    }

    fn contains_round(self, x: f64, y: f64, radius: f64) -> bool {
        if x < self.x0 || x > self.x1 || y < self.y0 || y > self.y1 {
            return false;
        }
        let radius = radius.min((self.x1 - self.x0) / 2.0).max(0.0);
        let cx = x.clamp(self.x0 + radius, self.x1 - radius);
        let cy = y.clamp(self.y0 + radius, self.y1 - radius);
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy <= radius * radius
    }
}

/// Where the bars sit inside a `size` × `size` canvas.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    margin: f64,
    gap: f64,
    bar_width: f64,
    stroke: f64,
    size: f64,
}

fn geometry(size: u32, slots: usize) -> Geometry {
    let size = f64::from(size);
    let margin = size / 8.0;
    let gap = (size / 8.0).max(1.0);
    let slots = f64::from(u32::try_from(slots.max(1)).unwrap_or(u32::MAX));
    let inner = size - 2.0 * margin;
    let bar_width = ((inner - gap * (slots - 1.0)) / slots).max(1.0);
    Geometry {
        margin,
        gap,
        bar_width,
        stroke: (size / 16.0).max(1.0),
        size,
    }
}

impl Geometry {
    fn bar(self, index: usize) -> Rect {
        let index = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
        let x0 = self.margin + index * (self.bar_width + self.gap);
        Rect {
            x0,
            y0: self.margin,
            x1: x0 + self.bar_width,
            y1: self.size - self.margin,
        }
    }
}

/// Coverage of the outline ring and of the filled part at one sample point.
fn sample(geo: Geometry, bar: Rect, spec: BarSpec, x: f64, y: f64) -> (bool, bool) {
    let radius = geo.bar_width / 2.0;
    let inner = bar.inset(geo.stroke);
    let inside_outer = bar.contains_round(x, y, radius);
    let inside_inner = inner.contains_round(x, y, radius - geo.stroke);
    let fill_top = inner.y1 - (inner.y1 - inner.y0) * spec.fraction();
    let filled = inside_inner && spec.percent.is_some() && y >= fill_top;
    (inside_outer && !inside_inner, filled)
}

/// Blend disjoint ring and fill coverage into one straight-alpha pixel.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "every channel is rounded and clamped to 0.0..=255.0 before the cast"
)]
fn blend(colors: BarColors, ring: f64, fill: f64) -> Rgba {
    let weight = |color: Rgba, coverage: f64| f64::from(color.a) / 255.0 * coverage;
    let (wr, wf) = (weight(colors.track, ring), weight(colors.fill, fill));
    let alpha = (wr + wf).clamp(0.0, 1.0);
    if alpha <= f64::EPSILON {
        return Rgba {
            r: 0,
            g: 0,
            b: 0,
            a: 0,
        };
    }
    let channel = |track: u8, fill_value: u8| {
        ((f64::from(track) * wr + f64::from(fill_value) * wf) / alpha)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgba {
        r: channel(colors.track.r, colors.fill.r),
        g: channel(colors.track.g, colors.fill.g),
        b: channel(colors.track.b, colors.fill.b),
        a: (alpha * 255.0).round().clamp(0.0, 255.0) as u8,
    }
}

/// Render the meter at one size.
#[must_use]
pub fn render(size: u32, specs: &[BarSpec], scheme: ColorScheme) -> Pixmap {
    let geo = geometry(size, specs.len());
    let step = 1.0 / f64::from(SUBSAMPLES);
    let per_pixel = f64::from(SUBSAMPLES * SUBSAMPLES);
    let mut argb = vec![0u8; (size * size * 4) as usize];

    for (index, spec) in specs.iter().enumerate() {
        let bar = geo.bar(index);
        let colors = bar_colors(scheme, spec.level);
        for py in 0..size {
            for px in 0..size {
                let (mut ring, mut fill) = (0.0, 0.0);
                for sy in 0..SUBSAMPLES {
                    for sx in 0..SUBSAMPLES {
                        let x = f64::from(px) + (f64::from(sx) + 0.5) * step;
                        let y = f64::from(py) + (f64::from(sy) + 0.5) * step;
                        let (r, f) = sample(geo, bar, *spec, x, y);
                        ring += f64::from(u8::from(r));
                        fill += f64::from(u8::from(f));
                    }
                }
                if ring == 0.0 && fill == 0.0 {
                    continue;
                }
                let pixel = blend(colors, ring / per_pixel, fill / per_pixel);
                let at = ((py * size + px) * 4) as usize;
                if let Some(slot) = argb.get_mut(at..at + 4) {
                    slot.copy_from_slice(&[pixel.a, pixel.r, pixel.g, pixel.b]);
                }
            }
        }
    }
    Pixmap { size, argb }
}

/// Render every size the host may ask for.
#[must_use]
pub fn render_all(specs: &[BarSpec], scheme: ColorScheme) -> Vec<Pixmap> {
    SIZES
        .iter()
        .map(|size| render(*size, specs, scheme))
        .collect()
}

impl From<&Pixmap> for ksni::Icon {
    fn from(pixmap: &Pixmap) -> ksni::Icon {
        ksni::Icon {
            width: i32::try_from(pixmap.size).unwrap_or(i32::MAX),
            height: i32::try_from(pixmap.size).unwrap_or(i32::MAX),
            data: pixmap.argb.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ts_core::ProviderId;

    fn specs(values: &[(Option<f64>, Level)]) -> Vec<BarSpec> {
        values
            .iter()
            .map(|(percent, level)| BarSpec {
                percent: *percent,
                level: *level,
            })
            .collect()
    }

    /// Straight-alpha pixel at (x, y).
    fn pixel(map: &Pixmap, x: u32, y: u32) -> Rgba {
        let at = ((y * map.size + x) * 4) as usize;
        Rgba {
            a: map.argb[at],
            r: map.argb[at + 1],
            g: map.argb[at + 2],
            b: map.argb[at + 3],
        }
    }

    fn opaque_rows(map: &Pixmap, column: u32) -> Vec<u32> {
        (0..map.size)
            .filter(|y| pixel(map, column, *y).a > 200)
            .collect()
    }

    #[test]
    fn every_size_is_square_argb32() {
        let bars = specs(&[(Some(50.0), Level::Normal), (Some(10.0), Level::Normal)]);
        let icons = render_all(&bars, ColorScheme::Light);
        assert_eq!(icons.len(), SIZES.len());
        for (icon, size) in icons.iter().zip(SIZES) {
            assert_eq!(icon.size, size);
            assert_eq!(icon.argb.len(), (size * size * 4) as usize);
            let converted = ksni::Icon::from(icon);
            assert_eq!(converted.width, i32::try_from(size).unwrap());
            assert_eq!(converted.height, i32::try_from(size).unwrap());
        }
    }

    #[test]
    fn bytes_are_alpha_first_network_order() {
        let bars = specs(&[(Some(100.0), Level::Critical), (None, Level::Normal)]);
        let map = render(32, &bars, ColorScheme::Light);
        // The centre of the first bar is solidly filled with the critical red.
        let centre = pixel(&map, 8, 16);
        assert_eq!(centre.a, 255, "alpha byte comes first");
        assert_eq!(
            (centre.r, centre.g, centre.b),
            (0xda, 0x44, 0x53),
            "R, G, B follow in that order"
        );
        // Outside both bars everything is fully transparent.
        assert_eq!(pixel(&map, 0, 0).a, 0);
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a pixel coordinate computed from the icon geometry is small and positive"
    )]
    #[test]
    fn an_empty_bar_keeps_its_outline_but_no_fill() {
        let bars = specs(&[(None, Level::Normal), (None, Level::Normal)]);
        let map = render(32, &bars, ColorScheme::Light);
        let geo = geometry(32, 2);
        let mid_x = f64::midpoint(geo.bar(0).x0, geo.bar(0).x1);
        // The middle of the bar is empty …
        assert_eq!(pixel(&map, mid_x as u32, 16).a, 0);
        // … but the side walls are drawn.
        let wall = pixel(&map, geo.bar(0).x0 as u32, 16);
        assert!(wall.a > 0, "outline is drawn: {wall:?}");
        assert!(map.argb.iter().any(|byte| *byte != 0), "not a blank icon");
    }

    #[test]
    fn the_fill_grows_from_the_bottom_with_the_percentage() {
        let column = 8;
        let at = |percent| {
            let bars = specs(&[(Some(percent), Level::Normal), (None, Level::Normal)]);
            render(32, &bars, ColorScheme::Light)
        };
        let (quarter, most) = (at(25.0), at(90.0));
        let low = opaque_rows(&quarter, column);
        let high = opaque_rows(&most, column);
        assert!(low.len() < high.len(), "{} < {}", low.len(), high.len());
        // Both fills touch the bottom of the bar and never the very top row.
        assert!(low.iter().max() > low.iter().min());
        assert_eq!(low.iter().max(), high.iter().max());
        assert!(high.iter().min() < low.iter().min());
    }

    #[test]
    fn levels_pick_the_fill_colour_and_the_scheme_picks_the_outline() {
        let at_centre = |level, scheme| {
            let map = render(32, &specs(&[(Some(100.0), level)]), scheme);
            pixel(&map, 8, 16)
        };
        assert_eq!(
            at_centre(Level::Warning, ColorScheme::Light),
            Rgba::rgb(0xf6, 0x74, 0x00).with_alpha(255)
        );
        assert_eq!(
            at_centre(Level::Normal, ColorScheme::Light),
            Rgba::rgb(0x23, 0x26, 0x29).with_alpha(255)
        );
        assert_eq!(
            at_centre(Level::Normal, ColorScheme::Dark),
            Rgba::rgb(0xef, 0xf0, 0xf1).with_alpha(255)
        );
    }

    #[test]
    fn a_meter_without_bars_still_draws_two_slots() {
        assert_eq!(bar_specs(None).len(), 2);
        assert!(bar_specs(None).iter().all(|bar| bar.percent.is_none()));

        let meter = Meter {
            bars: vec![MeterBar {
                provider: ProviderId::Claude,
                percent: Some(34.0),
                level: Level::Warning,
                window_id: Some("session".into()),
            }],
            level: Level::Warning,
        };
        let specs = bar_specs(Some(&meter));
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].percent, Some(34.0));
        assert_eq!(specs[0].level, Level::Warning);
    }

    #[expect(
        clippy::float_cmp,
        reason = "compares exact literals that never went through arithmetic"
    )]
    #[test]
    fn out_of_range_percentages_are_clamped() {
        assert_eq!(
            BarSpec {
                percent: Some(240.0),
                level: Level::Normal
            }
            .fraction(),
            1.0
        );
        assert_eq!(
            BarSpec {
                percent: Some(-4.0),
                level: Level::Normal
            }
            .fraction(),
            0.0
        );
    }
}
