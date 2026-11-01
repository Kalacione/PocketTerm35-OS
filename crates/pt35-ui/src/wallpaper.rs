//! A picture behind the launcher: `~/.config/pt35/wallpaper.svg`.
//!
//! Rendered once per menu, off the Wayland thread, at the SVG's own size.
//! `Canvas::blit_rgb` scales it to the surface.

use resvg::{tiny_skia, usvg};
use std::path::Path;

use crate::canvas::Canvas;

pub struct Wallpaper {
    pub width: u32,
    pub height: u32,
    /// Opaque RGB. A transparent pixel comes out black.
    pub rgb: Vec<u8>,
}

impl Wallpaper {
    /// No file is no wallpaper: the theme's background stays.
    pub fn load(path: &Path) -> Option<Self> {
        let data = std::fs::read(path).ok()?;
        let wallpaper = Self::from_svg(&data);
        if wallpaper.is_none() {
            log::warn!("{}: not an SVG this can draw", path.display());
        }
        wallpaper
    }

    /// resvg is built without text: an SVG made of `<text>` draws nothing.
    pub fn from_svg(data: &[u8]) -> Option<Self> {
        let tree = usvg::Tree::from_data(data, &usvg::Options::default()).ok()?;
        let width = tree.size().width().ceil() as u32;
        let height = tree.size().height().ceil() as u32;
        let mut pixmap = tiny_skia::Pixmap::new(width, height)?;
        resvg::render(
            &tree,
            tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        let rgb = pixmap
            .data()
            .chunks_exact(4)
            .flat_map(|p| [p[0], p[1], p[2]])
            .collect();
        Some(Self { width, height, rgb })
    }

    pub fn draw(&self, canvas: &mut Canvas) {
        let (w, h) = (canvas.width, canvas.height);
        canvas.blit_rgb(0, 0, w, h, &self.rgb, self.width, self.height);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_at_the_svg_size() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="2"><rect width="4" height="2" fill="#63e6a4"/></svg>"##;
        let wallpaper = Wallpaper::from_svg(svg).unwrap();
        assert_eq!((wallpaper.width, wallpaper.height), (4, 2));
        assert_eq!(wallpaper.rgb.len(), 4 * 2 * 3);
        assert_eq!(&wallpaper.rgb[..3], &[0x63, 0xe6, 0xa4]);
    }

    #[test]
    fn scales_to_the_canvas() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="#ff0000"/></svg>"##;
        let mut canvas = Canvas::new(8, 6);
        Wallpaper::from_svg(svg).unwrap().draw(&mut canvas);
        assert_eq!(canvas.pixel(7, 5), 0xffff_0000);
    }

    #[test]
    fn no_file_and_no_svg_are_no_wallpaper() {
        assert!(Wallpaper::load(Path::new("/nonexistent/wallpaper.svg")).is_none());
        assert!(Wallpaper::from_svg(b"not an svg").is_none());
    }
}
