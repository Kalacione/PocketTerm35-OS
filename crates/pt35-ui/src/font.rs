//! Font loading and text drawing.
//!
//! No fontconfig and no shaping engine: the shell's own UI is Latin text at a
//! handful of sizes. The font is looked up by the family name in `theme.toml`
//! against the paths Raspberry Pi OS actually ships.
//!
//! ab_glyph outlines a glyph only when it is first drawn. fontdue, before it,
//! parsed every glyph in the face on load: 19 MB and 43 ms for DejaVu Sans on
//! a Pi 5, in every bar and in every menu, for a few dozen glyphs.

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont as _};
use anyhow::{Context, Result};
use pt35_common::theme::Rgb;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::canvas::Canvas;

pub struct Font {
    inner: FontVec,
    /// Rasterised glyphs, keyed by (char, size in 1/10 px). The shell redraws
    /// the same few strings constantly, so caching is most of the speed.
    cache: HashMap<(char, u32), Glyph>,
}

/// One rasterised glyph.
#[derive(Clone)]
struct Glyph {
    /// The coverage bitmap's top-left corner, from the pen on the baseline.
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    advance: f32,
    coverage: Vec<u8>,
}

impl Glyph {
    fn rasterise(font: &FontVec, ch: char, size: f32) -> Self {
        // Every size in theme.toml is pixels per em. ab_glyph's scale is the
        // height from descender to ascender.
        let per_em = font.units_per_em().unwrap_or(1000.0);
        let scale = PxScale::from(size * font.height_unscaled() / per_em);
        let id = font.glyph_id(ch);
        let advance = font.as_scaled(scale).h_advance(id);
        let placed = id.with_scale_and_position(scale, ab_glyph::point(0.0, 0.0));
        let Some(outline) = font.outline_glyph(placed) else {
            // A space: nothing to draw, only an advance.
            return Self {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
                advance,
                coverage: Vec::new(),
            };
        };
        let bounds = outline.px_bounds();
        let (width, height) = (bounds.width() as usize, bounds.height() as usize);
        let mut coverage = vec![0u8; width * height];
        outline.draw(|x, y, c| {
            if let Some(px) = coverage.get_mut(y as usize * width + x as usize) {
                *px = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
            }
        });
        Self {
            left: bounds.min.x as i32,
            top: bounds.min.y as i32,
            width,
            height,
            advance,
            coverage,
        }
    }
}

/// Where a family name might live on Raspberry Pi OS / Debian.
pub fn candidates(family: &str) -> Vec<PathBuf> {
    let slug: String = family.chars().filter(|c| !c.is_whitespace()).collect();
    // "DejaVu Sans Bold" is the file DejaVuSans-Bold.ttf: the style is a suffix,
    // not part of the family name.
    let styled = family.rsplit_once(' ').map(|(head, style)| {
        let head: String = head.chars().filter(|c| !c.is_whitespace()).collect();
        format!("{head}-{style}")
    });
    let mut paths = Vec::new();
    if let Some(explicit) = std::env::var_os("PT35_FONT") {
        paths.push(PathBuf::from(explicit));
    }
    // The user's own fonts first: a theme can ship a face without root.
    let user = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .map(|d| d.join("fonts"));
    let system = [
        "/usr/share/fonts/truetype/dejavu",
        "/usr/share/fonts/truetype/liberation",
        "/usr/share/fonts/truetype/noto",
        "/usr/share/fonts/TTF",
        "/System/Library/Fonts/Supplemental",
    ]
    .map(PathBuf::from);
    for dir in user.into_iter().chain(system) {
        paths.push(dir.join(format!("{slug}.ttf")));
        if let Some(styled) = &styled {
            paths.push(dir.join(format!("{styled}.ttf")));
        }
        paths.push(dir.join(format!("{slug}-Regular.ttf")));
    }
    paths
}

