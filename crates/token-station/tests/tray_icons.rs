#![cfg(not(target_os = "macos"))]
//! Linux only: the session bus, the tray and the desktop integration.

//! Renders the tray icon for every fixture and writes PNG previews you can look at.
//!
//! Output: `/tmp/ts-tray-icons/<fixture>-<scheme>-<size>.png`, each composited over
//! the panel colour the scheme implies, so the previews show what a host would draw.

use std::path::{Path, PathBuf};

use token_station::tray::icon::{self, SIZES};
use token_station::tray::palette::{self, ColorScheme, Rgba};
use ts_core::Snapshot;

/// Where the previews land.
const OUTPUT: &str = "/tmp/ts-tray-icons";

const FIXTURES: [&str; 5] = [
    "snapshot-ok",
    "snapshot-near-limit",
    "snapshot-loading",
    "snapshot-not-installed",
    "snapshot-stale-auth",
];

fn fixture(name: &str) -> Snapshot {
    let path = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/fixtures"))
        .join(format!("{name}.json"));
    serde_json::from_str(&std::fs::read_to_string(&path).expect("fixture reads"))
        .expect("fixture parses")
}

/// Panel colour a scheme implies, used as the preview background.
fn panel(scheme: ColorScheme) -> Rgba {
    match scheme {
        ColorScheme::Dark => palette::DARK_FOREGROUND,
        ColorScheme::Light => palette::LIGHT_FOREGROUND,
    }
}

/// ARGB32 over an opaque background, as RGBA8 rows.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the composited channel is rounded and cannot leave 0.0..=255.0"
)]
fn composite(pixmap: &icon::Pixmap, background: Rgba) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(pixmap.argb.len());
    for pixel in pixmap.argb.chunks_exact(4) {
        let alpha = f64::from(pixel[0]) / 255.0;
        let over = |source: u8, under: u8| {
            (f64::from(source) * alpha + f64::from(under) * (1.0 - alpha)).round() as u8
        };
        rgba.extend_from_slice(&[
            over(pixel[1], background.r),
            over(pixel[2], background.g),
            over(pixel[3], background.b),
            255,
        ]);
    }
    rgba
}

fn write_png(path: &Path, size: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("preview is writable");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size, size);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("header")
        .write_image_data(rgba)
        .expect("image data");
}

#[test]
fn write_icon_previews() {
    std::fs::create_dir_all(OUTPUT).expect("preview directory");
    let mut written = Vec::new();

    for name in FIXTURES {
        let snapshot = fixture(name);
        let specs = icon::bar_specs(Some(&snapshot.meter));
        for (scheme, label) in [(ColorScheme::Light, "light"), (ColorScheme::Dark, "dark")] {
            for pixmap in icon::render_all(&specs, scheme) {
                let path = PathBuf::from(OUTPUT)
                    .join(format!("{name}-{label}-{size}.png", size = pixmap.size));
                write_png(&path, pixmap.size, &composite(&pixmap, panel(scheme)));
                written.push(path);
            }
        }
    }

    assert_eq!(written.len(), FIXTURES.len() * 2 * SIZES.len());
    assert!(written.iter().all(|path| path.exists()));
    eprintln!("{} icon previews written to {OUTPUT}", written.len());
}
