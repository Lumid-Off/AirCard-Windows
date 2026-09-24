use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use eframe::egui;
use image::{DynamicImage, GenericImageView, ImageFormat, RgbaImage, imageops::FilterType};

pub const CARD_WIDTH: u32 = 1_536;
pub const CARD_HEIGHT: u32 = 969;
pub const CARD_WIDTH_2X: u32 = 1_024;
pub const CARD_HEIGHT_2X: u32 = 646;

#[derive(Clone)]
pub struct PreparedSkin {
    /// Wallet @3x artwork (1536x969). Kept as `png` for compatibility with older code.
    pub png: Vec<u8>,
    /// Wallet @2x artwork (1024x646).
    pub png_2x: Vec<u8>,
    pub pdf: Vec<u8>,
    pub preview: egui::ColorImage,
    #[allow(dead_code)]
    pub source_width: u32,
    #[allow(dead_code)]
    pub source_height: u32,
}

impl PreparedSkin {
    #[allow(dead_code)]
    pub fn from_path(path: &Path) -> Result<Self> {
        let image =
            image::open(path).with_context(|| format!("Could not decode {}", path.display()))?;
        Self::from_image(image)
    }

    pub fn from_image(image: DynamicImage) -> Result<Self> {
        let (source_width, source_height) = image.dimensions();
        let cropped = center_crop_for_card(image);
        let final_image = cropped.resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3);
        Self::from_card_rgba_with_source(final_image.to_rgba8(), source_width, source_height)
    }

    /// Build all Wallet assets from an already-composed 1536x969 card image.
    /// This is used by the integrated card designer so crop/layout is preserved exactly.
    pub fn from_card_rgba(rgba: RgbaImage) -> Result<Self> {
        let (w, h) = rgba.dimensions();
        let rgba = if w == CARD_WIDTH && h == CARD_HEIGHT {
            rgba
        } else {
            DynamicImage::ImageRgba8(rgba)
                .resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3)
                .to_rgba8()
        };
        Self::from_card_rgba_with_source(rgba, w, h)
    }

    fn from_card_rgba_with_source(
        rgba_3x: RgbaImage,
        source_width: u32,
        source_height: u32,
    ) -> Result<Self> {
        // #--- CORRECT @2X / @3X ASSET GENERATION START ---
        let preview = egui::ColorImage::from_rgba_unmultiplied(
            [CARD_WIDTH as usize, CARD_HEIGHT as usize],
            rgba_3x.as_raw(),
        );

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(rgba_3x.clone())
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .context("Could not encode @3x prepared PNG")?;

        let rgba_2x = DynamicImage::ImageRgba8(rgba_3x)
            .resize_exact(CARD_WIDTH_2X, CARD_HEIGHT_2X, FilterType::Lanczos3)
            .to_rgba8();
        let mut png_2x = Vec::new();
        DynamicImage::ImageRgba8(rgba_2x)
            .write_to(&mut Cursor::new(&mut png_2x), ImageFormat::Png)
            .context("Could not encode @2x prepared PNG")?;

        // Transit cards such as Suica may actually render from the PDF, so keep the PDF
        // derived from the exact @3x composition rather than from a separate crop path.
        let pdf = png_to_pdf(&png).context("Could not generate card PDF artwork")?;
        // #--- CORRECT @2X / @3X ASSET GENERATION END ---

        Ok(Self {
            png,
            png_2x,
            pdf,
            preview,
            source_width,
            source_height,
        })
    }
}

pub fn png_to_pdf(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let img =
        image::load_from_memory(png_bytes).context("Failed to decode image for PDF conversion")?;
    let rgb = img.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();
    let raw_bytes = rgb.into_raw();
    let compressed_stream = miniz_oxide::deflate::compress_to_vec_zlib(&raw_bytes, 6);

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    let mut offsets = Vec::new();
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    offsets.push(pdf.len());
    let page_obj = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>\nendobj\n",
        width, height
    );
    pdf.extend_from_slice(page_obj.as_bytes());
    offsets.push(pdf.len());
    let content_stream = format!("q\n{} 0 0 {} 0 0 cm\n/Im0 Do\nQ\n", width, height);
    let contents_obj = format!(
        "4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
        content_stream.len(),
        content_stream
    );
    pdf.extend_from_slice(contents_obj.as_bytes());
    offsets.push(pdf.len());
    let image_header = format!(
        "5 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        width,
        height,
        compressed_stream.len()
    );
    pdf.extend_from_slice(image_header.as_bytes());
    pdf.extend_from_slice(&compressed_stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for &off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }
    let trailer = format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        offsets.len() + 1,
        xref_offset
    );
    pdf.extend_from_slice(trailer.as_bytes());
    Ok(pdf)
}

fn center_crop_for_card(image: DynamicImage) -> DynamicImage {
    let (width, height) = image.dimensions();
    let card_ratio = CARD_WIDTH as f64 / CARD_HEIGHT as f64;
    let source_ratio = width as f64 / height as f64;

    if source_ratio > card_ratio {
        let crop_width = (height as f64 * card_ratio).round() as u32;
        let x = (width - crop_width) / 2;
        image.crop_imm(x, 0, crop_width, height)
    } else {
        let crop_height = (width as f64 / card_ratio).round() as u32;
        let y = (height - crop_height) / 2;
        image.crop_imm(0, y, width, crop_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prepared_skin_sizes() {
        let rgba = RgbaImage::new(CARD_WIDTH, CARD_HEIGHT);
        let skin = PreparedSkin::from_card_rgba(rgba).expect("prepare failed");
        let three = image::load_from_memory(&skin.png).unwrap();
        let two = image::load_from_memory(&skin.png_2x).unwrap();
        assert_eq!(three.dimensions(), (CARD_WIDTH, CARD_HEIGHT));
        assert_eq!(two.dimensions(), (CARD_WIDTH_2X, CARD_HEIGHT_2X));
    }

    #[test]
    fn test_png_to_pdf_conversion() {
        let dummy = DynamicImage::new_rgb8(10, 10);
        let mut png = Vec::new();
        dummy
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        let pdf = png_to_pdf(&png).expect("png_to_pdf failed");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
    }
}