impl Font {
    /// Load the first candidate that exists, or fail with every path tried so
    /// the log says exactly what to install.
    pub fn load(family: &str) -> Result<Self> {
        let tried = candidates(family);
        for path in &tried {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            let inner = FontVec::try_from_vec(bytes)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            log::debug!("font {family:?} from {}", path.display());
            return Ok(Self {
                inner,
                cache: HashMap::new(),
            });
        }
        Err(anyhow::anyhow!(
            "no font file for {family:?}; tried: {}",
            tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .context("install fonts-dejavu-core, or set PT35_FONT to a .ttf")
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self> {
        let inner = FontVec::try_from_vec(bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
        Ok(Self {
            inner,
            cache: HashMap::new(),
        })
    }

    fn glyph(&mut self, ch: char, size: f32) -> &Glyph {
        let key = (ch, (size * 10.0) as u32);
        let font = &self.inner;
        self.cache
            .entry(key)
            .or_insert_with(|| Glyph::rasterise(font, ch, size))
    }

    /// Width in pixels of `text` at `size`.
    pub fn measure(&mut self, text: &str, size: f32) -> u32 {
        self.measure_tracked(text, size, 0.0)
    }

    /// Width with `tracking` extra pixels between glyphs.
    pub fn measure_tracked(&mut self, text: &str, size: f32, tracking: f32) -> u32 {
        let mut width = 0.0;
        for ch in text.chars() {
            width += self.glyph(ch, size).advance + tracking;
        }
        (width - tracking).max(0.0).ceil() as u32
    }

    /// Draw `text` with its left edge at `x` and its baseline at `y`.
    /// Returns the x coordinate just past the last glyph.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: Rgb,
    ) -> i32 {
        self.draw_tracked(canvas, text, x, y, size, color, 0.0)
    }

    /// Draw `text` over a soft halo of itself, `radius` pixels wide: the glow
    /// of a phosphor readout. A radius of 0 is plain [`Font::draw`].
    #[allow(clippy::too_many_arguments)]
    pub fn draw_glow(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: Rgb,
        radius: u32,
    ) -> i32 {
        if radius > 0 {
            let width = self.measure(text, size) as usize + 4;
            let height = (size * 1.5).ceil() as usize;
            let base = size.ceil() as i32;
            let mut alpha = vec![0u8; width * height];
            let mut pen = 0f32;
            for ch in text.chars() {
                let glyph = self.glyph(ch, size);
                let (gx, gy) = (pen as i32 + glyph.left, base + glyph.top);
                for row in 0..glyph.height {
                    for col in 0..glyph.width {
                        let (px, py) = (gx + col as i32, gy + row as i32);
                        if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                            let at = py as usize * width + px as usize;
                            alpha[at] = alpha[at].max(glyph.coverage[row * glyph.width + col]);
                        }
                    }
                }
                pen += glyph.advance;
            }
            let r = radius as usize;
            let halo = crate::canvas::glow_mask(&alpha, width, height, r);
            let side = width + 2 * r;
            for (i, &a) in halo.iter().enumerate() {
                let (col, row) = ((i % side) as i32, (i / side) as i32);
                canvas.blend(x + col - r as i32, y - base + row - r as i32, a, color);
            }
        }
        self.draw(canvas, text, x, y, size, color)
    }

    /// Draw with extra spacing between glyphs.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_tracked(
        &mut self,
        canvas: &mut Canvas,
        text: &str,
        x: i32,
        y: i32,
        size: f32,
        color: Rgb,
        tracking: f32,
    ) -> i32 {
        let mut pen = x as f32;
        for ch in text.chars() {
            let glyph = self.glyph(ch, size);
            let (gx, gy) = (pen as i32 + glyph.left, y + glyph.top);
            for row in 0..glyph.height {
                for col in 0..glyph.width {
                    let coverage = glyph.coverage[row * glyph.width + col];
                    canvas.blend(gx + col as i32, gy + row as i32, coverage, color);
                }
            }
            pen += glyph.advance + tracking;
        }
        (pen - tracking).ceil() as i32
    }

    /// Whether this face has `ch`, rather than drawing its empty box.
    pub fn has(&self, ch: char) -> bool {
        self.inner.glyph_id(ch).0 != 0
    }

    /// One glyph, flipped left to right when `mirror`, over a halo `glow`
    /// pixels wide when that is not 0.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_char(
        &mut self,
        canvas: &mut Canvas,
        ch: char,
        x: i32,
        y: i32,
        size: f32,
        color: Rgb,
        mirror: bool,
        glow: u32,
    ) {
        let glyph = self.glyph(ch, size).clone();
        let (width, height) = (glyph.width, glyph.height);
        let mut alpha = glyph.coverage;
        if width == 0 {
            return;
        }
        if mirror {
            for row in alpha.chunks_mut(width) {
                row.reverse();
            }
        }
        let gx = if mirror {
            x + glyph.advance as i32 - glyph.left - width as i32
        } else {
            x + glyph.left
        };
        let gy = y + glyph.top;
        if glow > 0 {
            let r = glow as usize;
            let halo = crate::canvas::glow_mask(&alpha, width, height, r);
            let side = width + 2 * r;
            for (i, &a) in halo.iter().enumerate() {
                let (col, row) = ((i % side) as i32, (i / side) as i32);
                canvas.blend(gx + col - r as i32, gy + row - r as i32, a, color);
            }
        }
        for (i, &coverage) in alpha.iter().enumerate() {
            let (col, row) = ((i % width) as i32, (i / width) as i32);
            canvas.blend(gx + col, gy + row, coverage, color);
        }
    }

