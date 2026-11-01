//! Slow falling glyphs behind the home: `[effects] rain`.
//!
//! Stateless and off the wall clock, like the sweep: the menu is a new
//! process per open, and the rain picks up where it was. A drop is a column
//! of glyphs that light up as its head passes and fade behind it. Each
//! column waits a random gap between two drops, so most are dark.

use pt35_common::theme::Rgb;

use crate::canvas::Canvas;
use crate::font::Font;

/// Hex, like a memory dump: every mono face has these.
const GLYPHS: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F',
];
const SIZE: f32 = 13.0;
const CELL: u32 = 16;
/// Lit cells behind a drop's head, the head included.
const TRAIL: i64 = 7;

/// splitmix64: the same number for the same input in every process.
pub fn scatter(n: u64) -> u64 {
    let mut z = n.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// The lit cells at `step`: column, row, glyph, and brightness out of
/// `TRAIL`, the head being `TRAIL`.
fn cells(cols: u32, rows: u32, step: u64) -> impl Iterator<Item = (u32, u32, char, u32)> {
    let rows = rows as i64;
    (0..cols).flat_map(move |col| {
        let seed = scatter(col as u64);
        // A screen, the trail and a gap of 20 rows or more.
        let cycle = (rows + TRAIL + 20) as u64 + seed % (2 * rows.max(1) as u64);
        let head = ((step + seed / 7) % cycle) as i64;
        (0..TRAIL).filter_map(move |k| {
            let row = head - k;
            (0..rows).contains(&row).then(|| {
                // A glyph stays put while drops pass and changes now and then.
                let pick = scatter(seed ^ ((row as u64) << 32) ^ ((step + row as u64) / 6));
                let glyph = GLYPHS[(pick % GLYPHS.len() as u64) as usize];
                (col, row as u32, glyph, (TRAIL - k) as u32)
            })
        })
    })
}

/// Draw step `step`. `strength` is the head's brightness, in percent of the
/// way from `back` to `color`.
pub fn draw(canvas: &mut Canvas, font: &mut Font, step: u64, strength: u8, color: Rgb, back: Rgb) {
    let strength = strength.min(100) as u32;
    if strength == 0 {
        return;
    }
    let (cols, rows) = (canvas.width / CELL, canvas.height / CELL);
    let left = (canvas.width - cols * CELL) as i32 / 2;
    let mix = |b: u8, c: u8, p: u32| ((b as u32 * (100 - p) + c as u32 * p) / 100) as u8;
    for (col, row, glyph, level) in cells(cols, rows, step) {
        let p = strength * level / TRAIL as u32;
        let ink = Rgb(
            mix(back.0, color.0, p),
            mix(back.1, color.1, p),
            mix(back.2, color.2, p),
        );
        let x = left + (col * CELL) as i32;
        // `draw` takes a baseline.
        let y = ((row + 1) * CELL) as i32 - 3;
        font.draw(canvas, glyph.encode_utf8(&mut [0; 4]), x, y, SIZE, ink);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_and_the_same_in_every_process() {
        let (cols, rows) = (40, 27);
        let a: Vec<_> = cells(cols, rows, 1234).collect();
        assert_eq!(a, cells(cols, rows, 1234).collect::<Vec<_>>());
        assert!(!a.is_empty());
        // Under a sixth of the cells, most of them a dim trail.
        assert!(a.len() < (cols * rows / 6) as usize, "{} lit", a.len());
        assert!(a.iter().all(|&(c, r, _, _)| c < cols && r < rows));
    }

    #[test]
    fn a_drop_falls_one_row_a_step() {
        let heads = |step| -> Vec<_> {
            cells(40, 27, step)
                .filter(|c| c.3 == TRAIL as u32 && c.1 < 26)
                .map(|c| (c.0, c.1))
                .collect()
        };
        let next = heads(101);
        for (col, row) in heads(100) {
            assert!(next.contains(&(col, row + 1)), "column {col}");
        }
    }
}
