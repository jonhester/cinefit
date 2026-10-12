//! A test pattern drawn by the CPU into XRGB8888 buffers.
//!
//! Static parts show the picture's edges and levels; a moving bar shows
//! motion, so a repeated or dropped frame is visible as a stutter.

/// Width of the moving bar, in pixels.
pub const BAR_WIDTH: usize = 32;
/// How far the bar moves each frame.
pub const BAR_STEP: usize = 16;

/// The static pattern's colour at (x, y), as 0xRRGGBB.
///
/// A 2-pixel white border, eight 75% colour bars over the top two thirds,
/// and a full-range grey ramp under them.
pub fn background(x: usize, y: usize, width: usize, height: usize) -> u32 {
    if x < 2 || y < 2 || x + 2 >= width || y + 2 >= height {
        return 0xFF_FF_FF;
    }
    if y < height * 2 / 3 {
        // White, yellow, cyan, green, magenta, red, blue, black at 75%.
        const BARS: [u32; 8] = [
            0xBF_BF_BF, 0xBF_BF_00, 0x00_BF_BF, 0x00_BF_00, 0xBF_00_BF, 0xBF_00_00, 0x00_00_BF,
            0x00_00_00,
        ];
        return BARS[x * BARS.len() / width];
    }
    let level = (x * 256 / width) as u32;
    level << 16 | level << 8 | level
}

fn put(row: &mut [u8], x: usize, rgb: u32) {
    // XRGB8888 is little-endian: B, G, R, X.
    row[x * 4..x * 4 + 4].copy_from_slice(&(rgb | 0xFF00_0000).to_le_bytes());
}

/// Draws the whole static pattern.
pub fn draw_background(pixels: &mut [u8], pitch: usize, width: usize, height: usize) {
    for (y, row) in pixels.chunks_exact_mut(pitch).take(height).enumerate() {
        for x in 0..width {
            put(row, x, background(x, y, width, height));
        }
    }
}

/// Where the bar is on `frame`.
pub fn bar_x(frame: u64, width: usize) -> usize {
    let span = width.saturating_sub(BAR_WIDTH).max(1);
    (frame as usize).wrapping_mul(BAR_STEP) % span
}

/// Moves the bar in a buffer from `old_x` (if it was drawn there) to `new_x`,
/// redrawing only the columns that change.
pub fn move_bar(
    pixels: &mut [u8],
    pitch: usize,
    width: usize,
    height: usize,
    old_x: Option<usize>,
    new_x: usize,
) {
    let height_px = height.saturating_sub(4);
    for (y, row) in pixels.chunks_exact_mut(pitch).take(height).enumerate() {
        // The bar spans the picture inside the border.
        if y < 2 || y >= 2 + height_px {
            continue;
        }
        if let Some(old) = old_x {
            for x in old..(old + BAR_WIDTH).min(width) {
                put(row, x, background(x, y, width, height));
            }
        }
        for x in new_x..(new_x + BAR_WIDTH).min(width) {
            put(row, x, 0xFF_FF_FF);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(buf: &[u8], pitch: usize, x: usize, y: usize) -> u32 {
        let o = y * pitch + x * 4;
        u32::from_le_bytes(buf[o..o + 4].try_into().unwrap()) & 0xFF_FF_FF
    }

    #[test]
    fn border_bars_and_ramp() {
        let (w, h) = (80, 30);
        assert_eq!(background(0, 15, w, h), 0xFF_FF_FF);
        assert_eq!(background(79, 15, w, h), 0xFF_FF_FF);
        assert_eq!(background(5, 5, w, h), 0xBF_BF_BF); // first bar: white
        assert_eq!(background(75, 5, w, h), 0x00_00_00); // last bar: black
        assert_eq!(background(40, 25, w, h), 0x80_80_80); // ramp midpoint
    }

    #[test]
    fn rows_follow_the_pitch_not_the_width() {
        let (w, h, pitch) = (8, 6, 8 * 4 + 16);
        let mut buf = vec![0u8; pitch * h];
        draw_background(&mut buf, pitch, w, h);
        assert_eq!(pixel(&buf, pitch, 0, 3), 0xFF_FF_FF);
        // The padding past the last pixel is untouched.
        assert_eq!(&buf[3 * pitch + w * 4..4 * pitch], &[0u8; 16][..]);
    }

    #[test]
    fn moving_the_bar_restores_what_was_under_it() {
        let (w, h) = (200, 40);
        let pitch = w * 4;
        let mut fresh = vec![0u8; pitch * h];
        draw_background(&mut fresh, pitch, w, h);
        let mut buf = fresh.clone();
        move_bar(&mut buf, pitch, w, h, None, 10);
        assert_eq!(pixel(&buf, pitch, 20, 20), 0xFF_FF_FF);
        move_bar(&mut buf, pitch, w, h, Some(10), 100);
        // Old position: back to the pattern. New: bar.
        assert_eq!(pixel(&buf, pitch, 20, 20), pixel(&fresh, pitch, 20, 20));
        assert_eq!(pixel(&buf, pitch, 110, 20), 0xFF_FF_FF);
    }

    #[test]
    fn the_bar_stays_on_screen() {
        for frame in 0..10_000 {
            assert!(bar_x(frame, 3840) + BAR_WIDTH <= 3840);
        }
    }
}
