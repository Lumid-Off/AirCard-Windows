use std::io::Cursor;
use std::path::Path;

use anyhow::{Context, Result};
use eframe::egui;
use image::{DynamicImage, GenericImageView, ImageFormat, imageops::FilterType};

pub const CARD_WIDTH: u32 = 1_536;
pub const CARD_HEIGHT: u32 = 969;

#[derive(Clone)]
pub struct PreparedSkin {
    pub png: Vec<u8>,
    pub preview: egui::ColorImage,
    pub source_width: u32,
    pub source_height: u32,
}

impl PreparedSkin {
    pub fn from_path(path: &Path) -> Result<Self> {
        let image =
            image::open(path).with_context(|| format!("Could not decode {}", path.display()))?;
        Self::from_image(image)
    }

    pub fn from_image(image: DynamicImage) -> Result<Self> {
        let (source_width, source_height) = image.dimensions();
        let cropped = center_crop_for_card(image);
        let final_image = cropped.resize_exact(CARD_WIDTH, CARD_HEIGHT, FilterType::Lanczos3);
        let rgba = final_image.to_rgba8();
        let preview = egui::ColorImage::from_rgba_unmultiplied(
            [CARD_WIDTH as usize, CARD_HEIGHT as usize],
            rgba.as_raw(),
        );

        let mut png = Vec::new();
        DynamicImage::ImageRgba8(rgba)
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .context("Could not encode prepared PNG")?;

        Ok(Self {
            png,
            preview,
            source_width,
            source_height,
        })
    }
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
