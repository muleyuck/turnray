//! Menu bar images. tray-icon shows one image and one title per item, so the full style's
//! row of "icon + count" pairs is composed into a single image at runtime.

use std::io::Cursor;

use agent_core::Status;

/// Rendered from `assets/*.svg` by `make icons`, all 36px tall.
const BLOCKED: &[u8] = include_bytes!("../assets/tray-blocked-36.png");
const DONE: &[u8] = include_bytes!("../assets/tray-done-36.png");
const IDLE: &[u8] = include_bytes!("../assets/tray-idle-36.png");
const WORKING: &[u8] = include_bytes!("../assets/tray-working-36.png");
const UNKNOWN: &[u8] = include_bytes!("../assets/tray-unknown-36.png");
const STANDBY: &[u8] = include_bytes!("../assets/tray-standby-36.png");
const ERROR: &[u8] = include_bytes!("../assets/tray-error-36.png");
const DIGITS: [&[u8]; 10] = [
    include_bytes!("../assets/tray-digit-0-36.png"),
    include_bytes!("../assets/tray-digit-1-36.png"),
    include_bytes!("../assets/tray-digit-2-36.png"),
    include_bytes!("../assets/tray-digit-3-36.png"),
    include_bytes!("../assets/tray-digit-4-36.png"),
    include_bytes!("../assets/tray-digit-5-36.png"),
    include_bytes!("../assets/tray-digit-6-36.png"),
    include_bytes!("../assets/tray-digit-7-36.png"),
    include_bytes!("../assets/tray-digit-8-36.png"),
    include_bytes!("../assets/tray-digit-9-36.png"),
];

pub const HEIGHT: u32 = 36;
/// Between an icon and its count, which belong together.
const ICON_COUNT_GAP: u32 = 2;
/// Between one pair and the next (6pt).
const PAIR_GAP: u32 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

fn decode(png: &[u8]) -> Image {
    let mut reader = png::Decoder::new(Cursor::new(png))
        .read_info()
        .expect("tray image header");
    let mut buf = vec![0; reader.output_buffer_size().expect("tray image buffer size")];
    let info = reader.next_frame(&mut buf).expect("tray image pixels");
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "tray images must be RGBA"
    );
    buf.truncate(info.buffer_size());
    Image {
        rgba: buf,
        width: info.width,
        height: info.height,
    }
}

/// The PNG as embedded, for the panel to hand to AppKit.
pub fn status_png(status: Status) -> &'static [u8] {
    match status {
        Status::Blocked => BLOCKED,
        Status::Done => DONE,
        Status::Idle => IDLE,
        Status::Working => WORKING,
        Status::Unknown => UNKNOWN,
    }
}

pub fn status_icon(status: Status) -> Image {
    decode(status_png(status))
}

pub fn standby_icon() -> Image {
    decode(STANDBY)
}

pub fn error_png() -> &'static [u8] {
    ERROR
}

pub fn error_icon() -> Image {
    decode(ERROR)
}

fn digit(d: u32) -> Image {
    decode(DIGITS[d as usize])
}

/// A transparent strip to space parts apart.
fn gap(width: u32) -> Image {
    Image {
        rgba: vec![0; (width * HEIGHT * 4) as usize],
        width,
        height: HEIGHT,
    }
}

/// Lays `parts` side by side. They share one height, so rows copy straight across.
fn hconcat(parts: &[Image]) -> Image {
    let width: u32 = parts.iter().map(|p| p.width).sum();
    let mut rgba = Vec::with_capacity((width * HEIGHT * 4) as usize);
    for y in 0..HEIGHT as usize {
        for p in parts {
            assert_eq!(p.height, HEIGHT, "parts must share one height");
            let row = p.width as usize * 4;
            rgba.extend_from_slice(&p.rgba[y * row..(y + 1) * row]);
        }
    }
    Image {
        rgba,
        width,
        height: HEIGHT,
    }
}

/// Every status that has an agent, as "icon + count", in the order given.
pub fn full(counts: &[(Status, usize)]) -> Image {
    let mut parts = Vec::new();
    for (i, &(status, n)) in counts.iter().enumerate() {
        if i > 0 {
            parts.push(gap(PAIR_GAP));
        }
        parts.push(status_icon(status));
        parts.push(gap(ICON_COUNT_GAP));
        parts.extend(
            n.to_string()
                .chars()
                .map(|c| digit(c.to_digit(10).expect("digit"))),
        );
    }
    hconcat(&parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(img: &Image, x: u32, y: u32) -> u8 {
        img.rgba[((y * img.width + x) * 4 + 3) as usize]
    }

    fn column_is_empty(img: &Image, x: u32) -> bool {
        (0..img.height).all(|y| alpha_at(img, x, y) == 0)
    }

    #[test]
    fn every_status_icon_and_the_standby_and_error_icons_decode_to_36px_rgba() {
        let icons = Status::ALL
            .into_iter()
            .map(status_icon)
            .chain([standby_icon(), error_icon()]);
        for img in icons {
            assert_eq!((img.width, img.height), (36, 36));
            assert_eq!(img.rgba.len() as u32, img.width * img.height * 4);
        }
    }

    #[test]
    fn every_digit_decodes_to_36px_tall_rgba() {
        for d in 0..10 {
            let img = digit(d);
            assert_eq!(img.height, HEIGHT, "{d}");
            assert_eq!(img.rgba.len() as u32, img.width * img.height * 4, "{d}");
        }
    }

    #[test]
    fn full_lays_out_each_pair_with_its_digits() {
        let icon = 36;
        let digit_w = digit(0).width;
        let img = full(&[(Status::Blocked, 1), (Status::Idle, 12)]);
        assert_eq!(
            img.width,
            icon + ICON_COUNT_GAP + digit_w + PAIR_GAP + icon + ICON_COUNT_GAP + 2 * digit_w
        );
        assert_eq!(img.height, HEIGHT);
        assert_eq!(img.rgba.len() as u32, img.width * img.height * 4);
    }

    #[test]
    fn full_copies_each_part_in_place() {
        // The second icon must land exactly after the first pair and the gap.
        let img = full(&[(Status::Done, 1), (Status::Working, 1)]);
        let start = 36 + ICON_COUNT_GAP + digit(1).width + PAIR_GAP;
        let working = status_icon(Status::Working);
        for y in 0..HEIGHT {
            for x in 0..36 {
                assert_eq!(alpha_at(&img, start + x, y), alpha_at(&working, x, y));
            }
        }
        for x in start - PAIR_GAP..start {
            assert!(column_is_empty(&img, x), "gap column {x} has ink");
        }
    }

    #[test]
    fn full_of_nothing_is_an_empty_image() {
        let img = full(&[]);
        assert_eq!(img.width, 0);
        assert!(img.rgba.is_empty());
    }
}
