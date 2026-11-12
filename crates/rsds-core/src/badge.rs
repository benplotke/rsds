//! Geometry of the number badge shown on each monitor by Identify.
//!
//! The badge is a filled box with a border and blocky seven-segment digits,
//! described as rectangles so each backend can draw it with whatever
//! primitive it has (pixel buffer on Wayland, rectangle fills on X11).

use crate::layout::Rect;

/// Colours as 0xAARRGGBB.
pub const BACKGROUND: u32 = 0xff20_2020;
pub const BORDER: u32 = 0xffd5_5050;
pub const DIGIT: u32 = 0xffee_eeee;

/// Distance from the monitor's bottom-left corner, in logical pixels.
pub const MARGIN: i32 = 48;

const THICKNESS: i32 = 16;
const DIGIT_W: i32 = 72;
const DIGIT_H: i32 = 128;
const GAP: i32 = 20;
const PADDING: i32 = 36;
const BORDER_W: i32 = 4;

pub struct Badge {
    pub width: i32,
    pub height: i32,
    /// Border rectangles (drawn over the background).
    pub border: Vec<Rect>,
    /// Digit segment rectangles.
    pub segments: Vec<Rect>,
}

impl Badge {
    pub fn new(number: usize) -> Self {
        let digits: Vec<u8> = number.to_string().bytes().map(|b| b - b'0').collect();
        let n = digits.len() as i32;
        let width = 2 * PADDING + n * DIGIT_W + (n - 1) * GAP;
        let height = 2 * PADDING + DIGIT_H;

        let border = vec![
            Rect::new(0, 0, width, BORDER_W),
            Rect::new(0, height - BORDER_W, width, BORDER_W),
            Rect::new(0, 0, BORDER_W, height),
            Rect::new(width - BORDER_W, 0, BORDER_W, height),
        ];

        let mut segments = Vec::new();
        for (i, d) in digits.into_iter().enumerate() {
            let x = PADDING + i as i32 * (DIGIT_W + GAP);
            segments.extend(digit_segments(d, x, PADDING));
        }
        Self { width, height, border, segments }
    }

    /// Render to ARGB8888 pixels (native endian, as wl_shm expects).
    pub fn pixels(&self) -> Vec<u32> {
        let (w, h) = (self.width as usize, self.height as usize);
        let mut px = vec![BACKGROUND; w * h];
        let mut fill = |r: &Rect, colour: u32| {
            for y in r.y.max(0) as usize..(r.bottom() as usize).min(h) {
                px[y * w + r.x.max(0) as usize..y * w + (r.right() as usize).min(w)].fill(colour);
            }
        };
        self.border.iter().for_each(|r| fill(r, BORDER));
        self.segments.iter().for_each(|r| fill(r, DIGIT));
        px
    }
}

/// Segments lit for each digit, as bits a..g (bit 0 = a).
///
/// ```text
///  aaa
/// f   b
///  ggg
/// e   c
///  ddd
/// ```
const SEGMENTS: [u8; 10] = [
    0b0111111, // 0
    0b0000110, // 1
    0b1011011, // 2
    0b1001111, // 3
    0b1100110, // 4
    0b1101101, // 5
    0b1111101, // 6
    0b0000111, // 7
    0b1111111, // 8
    0b1101111, // 9
];

fn digit_segments(digit: u8, x: i32, y: i32) -> Vec<Rect> {
    let t = THICKNESS;
    let half = DIGIT_H / 2 + t / 2;
    let mid = y + DIGIT_H / 2 - t / 2;
    let right = x + DIGIT_W - t;
    let all = [
        Rect::new(x, y, DIGIT_W, t),               // a
        Rect::new(right, y, t, half),              // b
        Rect::new(right, mid, t, half),            // c
        Rect::new(x, y + DIGIT_H - t, DIGIT_W, t), // d
        Rect::new(x, mid, t, half),                // e
        Rect::new(x, y, t, half),                  // f
        Rect::new(x, mid, DIGIT_W, t),             // g
    ];
    let bits = SEGMENTS[digit as usize];
    all.into_iter()
        .enumerate()
        .filter(|(i, _)| bits & (1 << i) != 0)
        .map(|(_, r)| r)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_digits_are_wider() {
        assert!(Badge::new(12).width > Badge::new(1).width);
        assert_eq!(Badge::new(1).segments.len(), 2);
        assert_eq!(Badge::new(8).segments.len(), 7);
    }

    #[test]
    fn segments_inside_badge() {
        let b = Badge::new(88);
        for r in b.segments.iter().chain(&b.border) {
            assert!(r.x >= 0 && r.y >= 0 && r.right() <= b.width && r.bottom() <= b.height, "{r:?}");
        }
    }
}
