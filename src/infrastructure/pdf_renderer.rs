//! `PdfRenderer` — implements [`PdfPageRenderer`] using the pure-Rust
//! `pdf_render` rasterizer.
//!
//! Renders a PDF page to a single-channel grayscale bitmap (converted from the
//! premultiplied RGBA8 pixmap) for OCR consumption.

use std::sync::Arc;

use pdf_render::RenderSettings;
use pdf_render::pdf_interpret::InterpreterSettings;
use pdf_render::pdf_syntax::Pdf;

use crate::domain::error::ExtractionError;
use crate::domain::ports::PdfPageRenderer;
use crate::domain::result::PageBitmap;

use super::error_map::map_load_pdf_error;

/// Maximum total pixels allowed for a rendered page bitmap (guards against a
/// giant pixmap allocation from a pathological page / DPI).
pub const MAX_RENDER_PIXELS: u64 = 100_000_000;
/// Maximum allowed side (width or height) in pixels.
pub const MAX_RENDER_SIDE: u64 = 15_000;

/// Convert a premultiplied RGBA8 pixel buffer to single-channel grayscale
/// (luma), one byte per pixel.
pub fn rgba8_to_luma(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 4);
    let mut i = 0;
    while i + 4 <= rgba.len() {
        let r = rgba[i];
        let g = rgba[i + 1];
        let b = rgba[i + 2];
        // Alpha is ignored; Rec. 601 luma.
        out.push((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32).round() as u8);
        i += 4;
    }
    out
}

/// Estimate the rendered page's pixel dimensions from its crop box and the
/// scale, returning `(width_px, height_px)`.
fn estimate_dimensions(page: &pdf_render::pdf_syntax::page::Page<'_>, scale: f32) -> (u64, u64) {
    let b = page.intersected_crop_box();
    let w = (((b.x1 - b.x0) as f32) * scale).ceil() as u64;
    let h = (((b.y1 - b.y0) as f32) * scale).ceil() as u64;
    (w.max(1), h.max(1))
}

/// Enforce the render pixel cap: returns `ResourceLimit` when the page would
/// allocate a giant pixmap.
pub fn check_render_cap(
    width_px: u64,
    height_px: u64,
    page_index: usize,
) -> Result<(), ExtractionError> {
    if width_px > MAX_RENDER_SIDE
        || height_px > MAX_RENDER_SIDE
        || width_px * height_px > MAX_RENDER_PIXELS
    {
        return Err(ExtractionError::ResourceLimit(format!(
            "page {page_index} renders at {width_px}x{height_px} px, exceeding the render cap \
             (max side {MAX_RENDER_SIDE} px, max {MAX_RENDER_PIXELS} px)"
        )));
    }
    Ok(())
}

/// Pure-Rust PDF page → image renderer.
pub struct PdfRenderer {}

impl PdfRenderer {
    /// Create a new renderer.
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for PdfRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl PdfPageRenderer for PdfRenderer {
    fn render_page(
        &self,
        pdf_bytes: &[u8],
        page_index: usize,
        dpi: u32,
    ) -> Result<PageBitmap, ExtractionError> {
        let scale = dpi.max(1) as f32 / 72.0;

        let result = std::panic::catch_unwind(|| {
            let pdf = Pdf::new(pdf_bytes.to_vec()).map_err(map_load_pdf_error)?;
            let page = pdf
                .pages()
                .get(page_index)
                .ok_or_else(|| ExtractionError::Io(format!("page {page_index} out of range")))?;
            // Guard against a giant pixmap allocation BEFORE rendering.
            let (w_px, h_px) = estimate_dimensions(page, scale);
            check_render_cap(w_px, h_px, page_index)?;

            let pixmap = pdf_render::render(
                page,
                &InterpreterSettings::default(),
                &RenderSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: pdf_render::vello_cpu::color::palette::css::WHITE,
                    ..Default::default()
                },
            );
            // The pixmap is premultiplied RGBA8; carry its real dimensions and
            // convert to single-channel grayscale (luma), one byte per pixel.
            let width = pixmap.width() as u32;
            let height = pixmap.height() as u32;
            let luma = rgba8_to_luma(pixmap.data_as_u8_slice());
            PageBitmap::new(width, height, 1, luma)
        });

        match result {
            Ok(inner) => inner,
            Err(_) => Err(ExtractionError::Internal(
                "PDF renderer panicked while rasterizing a page".into(),
            )),
        }
    }
}

/// Build an `Arc<dyn PdfPageRenderer>` for wiring/composition.
pub fn make_renderer() -> Arc<dyn PdfPageRenderer> {
    Arc::new(PdfRenderer::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_to_luma_converts() {
        // White -> ~255, black -> 0.
        let rgba = [255, 255, 255, 255, 0, 0, 0, 255];
        let luma = rgba8_to_luma(&rgba);
        assert_eq!(luma.len(), 2);
        assert!(luma[0] > 250);
        assert_eq!(luma[1], 0);
    }

    #[test]
    fn render_page_out_of_range_is_error() {
        let r = PdfRenderer::new();
        let err = r.render_page(b"%PDF-1.4 junk", 5, 300).unwrap_err();
        // Either the PDF fails to load or the page is out of range — both are
        // domain errors, never a panic.
        let _ = err;
    }

    #[test]
    fn constructors_and_factory() {
        // `new()`, `Default` and `make_renderer()` all yield a usable renderer.
        let a = PdfRenderer::new();
        let b = PdfRenderer::default();
        let c = make_renderer();
        assert!(a.render_page(b"junk", 0, 72).is_err());
        assert!(b.render_page(b"junk", 0, 72).is_err());
        assert!(c.render_page(b"junk", 0, 72).is_err());
    }

    #[test]
    fn rgba8_to_luma_handles_greys() {
        // Mid-grey rounds to the same value for all channels.
        let rgba = [100, 100, 100, 255];
        let luma = rgba8_to_luma(&rgba);
        assert_eq!(luma, vec![100]);
    }

    #[test]
    fn check_render_cap_accepts_normal() {
        assert!(check_render_cap(3000, 4000, 0).is_ok());
    }

    #[test]
    fn check_render_cap_rejects_giant_side_and_pixels() {
        assert!(matches!(
            check_render_cap(20_000, 100, 0),
            Err(ExtractionError::ResourceLimit(_))
        ));
        assert!(matches!(
            check_render_cap(20_000, 20_000, 0),
            Err(ExtractionError::ResourceLimit(_))
        ));
    }
}
