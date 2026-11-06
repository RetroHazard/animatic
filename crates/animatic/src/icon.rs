//! The application icon (a cherry blossom), shared by the window, the tray and, through
//! `build.rs`, the Windows executable. `assets/animatic.png` is the 256 px source;
//! `assets/animatic.ico` holds the same picture at the sizes Windows uses.

use image::imageops::FilterType;

const PNG: &[u8] = include_bytes!("../assets/animatic.png");

fn decoded() -> image::RgbaImage {
    image::load_from_memory_with_format(PNG, image::ImageFormat::Png)
        .expect("the bundled icon is a valid PNG")
        .into_rgba8()
}

/// The icon as RGBA bytes, `size` x `size` pixels.
pub fn rgba(size: u32) -> Vec<u8> {
    let icon = decoded();
    if icon.width() == size && icon.height() == size {
        icon.into_raw()
    } else {
        image::imageops::resize(&icon, size, size, FilterType::Lanczos3).into_raw()
    }
}

/// The icon of the main window.
pub fn window_icon() -> egui::IconData {
    let icon = decoded();
    egui::IconData {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_scales_to_any_requested_size() {
        for size in [16, 32, 256] {
            assert_eq!(rgba(size).len(), (size * size * 4) as usize);
        }
        let window = window_icon();
        assert_eq!(
            window.rgba.len(),
            (window.width * window.height * 4) as usize
        );
    }
}
