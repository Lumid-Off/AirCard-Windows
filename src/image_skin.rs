use std::io::Cursor;

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};

pub const CARD_WIDTH: u32 = 1_536;
pub const CARD_HEIGHT: u32 = 969;

#[derive(Clone)]
pub struct PreparedSkin {
    pub png: Vec<u8>,
    pub pdf: Vec<u8>,
    pub source_width: u32,
    pub source_height: u32,
}

impl PreparedSkin {
    pub fn from_image_with_focus(
        image: DynamicImage,
        focus_x: f32,
        focus_y: f32,
    ) -> Result<Self> {
        let (source_width, source_height) = image.dimensions();
        let cropped = crop_for_card(image, focus_x, focus_y);
        let final_image = cropped.resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3);
        let rgba = final_image.to_rgba8();

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .context("Could not encode prepared PNG")?;

        let pdf = png_to_pdf(&png).context("Could not generate card PDF artwork")?;

        Ok(Self {
            png,
            pdf,
            source_width,
            source_height,
        })
    }
}

pub fn crop_uv_for_card(
    source_width: u32,
    source_height: u32,
    focus_x: f32,
    focus_y: f32,
) -> [f32; 4] {
    let (x, y, width, height) = crop_bounds_for_card(
        source_width,
        source_height,
        focus_x,
        focus_y,
    );

    [
        x as f32 / source_width.max(1) as f32,
        y as f32 / source_height.max(1) as f32,
        (x + width) as f32 / source_width.max(1) as f32,
        (y + height) as f32 / source_height.max(1) as f32,
    ]
}

pub fn png_to_pdf(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let img = image::load_from_memory(png_bytes)
        .context("Failed to decode image for PDF conversion")?;
    let rgb = img.to_rgb8();
    let width = rgb.width();
    let height = rgb.height();
    let raw_bytes = rgb.into_raw();
    let compressed_stream = miniz_oxide::deflate::compress_to_vec_zlib(&raw_bytes, 6);

    let mut pdf = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n");

    let mut offsets = Vec::new();

    // 1 0 obj: Catalog
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    // 2 0 obj: Pages
    offsets.push(pdf.len());
    pdf.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");

    // 3 0 obj: Page
    offsets.push(pdf.len());
    let page_obj = format!(
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Contents 4 0 R /Resources << /XObject << /Im0 5 0 R >> >> >>\nendobj\n",
        width, height
    );
    pdf.extend_from_slice(page_obj.as_bytes());

    // 4 0 obj: Contents stream
    offsets.push(pdf.len());
    let content_stream = format!("q\n{} 0 0 {} 0 0 cm\n/Im0 Do\nQ\n", width, height);
    let contents_obj = format!(
        "4 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
        content_stream.len(),
        content_stream
    );
    pdf.extend_from_slice(contents_obj.as_bytes());

    // 5 0 obj: Image XObject
    offsets.push(pdf.len());
    let image_header = format!(
        "5 0 obj\n<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
        width, height, compressed_stream.len()
    );
    pdf.extend_from_slice(image_header.as_bytes());
    pdf.extend_from_slice(&compressed_stream);
    pdf.extend_from_slice(b"\nendstream\nendobj\n");

    // xref table
    let xref_offset = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", offsets.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for &off in &offsets {
        pdf.extend_from_slice(format!("{:010} 00000 n \n", off).as_bytes());
    }

    // trailer
    let trailer = format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF\n",
        offsets.len() + 1,
        xref_offset
    );
    pdf.extend_from_slice(trailer.as_bytes());

    Ok(pdf)
}

fn crop_for_card(image: DynamicImage, focus_x: f32, focus_y: f32) -> DynamicImage {
    let (width, height) = image.dimensions();
    let (x, y, crop_width, crop_height) =
        crop_bounds_for_card(width, height, focus_x, focus_y);
    image.crop_imm(x, y, crop_width, crop_height)
}

fn crop_bounds_for_card(
    width: u32,
    height: u32,
    focus_x: f32,
    focus_y: f32,
) -> (u32, u32, u32, u32) {
    let card_ratio = CARD_WIDTH as f64 / CARD_HEIGHT as f64;
    let source_ratio = width as f64 / height as f64;

    if source_ratio > card_ratio {
        let crop_width = ((height as f64 * card_ratio).round() as u32).clamp(1, width);
        let max_x = width.saturating_sub(crop_width);
        let x = (focus_x.clamp(0.0, 1.0) * max_x as f32).round() as u32;
        (x, 0, crop_width, height)
    } else {
        let crop_height = ((width as f64 / card_ratio).round() as u32).clamp(1, height);
        let max_y = height.saturating_sub(crop_height);
        let y = (focus_y.clamp(0.0, 1.0) * max_y as f32).round() as u32;
        (0, y, width, crop_height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_png_to_pdf_conversion() {
        let dummy = DynamicImage::new_rgb8(10, 10);
        let mut png = Vec::new();
        dummy.write_to(&mut Cursor::new(&mut png), ImageFormat::Png).unwrap();

        let pdf = png_to_pdf(&png).expect("png_to_pdf failed");
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let pdf_str = String::from_utf8_lossy(&pdf);
        assert!(pdf_str.contains("/FlateDecode"));
        assert!(pdf_str.contains("/MediaBox [0 0 10 10]"));
        assert!(pdf_str.contains("xref"));
    }

    #[test]
    fn crop_focus_moves_toward_image_edges() {
        let left = crop_uv_for_card(2400, 1000, 0.0, 0.5);
        let right = crop_uv_for_card(2400, 1000, 1.0, 0.5);
        assert!(left[0] < right[0]);
        assert!(left[2] < right[2]);
        assert_eq!(left[1], 0.0);
        assert_eq!(right[3], 1.0);
    }
}
