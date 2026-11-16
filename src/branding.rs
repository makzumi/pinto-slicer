//! Application identity and the embedded Pinto artwork.
//!
//! Windows and X11 can use the RGBA window icon directly. Wayland instead
//! matches `APP_ID` to `packaging/pinto-slicer.desktop`, whose `Icon` entry
//! resolves to the same installed artwork for the taskbar and alt-tab UI.

pub const APP_NAME: &str = "Pinto Slicer";
pub const APP_ID: &str = "pinto-slicer";
pub const HEADER_SIZE: f32 = 32.0;

const ICON_PNG: &[u8] = include_bytes!("../assets/pinto-slicer.png");

fn rgba(png: &[u8]) -> (u32, u32, Vec<u8>) {
    let image = image::load_from_memory_with_format(png, image::ImageFormat::Png)
        .expect("the embedded Pinto icon is a valid PNG")
        .into_rgba8();
    let (width, height) = image.dimensions();
    (width, height, image.into_raw())
}

/// Native window icon for platforms that support per-window icons.
pub fn window_icon() -> eframe::egui::IconData {
    let (width, height, rgba) = rgba(ICON_PNG);
    eframe::egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Resize once to the exact physical size of the header image. Egui textures
/// have no mipmaps, so minifying the 512px source directly aliases the white
/// bean's curved edge. A one-time Lanczos filter is crisp at every DPI.
pub fn header_image(pixels_per_point: f32) -> (u32, eframe::egui::ColorImage) {
    let size = (HEADER_SIZE * pixels_per_point).round().clamp(16.0, 256.0) as u32;
    let (width, height, rgba) = rgba(ICON_PNG);
    let source = image::RgbaImage::from_raw(width, height, rgba)
        .expect("the embedded Pinto icon dimensions match its pixels");
    let resized =
        image::imageops::resize(&source, size, size, image::imageops::FilterType::Lanczos3);
    (
        size,
        eframe::egui::ColorImage::from_rgba_unmultiplied(
            [size as usize, size as usize],
            resized.as_raw(),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_icon_is_a_square_rgba_image() {
        let icon = window_icon();
        assert_eq!((icon.width, icon.height), (512, 512));
        assert_eq!(icon.rgba.len(), 512 * 512 * 4);
        assert!(icon
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .any(|pixel| pixel[3] != 0));

        let (small_size, small) = header_image(1.0);
        assert_eq!((small_size, small.size), (32, [32, 32]));
        let (retina_size, retina) = header_image(2.0);
        assert_eq!((retina_size, retina.size), (64, [64, 64]));
    }
}