    /// Shorten `text` with an ellipsis so it fits in `max_width` pixels.
    pub fn elide(&mut self, text: &str, size: f32, max_width: u32) -> String {
        if self.measure(text, size) <= max_width {
            return text.to_string();
        }
        // When even the ellipsis does not fit, a bare truncation is better
        // than drawing past the edge of a 640px screen.
        let ellipsis = "…";
        let ellipsis_width = self.measure(ellipsis, size);
        let (budget, suffix) = if ellipsis_width > max_width {
            (max_width, "")
        } else {
            (max_width - ellipsis_width, ellipsis)
        };
        let mut out = String::new();
        let mut width = 0;
        for ch in text.chars() {
            let advance = self.measure(&ch.to_string(), size);
            if width + advance > budget {
                break;
            }
            width += advance;
            out.push(ch);
        }
        out.push_str(suffix);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Any real TTF on the host will do; skip when the box has none.
    fn test_font() -> Option<Font> {
        for family in ["DejaVu Sans", "Helvetica", "Arial"] {
            if let Ok(font) = Font::load(family) {
                return Some(font);
            }
        }
        None
    }

    #[test]
    fn a_mirrored_glyph_is_the_same_ink_flipped() {
        let Some(mut font) = test_font() else {
            return;
        };
        let ink = |font: &mut Font, mirror| {
            let mut canvas = Canvas::new(24, 24);
            font.draw_char(&mut canvas, 'L', 4, 20, 16.0, Rgb(255, 255, 255), mirror, 0);
            canvas
        };
        let (plain, flipped) = (ink(&mut font, false), ink(&mut font, true));
        let lit = |c: &Canvas| {
            (0..24)
                .flat_map(|x| (0..24).map(move |y| (x, y)))
                .filter(|&(x, y)| c.pixel(x, y) & 0xff > 0)
                .count()
        };
        assert_eq!(lit(&plain), lit(&flipped));
        assert!((0..24).any(|x| (0..24).any(|y| plain.pixel(x, y) != flipped.pixel(x, y))));
    }

    #[test]
    fn candidate_paths_cover_debian_and_the_env_override() {
        std::env::set_var("PT35_FONT", "/tmp/x.ttf");
        let paths = candidates("DejaVu Sans");
        std::env::remove_var("PT35_FONT");
        assert_eq!(paths[0], PathBuf::from("/tmp/x.ttf"));
        assert!(paths.iter().any(|p| p.ends_with("dejavu/DejaVuSans.ttf")));
    }

    #[test]
    fn missing_font_names_every_path_tried() {
        let err = match Font::load("NoSuchFamilyAtAll") {
            Ok(_) => panic!("a font named NoSuchFamilyAtAll should not exist"),
            Err(err) => err,
        };
        let text = format!("{err:#}");
        assert!(
            text.contains("fonts-dejavu-core"),
            "error should say how to fix it: {text}"
        );
    }

    #[test]
    fn measures_and_elides() {
        let Some(mut font) = test_font() else { return };
        let wide = font.measure("mmmmmmmmmmmm", 16.0);
        let narrow = font.measure("ii", 16.0);
        assert!(wide > narrow);

        let elided = font.elide("a very long menu entry indeed", 16.0, wide);
        assert!(elided.ends_with('…'), "{elided:?}");
        assert!(
            font.measure(&elided, 16.0) <= wide,
            "elided text must fit the budget"
        );

        // Budget too small even for the ellipsis: truncate rather than overflow.
        let tiny = font.elide("a very long menu entry indeed", 16.0, 4);
        assert!(
            font.measure(&tiny, 16.0) <= 4,
            "{tiny:?} overflows a 4px budget"
        );
    }

    #[test]
    fn sizes_are_pixels_per_em() {
        // theme.toml was laid out with fontdue, which takes pixels per em.
        // ab_glyph's own scale is a line height: unconverted, text grows.
        let Some(mut font) = test_font() else { return };
        let id = font.inner.glyph_id('H');
        let per_em = font.inner.units_per_em().unwrap();
        let want = font.inner.h_advance_unscaled(id) * 20.0 / per_em;
        let glyph = font.glyph('H', 20.0);
        assert!(
            (glyph.advance - want).abs() < 0.01,
            "{} vs {want}",
            glyph.advance
        );
        // Ink above the baseline, as tall as a capital at 20px per em.
        assert!(glyph.top < -10 && glyph.top + glyph.height as i32 <= 1);
        assert!(glyph.coverage.contains(&255), "solid stems");
    }

    #[test]
    fn a_space_has_an_advance_and_no_ink() {
        let Some(mut font) = test_font() else { return };
        let space = font.glyph(' ', 16.0);
        assert!(space.advance > 2.0);
        assert!(space.coverage.iter().all(|c| *c == 0));
    }

    #[test]
    fn tracking_widens_text_without_changing_the_first_glyph() {
        let Some(mut font) = test_font() else { return };
        let plain = font.measure("menu", 16.0);
        let tracked = font.measure_tracked("menu", 16.0, 2.0);
        assert_eq!(tracked, plain + 6, "three gaps between four glyphs");
        assert_eq!(
            font.measure_tracked("m", 16.0, 2.0),
            font.measure("m", 16.0)
        );
    }

    #[test]
    fn draws_ink_on_the_canvas() {
        let Some(mut font) = test_font() else { return };
        let mut canvas = Canvas::new(64, 24);
        canvas.fill(Rgb(0, 0, 0));
        let end = font.draw(&mut canvas, "Hi", 2, 18, 14.0, Rgb(0xff, 0xff, 0xff));
        assert!(end > 2);
        let lit = (0..64 * 24)
            .filter(|i| canvas.pixel(i % 64, i / 64) != 0xff00_0000)
            .count();
        assert!(lit > 10, "expected glyph coverage, got {lit} lit pixels");
    }
}
