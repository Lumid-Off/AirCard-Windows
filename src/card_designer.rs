use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, PxScale, ScaleFont, point};
use anyhow::{Context, Result, bail};
use image::{Rgba, RgbaImage, imageops::FilterType};
use serde::{Deserialize, Serialize};

use crate::image_skin::{CARD_HEIGHT, CARD_WIDTH, PreparedSkin};

// #--- V9 LAYER CARD DESIGNER START ---
// Layer model follows common editor behavior: every canvas object is a layer,
// exactly one layer is selected for direct manipulation, and z-order is explicit.

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ImageMask {
    Rectangle,
    RoundedRectangle,
    Circle,
}

impl Default for ImageMask {
    fn default() -> Self {
        Self::Rectangle
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum ShapeKind {
    Rectangle,
    RoundedRectangle,
    Circle,
}

impl Default for ShapeKind {
    fn default() -> Self {
        Self::Rectangle
    }
}

#[derive(Clone)]
pub struct ImageLayer {
    pub path: PathBuf,
    pub source: RgbaImage,
    pub scale: f32,
    pub remove_bg: bool,
    pub tolerance: f32,
    pub outline: bool,
    pub outline_width: u32,
    pub mask: ImageMask,
    /// 0.0..=0.5 of the shorter side.
    pub corner_radius: f32,
}

#[derive(Clone)]
pub struct TextLayer {
    pub text: String,
    pub font_size: f32,
    pub color: [u8; 4],
    pub bold: bool,
}

#[derive(Clone)]
pub struct ShapeLayer {
    pub kind: ShapeKind,
    /// Fraction of card width.
    pub width: f32,
    /// Fraction of card height.
    pub height: f32,
    pub fill: [u8; 4],
    pub stroke: [u8; 4],
    pub stroke_width: u32,
    /// 0.0..=0.5 of the shorter side.
    pub corner_radius: f32,
}

#[derive(Clone)]
pub enum DesignerLayerKind {
    Image(ImageLayer),
    Text(TextLayer),
    Shape(ShapeLayer),
}

#[derive(Clone)]
pub struct DesignerLayer {
    pub name: String,
    pub x: f32,
    pub y: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
    pub visible: bool,
    pub locked: bool,
    pub kind: DesignerLayerKind,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct DesignerDraft {
    pub background_path: Option<PathBuf>,
    pub crop_zoom: f32,
    pub crop_x: f32,
    pub crop_y: f32,
    pub reference_path: Option<PathBuf>,
    pub reference_zoom: f32,
    pub reference_x: f32,
    pub reference_y: f32,
    pub reference_opacity: f32,
    pub show_reference: bool,
    pub layers: Vec<LayerDraft>,
    /// v8 compatibility. Restored only when `layers` is empty.
    pub logos: Vec<LogoDraft>,
}

impl Default for DesignerDraft {
    fn default() -> Self {
        Self {
            background_path: None,
            crop_zoom: 1.0,
            crop_x: 0.0,
            crop_y: 0.0,
            reference_path: None,
            reference_zoom: 1.0,
            reference_x: 0.0,
            reference_y: 0.0,
            reference_opacity: 0.40,
            show_reference: true,
            layers: Vec::new(),
            logos: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum LayerDraft {
    Image {
        path: PathBuf,
        name: String,
        x: f32,
        y: f32,
        scale: f32,
        rotation_deg: f32,
        opacity: f32,
        visible: bool,
        locked: bool,
        remove_bg: bool,
        tolerance: f32,
        outline: bool,
        outline_width: u32,
        mask: ImageMask,
        corner_radius: f32,
    },
    Text {
        name: String,
        x: f32,
        y: f32,
        rotation_deg: f32,
        opacity: f32,
        visible: bool,
        locked: bool,
        text: String,
        font_size: f32,
        color: [u8; 4],
        bold: bool,
    },
    Shape {
        name: String,
        x: f32,
        y: f32,
        rotation_deg: f32,
        opacity: f32,
        visible: bool,
        locked: bool,
        kind: ShapeKind,
        width: f32,
        height: f32,
        fill: [u8; 4],
        stroke: [u8; 4],
        stroke_width: u32,
        corner_radius: f32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogoDraft {
    pub path: PathBuf,
    pub x: f32,
    pub y: f32,
    pub scale: f32,
    pub rotation_deg: f32,
    pub opacity: f32,
    pub remove_bg: bool,
    pub tolerance: f32,
    pub outline: bool,
    pub outline_width: u32,
}

pub struct CardDesigner {
    pub background_path: Option<PathBuf>,
    pub background: Option<RgbaImage>,
    pub crop_zoom: f32,
    pub crop_x: f32,
    pub crop_y: f32,

    pub reference_path: Option<PathBuf>,
    pub reference: Option<RgbaImage>,
    pub reference_zoom: f32,
    pub reference_x: f32,
    pub reference_y: f32,
    pub reference_opacity: f32,
    pub show_reference: bool,

    /// Bottom -> top. The last entry is rendered on top.
    pub layers: Vec<DesignerLayer>,
    pub selected_layer: Option<usize>,
    pub dirty: bool,
}

impl Default for CardDesigner {
    fn default() -> Self {
        Self {
            background_path: None,
            background: None,
            crop_zoom: 1.0,
            crop_x: 0.0,
            crop_y: 0.0,
            reference_path: None,
            reference: None,
            reference_zoom: 1.0,
            reference_x: 0.0,
            reference_y: 0.0,
            reference_opacity: 0.40,
            show_reference: true,
            layers: Vec::new(),
            selected_layer: None,
            dirty: true,
        }
    }
}

impl CardDesigner {
    pub fn has_background(&self) -> bool {
        self.background.is_some()
    }

    pub fn load_background(&mut self, path: &Path) -> Result<()> {
        let image = image::open(path)
            .with_context(|| format!("Could not decode background {}", path.display()))?
            .to_rgba8();
        self.background = Some(image);
        self.background_path = Some(path.to_path_buf());
        self.reset_crop();
        self.selected_layer = None;
        self.dirty = true;
        Ok(())
    }

    pub fn load_reference(&mut self, path: &Path) -> Result<()> {
        let image = image::open(path)
            .with_context(|| format!("Could not decode reference {}", path.display()))?
            .to_rgba8();
        self.reference = Some(image);
        self.reference_path = Some(path.to_path_buf());
        self.reference_zoom = 1.0;
        self.reference_x = 0.0;
        self.reference_y = 0.0;
        self.show_reference = true;
        self.dirty = true;
        Ok(())
    }

    pub fn clear_reference(&mut self) {
        self.reference = None;
        self.reference_path = None;
        self.dirty = true;
    }

    pub fn reset_crop(&mut self) {
        self.crop_zoom = 1.0;
        self.crop_x = 0.0;
        self.crop_y = 0.0;
        self.dirty = true;
    }

    pub fn add_image(&mut self, path: &Path) -> Result<()> {
        let source = image::open(path)
            .with_context(|| format!("Could not decode image layer {}", path.display()))?
            .to_rgba8();
        let name = path
            .file_stem()
            .and_then(|v| v.to_str())
            .unwrap_or("Image")
            .to_string();
        self.layers.push(DesignerLayer {
            name,
            x: 0.5,
            y: 0.5,
            rotation_deg: 0.0,
            opacity: 1.0,
            visible: true,
            locked: false,
            kind: DesignerLayerKind::Image(ImageLayer {
                path: path.to_path_buf(),
                source,
                scale: 1.0,
                remove_bg: false,
                tolerance: 24.0,
                outline: false,
                outline_width: 5,
                mask: ImageMask::Rectangle,
                corner_radius: 0.12,
            }),
        });
        self.selected_layer = Some(self.layers.len() - 1);
        self.dirty = true;
        Ok(())
    }

    pub fn add_text(&mut self) {
        self.layers.push(DesignerLayer {
            name: "Text".to_string(),
            x: 0.5,
            y: 0.5,
            rotation_deg: 0.0,
            opacity: 1.0,
            visible: true,
            locked: false,
            kind: DesignerLayerKind::Text(TextLayer {
                text: "Text".to_string(),
                font_size: 72.0,
                color: [255, 255, 255, 255],
                bold: false,
            }),
        });
        self.selected_layer = Some(self.layers.len() - 1);
        self.dirty = true;
    }

    pub fn add_shape(&mut self, kind: ShapeKind) {
        let (name, corner_radius) = match kind {
            ShapeKind::Rectangle => ("Rectangle", 0.0),
            ShapeKind::RoundedRectangle => ("Rounded Rectangle", 0.16),
            ShapeKind::Circle => ("Circle", 0.5),
        };
        self.layers.push(DesignerLayer {
            name: name.to_string(),
            x: 0.5,
            y: 0.5,
            rotation_deg: 0.0,
            opacity: 1.0,
            visible: true,
            locked: false,
            kind: DesignerLayerKind::Shape(ShapeLayer {
                kind,
                width: if kind == ShapeKind::Circle {
                    0.22
                } else {
                    0.32
                },
                height: if kind == ShapeKind::Circle {
                    0.35
                } else {
                    0.22
                },
                fill: [255, 255, 255, 220],
                stroke: [255, 255, 255, 255],
                stroke_width: 0,
                corner_radius,
            }),
        });
        self.selected_layer = Some(self.layers.len() - 1);
        self.dirty = true;
    }

    pub fn remove_selected_layer(&mut self) {
        if let Some(index) = self.selected_layer {
            if index < self.layers.len() {
                self.layers.remove(index);
            }
            self.selected_layer = if self.layers.is_empty() {
                None
            } else {
                Some(index.min(self.layers.len() - 1))
            };
            self.dirty = true;
        }
    }

    pub fn duplicate_selected_layer(&mut self) {
        let Some(index) = self.selected_layer else {
            return;
        };
        if index >= self.layers.len() {
            return;
        }
        let mut copy = self.layers[index].clone();
        copy.name = format!("{} copy", copy.name);
        copy.x = (copy.x + 0.03).min(1.0);
        copy.y = (copy.y + 0.03).min(1.0);
        self.layers.insert(index + 1, copy);
        self.selected_layer = Some(index + 1);
        self.dirty = true;
    }

    /// delta +1 = bring one step forward; -1 = send one step backward.
    pub fn move_selected_layer(&mut self, delta: isize) {
        let Some(index) = self.selected_layer else {
            return;
        };
        if self.layers.is_empty() {
            return;
        }
        let new_index = (index as isize + delta).clamp(0, self.layers.len() as isize - 1) as usize;
        if new_index != index {
            self.layers.swap(index, new_index);
            self.selected_layer = Some(new_index);
            self.dirty = true;
        }
    }

    pub fn bring_selected_to_front(&mut self) {
        let Some(index) = self.selected_layer else {
            return;
        };
        if index + 1 >= self.layers.len() {
            return;
        }
        let item = self.layers.remove(index);
        self.layers.push(item);
        self.selected_layer = Some(self.layers.len() - 1);
        self.dirty = true;
    }

    pub fn send_selected_to_back(&mut self) {
        let Some(index) = self.selected_layer else {
            return;
        };
        if index == 0 || index >= self.layers.len() {
            return;
        }
        let item = self.layers.remove(index);
        self.layers.insert(0, item);
        self.selected_layer = Some(0);
        self.dirty = true;
    }

    pub fn selected_layer_mut(&mut self) -> Option<&mut DesignerLayer> {
        let index = self.selected_layer?;
        self.layers.get_mut(index)
    }

    pub fn hit_test_layer(&self, x: f32, y: f32) -> Option<usize> {
        for index in (0..self.layers.len()).rev() {
            let layer = &self.layers[index];
            if !layer.visible || layer.locked {
                continue;
            }
            let (x0, y0, x1, y1) = self.layer_bounds_norm(index)?;
            if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                return Some(index);
            }
        }
        None
    }

    pub fn layer_bounds_norm(&self, index: usize) -> Option<(f32, f32, f32, f32)> {
        let layer = self.layers.get(index)?;
        let (w, h) = match &layer.kind {
            DesignerLayerKind::Image(image) => {
                let sw = image.source.width().max(1) as f32;
                let sh = image.source.height().max(1) as f32;
                let base_w = CARD_WIDTH as f32 * 0.38;
                let scale = base_w / sw * image.scale.clamp(0.05, 4.0);
                if image.mask == ImageMask::Circle {
                    let side = (sw * scale).min(sh * scale);
                    (side / CARD_WIDTH as f32, side / CARD_HEIGHT as f32)
                } else {
                    (
                        (sw * scale) / CARD_WIDTH as f32,
                        (sh * scale) / CARD_HEIGHT as f32,
                    )
                }
            }
            DesignerLayerKind::Text(text) => {
                let lines: Vec<&str> = text.text.lines().collect();
                let longest = lines
                    .iter()
                    .map(|line| line.chars().count())
                    .max()
                    .unwrap_or(1)
                    .max(1) as f32;
                let line_count = lines.len().max(1) as f32;
                let px_w = longest * text.font_size * 0.72 + 20.0;
                let px_h = line_count * text.font_size * 1.35 + 20.0;
                (px_w / CARD_WIDTH as f32, px_h / CARD_HEIGHT as f32)
            }
            DesignerLayerKind::Shape(shape) => {
                if shape.kind == ShapeKind::Circle {
                    (
                        shape.width,
                        shape.width * CARD_WIDTH as f32 / CARD_HEIGHT as f32,
                    )
                } else {
                    (shape.width, shape.height)
                }
            }
        };
        let hw = (w * 0.5).max(0.005);
        let hh = (h * 0.5).max(0.005);
        Some((layer.x - hw, layer.y - hh, layer.x + hw, layer.y + hh))
    }

    pub fn drag_selected(&mut self, dx_norm: f32, dy_norm: f32) {
        if let Some(layer) = self.selected_layer_mut() {
            if layer.locked {
                return;
            }
            layer.x = (layer.x + dx_norm).clamp(0.0, 1.0);
            layer.y = (layer.y + dy_norm).clamp(0.0, 1.0);
            self.dirty = true;
        }
    }

    /// Canvas drag while background is selected. Positive pointer X drags the image right,
    /// which means the crop window moves left in source coordinates.
    pub fn drag_background(&mut self, dx_norm: f32, dy_norm: f32) {
        self.crop_x = (self.crop_x - dx_norm * 2.0).clamp(-1.0, 1.0);
        self.crop_y = (self.crop_y - dy_norm * 2.0).clamp(-1.0, 1.0);
        self.dirty = true;
    }

    pub fn scale_selected(&mut self, factor: f32) {
        let Some(layer) = self.selected_layer_mut() else {
            self.crop_zoom = (self.crop_zoom * factor).clamp(1.0, 6.0);
            self.dirty = true;
            return;
        };
        if layer.locked {
            return;
        }
        match &mut layer.kind {
            DesignerLayerKind::Image(image) => {
                image.scale = (image.scale * factor).clamp(0.05, 4.0)
            }
            DesignerLayerKind::Text(text) => {
                text.font_size = (text.font_size * factor).clamp(8.0, 360.0)
            }
            DesignerLayerKind::Shape(shape) => {
                shape.width = (shape.width * factor).clamp(0.02, 1.5);
                shape.height = (shape.height * factor).clamp(0.02, 1.5);
            }
        }
        self.dirty = true;
    }

    pub fn draft(&self) -> DesignerDraft {
        let layers = self
            .layers
            .iter()
            .map(|layer| match &layer.kind {
                DesignerLayerKind::Image(image) => LayerDraft::Image {
                    path: image.path.clone(),
                    name: layer.name.clone(),
                    x: layer.x,
                    y: layer.y,
                    scale: image.scale,
                    rotation_deg: layer.rotation_deg,
                    opacity: layer.opacity,
                    visible: layer.visible,
                    locked: layer.locked,
                    remove_bg: image.remove_bg,
                    tolerance: image.tolerance,
                    outline: image.outline,
                    outline_width: image.outline_width,
                    mask: image.mask,
                    corner_radius: image.corner_radius,
                },
                DesignerLayerKind::Text(text) => LayerDraft::Text {
                    name: layer.name.clone(),
                    x: layer.x,
                    y: layer.y,
                    rotation_deg: layer.rotation_deg,
                    opacity: layer.opacity,
                    visible: layer.visible,
                    locked: layer.locked,
                    text: text.text.clone(),
                    font_size: text.font_size,
                    color: text.color,
                    bold: text.bold,
                },
                DesignerLayerKind::Shape(shape) => LayerDraft::Shape {
                    name: layer.name.clone(),
                    x: layer.x,
                    y: layer.y,
                    rotation_deg: layer.rotation_deg,
                    opacity: layer.opacity,
                    visible: layer.visible,
                    locked: layer.locked,
                    kind: shape.kind,
                    width: shape.width,
                    height: shape.height,
                    fill: shape.fill,
                    stroke: shape.stroke,
                    stroke_width: shape.stroke_width,
                    corner_radius: shape.corner_radius,
                },
            })
            .collect();
        DesignerDraft {
            background_path: self.background_path.clone(),
            crop_zoom: self.crop_zoom,
            crop_x: self.crop_x,
            crop_y: self.crop_y,
            reference_path: self.reference_path.clone(),
            reference_zoom: self.reference_zoom,
            reference_x: self.reference_x,
            reference_y: self.reference_y,
            reference_opacity: self.reference_opacity,
            show_reference: self.show_reference,
            layers,
            logos: Vec::new(),
        }
    }

    pub fn restore_draft(&mut self, draft: &DesignerDraft) -> Result<()> {
        *self = Self::default();
        if let Some(path) = &draft.background_path {
            if path.exists() {
                self.load_background(path)?;
                self.crop_zoom = draft.crop_zoom.clamp(1.0, 6.0);
                self.crop_x = draft.crop_x.clamp(-1.0, 1.0);
                self.crop_y = draft.crop_y.clamp(-1.0, 1.0);
            }
        }
        if let Some(path) = &draft.reference_path {
            if path.exists() {
                self.load_reference(path)?;
                self.reference_zoom = draft.reference_zoom.clamp(1.0, 6.0);
                self.reference_x = draft.reference_x.clamp(-1.0, 1.0);
                self.reference_y = draft.reference_y.clamp(-1.0, 1.0);
                self.reference_opacity = draft.reference_opacity.clamp(0.0, 1.0);
                self.show_reference = draft.show_reference;
            }
        }

        if !draft.layers.is_empty() {
            for item in &draft.layers {
                match item {
                    LayerDraft::Image {
                        path,
                        name,
                        x,
                        y,
                        scale,
                        rotation_deg,
                        opacity,
                        visible,
                        locked,
                        remove_bg,
                        tolerance,
                        outline,
                        outline_width,
                        mask,
                        corner_radius,
                    } => {
                        if !path.exists() {
                            continue;
                        }
                        self.add_image(path)?;
                        if let Some(layer) = self.layers.last_mut() {
                            layer.name = name.clone();
                            layer.x = x.clamp(0.0, 1.0);
                            layer.y = y.clamp(0.0, 1.0);
                            layer.rotation_deg = rotation_deg.clamp(-180.0, 180.0);
                            layer.opacity = opacity.clamp(0.0, 1.0);
                            layer.visible = *visible;
                            layer.locked = *locked;
                            if let DesignerLayerKind::Image(image) = &mut layer.kind {
                                image.scale = scale.clamp(0.05, 4.0);
                                image.remove_bg = *remove_bg;
                                image.tolerance = tolerance.clamp(1.0, 120.0);
                                image.outline = *outline;
                                image.outline_width = (*outline_width).min(40);
                                image.mask = *mask;
                                image.corner_radius = corner_radius.clamp(0.0, 0.5);
                            }
                        }
                    }
                    LayerDraft::Text {
                        name,
                        x,
                        y,
                        rotation_deg,
                        opacity,
                        visible,
                        locked,
                        text,
                        font_size,
                        color,
                        bold,
                    } => {
                        self.add_text();
                        if let Some(layer) = self.layers.last_mut() {
                            layer.name = name.clone();
                            layer.x = x.clamp(0.0, 1.0);
                            layer.y = y.clamp(0.0, 1.0);
                            layer.rotation_deg = rotation_deg.clamp(-180.0, 180.0);
                            layer.opacity = opacity.clamp(0.0, 1.0);
                            layer.visible = *visible;
                            layer.locked = *locked;
                            if let DesignerLayerKind::Text(runtime) = &mut layer.kind {
                                runtime.text = text.clone();
                                runtime.font_size = font_size.clamp(8.0, 360.0);
                                runtime.color = *color;
                                runtime.bold = *bold;
                            }
                        }
                    }
                    LayerDraft::Shape {
                        name,
                        x,
                        y,
                        rotation_deg,
                        opacity,
                        visible,
                        locked,
                        kind,
                        width,
                        height,
                        fill,
                        stroke,
                        stroke_width,
                        corner_radius,
                    } => {
                        self.add_shape(*kind);
                        if let Some(layer) = self.layers.last_mut() {
                            layer.name = name.clone();
                            layer.x = x.clamp(0.0, 1.0);
                            layer.y = y.clamp(0.0, 1.0);
                            layer.rotation_deg = rotation_deg.clamp(-180.0, 180.0);
                            layer.opacity = opacity.clamp(0.0, 1.0);
                            layer.visible = *visible;
                            layer.locked = *locked;
                            if let DesignerLayerKind::Shape(runtime) = &mut layer.kind {
                                runtime.kind = *kind;
                                runtime.width = width.clamp(0.02, 1.5);
                                runtime.height = height.clamp(0.02, 1.5);
                                runtime.fill = *fill;
                                runtime.stroke = *stroke;
                                runtime.stroke_width = (*stroke_width).min(80);
                                runtime.corner_radius = corner_radius.clamp(0.0, 0.5);
                            }
                        }
                    }
                }
            }
        } else {
            // v8 migration.
            for old in &draft.logos {
                if !old.path.exists() {
                    continue;
                }
                self.add_image(&old.path)?;
                if let Some(layer) = self.layers.last_mut() {
                    layer.x = old.x.clamp(0.0, 1.0);
                    layer.y = old.y.clamp(0.0, 1.0);
                    layer.rotation_deg = old.rotation_deg.clamp(-180.0, 180.0);
                    layer.opacity = old.opacity.clamp(0.0, 1.0);
                    if let DesignerLayerKind::Image(image) = &mut layer.kind {
                        image.scale = old.scale.clamp(0.05, 4.0);
                        image.remove_bg = old.remove_bg;
                        image.tolerance = old.tolerance.clamp(1.0, 120.0);
                        image.outline = old.outline;
                        image.outline_width = old.outline_width.min(40);
                    }
                }
            }
        }
        self.selected_layer = self.layers.len().checked_sub(1);
        self.dirty = true;
        Ok(())
    }

    /// Preview includes the optional reference overlay. Export/Wallet output never does.
    pub fn render_preview(&self) -> RgbaImage {
        self.render_internal(true)
    }

    pub fn render_export(&self) -> Result<RgbaImage> {
        if self.background.is_none() {
            bail!("No background image is loaded");
        }
        Ok(self.render_internal(false))
    }

    pub fn prepared_skin(&self) -> Result<PreparedSkin> {
        PreparedSkin::from_card_rgba(self.render_export()?)
    }

    fn render_internal(&self, include_reference: bool) -> RgbaImage {
        let mut canvas = RgbaImage::from_pixel(CARD_WIDTH, CARD_HEIGHT, Rgba([24, 28, 36, 255]));
        if let Some(background) = &self.background {
            canvas = crop_fill(background, self.crop_zoom, self.crop_x, self.crop_y);
        }

        if include_reference && self.show_reference {
            if let Some(reference) = &self.reference {
                let ref_img = crop_fill(
                    reference,
                    self.reference_zoom,
                    self.reference_x,
                    self.reference_y,
                );
                overlay_image(&mut canvas, &ref_img, 0, 0, self.reference_opacity);
            }
        }

        for layer in &self.layers {
            if !layer.visible {
                continue;
            }
            match &layer.kind {
                DesignerLayerKind::Image(image) => draw_image_layer(&mut canvas, layer, image),
                DesignerLayerKind::Text(text) => draw_text_layer(&mut canvas, layer, text),
                DesignerLayerKind::Shape(shape) => draw_shape_layer(&mut canvas, layer, shape),
            }
        }
        canvas
    }
}

fn crop_fill(source: &RgbaImage, zoom: f32, pan_x: f32, pan_y: f32) -> RgbaImage {
    let sw = source.width().max(1);
    let sh = source.height().max(1);
    let base_scale = (CARD_WIDTH as f32 / sw as f32).max(CARD_HEIGHT as f32 / sh as f32);
    let scale = base_scale * zoom.clamp(1.0, 6.0);
    let rw = ((sw as f32 * scale).round() as u32).max(CARD_WIDTH);
    let rh = ((sh as f32 * scale).round() as u32).max(CARD_HEIGHT);
    let resized = image::imageops::resize(source, rw, rh, FilterType::Lanczos3);
    let extra_x = rw.saturating_sub(CARD_WIDTH);
    let extra_y = rh.saturating_sub(CARD_HEIGHT);
    let px = ((pan_x.clamp(-1.0, 1.0) + 1.0) * 0.5 * extra_x as f32)
        .round()
        .clamp(0.0, extra_x as f32) as u32;
    let py = ((pan_y.clamp(-1.0, 1.0) + 1.0) * 0.5 * extra_y as f32)
        .round()
        .clamp(0.0, extra_y as f32) as u32;
    image::imageops::crop_imm(&resized, px, py, CARD_WIDTH, CARD_HEIGHT).to_image()
}

fn draw_image_layer(canvas: &mut RgbaImage, layer: &DesignerLayer, image: &ImageLayer) {
    let mut source = image.source.clone();
    if image.remove_bg {
        key_out_background(&mut source, image.tolerance);
    }
    let sw = source.width().max(1);
    let sh = source.height().max(1);
    let base_w = CARD_WIDTH as f32 * 0.38;
    let base_scale = base_w / sw as f32;
    let scale = base_scale * image.scale.clamp(0.05, 4.0);
    let dw = ((sw as f32 * scale).round() as u32).max(1);
    let dh = ((sh as f32 * scale).round() as u32).max(1);
    let resized = image::imageops::resize(&source, dw, dh, FilterType::Lanczos3);
    let mut masked = if image.mask == ImageMask::Circle {
        let side = resized.width().min(resized.height()).max(1);
        let x = (resized.width() - side) / 2;
        let y = (resized.height() - side) / 2;
        image::imageops::crop_imm(&resized, x, y, side, side).to_image()
    } else {
        resized
    };
    apply_image_mask(&mut masked, image.mask, image.corner_radius);
    let rotated = rotate_rgba(&masked, layer.rotation_deg);
    let cx = (layer.x.clamp(0.0, 1.0) * CARD_WIDTH as f32).round() as i32;
    let cy = (layer.y.clamp(0.0, 1.0) * CARD_HEIGHT as f32).round() as i32;
    let left = cx - rotated.width() as i32 / 2;
    let top = cy - rotated.height() as i32 / 2;

    if image.outline && image.outline_width > 0 {
        let outline_color = auto_outline_color(&source);
        let radius = image.outline_width.min(40) as i32;
        let steps = 28;
        for i in 0..steps {
            let angle = std::f32::consts::TAU * i as f32 / steps as f32;
            let ox = (angle.cos() * radius as f32).round() as i32;
            let oy = (angle.sin() * radius as f32).round() as i32;
            overlay_tinted_alpha(
                canvas,
                &rotated,
                left + ox,
                top + oy,
                layer.opacity,
                outline_color,
            );
        }
    }
    overlay_image(canvas, &rotated, left, top, layer.opacity);
}

fn draw_text_layer(canvas: &mut RgbaImage, layer: &DesignerLayer, text: &TextLayer) {
    if text.text.is_empty() {
        return;
    }
    let rendered = render_text_rgba(text);
    if rendered.width() == 0 || rendered.height() == 0 {
        return;
    }
    let rotated = rotate_rgba(&rendered, layer.rotation_deg);
    let cx = (layer.x.clamp(0.0, 1.0) * CARD_WIDTH as f32).round() as i32;
    let cy = (layer.y.clamp(0.0, 1.0) * CARD_HEIGHT as f32).round() as i32;
    let left = cx - rotated.width() as i32 / 2;
    let top = cy - rotated.height() as i32 / 2;
    overlay_image(canvas, &rotated, left, top, layer.opacity);
}

fn draw_shape_layer(canvas: &mut RgbaImage, layer: &DesignerLayer, shape: &ShapeLayer) {
    let w = (shape.width.clamp(0.02, 1.5) * CARD_WIDTH as f32)
        .round()
        .max(2.0) as u32;
    let h = if shape.kind == ShapeKind::Circle {
        // A circle remains a true circle even though X/Y use normalized card coordinates.
        w
    } else {
        (shape.height.clamp(0.02, 1.5) * CARD_HEIGHT as f32)
            .round()
            .max(2.0) as u32
    };
    let local = render_shape_rgba(shape, w, h);
    let rotated = rotate_rgba(&local, layer.rotation_deg);
    let cx = (layer.x.clamp(0.0, 1.0) * CARD_WIDTH as f32).round() as i32;
    let cy = (layer.y.clamp(0.0, 1.0) * CARD_HEIGHT as f32).round() as i32;
    let left = cx - rotated.width() as i32 / 2;
    let top = cy - rotated.height() as i32 / 2;
    overlay_image(canvas, &rotated, left, top, layer.opacity);
}

fn apply_image_mask(image: &mut RgbaImage, mask: ImageMask, corner_radius: f32) {
    if mask == ImageMask::Rectangle {
        return;
    }
    let w = image.width().max(1) as f32;
    let h = image.height().max(1) as f32;
    for y in 0..image.height() {
        for x in 0..image.width() {
            let inside = match mask {
                ImageMask::Rectangle => true,
                ImageMask::Circle => ellipse_inside(x as f32 + 0.5, y as f32 + 0.5, w, h, 0.0),
                ImageMask::RoundedRectangle => rounded_rect_inside(
                    x as f32 + 0.5,
                    y as f32 + 0.5,
                    w,
                    h,
                    corner_radius.clamp(0.0, 0.5) * w.min(h),
                ),
            };
            if !inside {
                image.get_pixel_mut(x, y)[3] = 0;
            }
        }
    }
}

fn render_shape_rgba(shape: &ShapeLayer, w: u32, h: u32) -> RgbaImage {
    let mut out = RgbaImage::new(w, h);
    let fw = w as f32;
    let fh = h as f32;
    let stroke = shape.stroke_width.min(80) as f32;
    for y in 0..h {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let outer = match shape.kind {
                ShapeKind::Rectangle => true,
                ShapeKind::RoundedRectangle => rounded_rect_inside(
                    px,
                    py,
                    fw,
                    fh,
                    shape.corner_radius.clamp(0.0, 0.5) * fw.min(fh),
                ),
                ShapeKind::Circle => ellipse_inside(px, py, fw, fh, 0.0),
            };
            if !outer {
                continue;
            }
            let on_stroke = if stroke <= 0.0 {
                false
            } else {
                let iw = (fw - stroke * 2.0).max(0.0);
                let ih = (fh - stroke * 2.0).max(0.0);
                if iw <= 1.0 || ih <= 1.0 {
                    true
                } else {
                    let inner = match shape.kind {
                        ShapeKind::Rectangle => {
                            px >= stroke && px < fw - stroke && py >= stroke && py < fh - stroke
                        }
                        ShapeKind::RoundedRectangle => rounded_rect_inside(
                            px - stroke,
                            py - stroke,
                            iw,
                            ih,
                            (shape.corner_radius.clamp(0.0, 0.5) * fw.min(fh) - stroke).max(0.0),
                        ),
                        ShapeKind::Circle => ellipse_inside(px - stroke, py - stroke, iw, ih, 0.0),
                    };
                    !inner
                }
            };
            let color = if on_stroke { shape.stroke } else { shape.fill };
            out.put_pixel(x, y, Rgba(color));
        }
    }
    out
}

fn rounded_rect_inside(x: f32, y: f32, w: f32, h: f32, radius: f32) -> bool {
    let r = radius.clamp(0.0, w.min(h) * 0.5);
    if r <= 0.5 {
        return x >= 0.0 && x <= w && y >= 0.0 && y <= h;
    }
    if x >= r && x <= w - r {
        return y >= 0.0 && y <= h;
    }
    if y >= r && y <= h - r {
        return x >= 0.0 && x <= w;
    }
    let cx = if x < r { r } else { w - r };
    let cy = if y < r { r } else { h - r };
    let dx = x - cx;
    let dy = y - cy;
    dx * dx + dy * dy <= r * r
}

fn ellipse_inside(x: f32, y: f32, w: f32, h: f32, _unused: f32) -> bool {
    let rx = (w * 0.5).max(0.5);
    let ry = (h * 0.5).max(0.5);
    let dx = (x - rx) / rx;
    let dy = (y - ry) / ry;
    dx * dx + dy * dy <= 1.0
}

static REGULAR_FONTS: OnceLock<Vec<FontArc>> = OnceLock::new();
static BOLD_FONTS: OnceLock<Vec<FontArc>> = OnceLock::new();

fn system_fonts(bold: bool) -> &'static Vec<FontArc> {
    let cell = if bold { &BOLD_FONTS } else { &REGULAR_FONTS };
    cell.get_or_init(|| {
        #[cfg(windows)]
        let candidates: &[&str] = if bold {
            &[
                r"C:\Windows\Fonts\msjhbd.ttc",
                r"C:\Windows\Fonts\msyhbd.ttc",
                r"C:\Windows\Fonts\YuGothB.ttc",
                r"C:\Windows\Fonts\malgunbd.ttf",
                r"C:\Windows\Fonts\NirmalaB.ttf",
                r"C:\Windows\Fonts\segoeuib.ttf",
                r"C:\Windows\Fonts\arialbd.ttf",
            ]
        } else {
            &[
                r"C:\Windows\Fonts\msjh.ttc",
                r"C:\Windows\Fonts\msyh.ttc",
                r"C:\Windows\Fonts\YuGothR.ttc",
                r"C:\Windows\Fonts\malgun.ttf",
                r"C:\Windows\Fonts\Nirmala.ttf",
                r"C:\Windows\Fonts\LeelawUI.ttf",
                r"C:\Windows\Fonts\segoeui.ttf",
                r"C:\Windows\Fonts\arial.ttf",
            ]
        };
        #[cfg(not(windows))]
        let candidates: &[&str] = &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
        ];
        let mut fonts = Vec::new();
        for path in candidates {
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if let Ok(font) = FontArc::try_from_vec(bytes) {
                fonts.push(font);
            }
        }
        fonts
    })
}

fn font_for_char(ch: char, bold: bool) -> Option<&'static FontArc> {
    let fonts = system_fonts(bold);
    fonts
        .iter()
        .find(|font| font.glyph_id(ch).0 != 0)
        .or_else(|| fonts.first())
}

fn render_text_rgba(text: &TextLayer) -> RgbaImage {
    let size = text.font_size.clamp(8.0, 360.0);
    let lines: Vec<&str> = text.text.split('\n').collect();
    let line_height = size * 1.34;
    let mut max_width = 1.0f32;
    for line in &lines {
        let mut width = 0.0f32;
        for ch in line.chars() {
            if let Some(font) = font_for_char(ch, text.bold) {
                let scaled = font.as_scaled(PxScale::from(size));
                width += scaled.h_advance(font.glyph_id(ch));
            } else {
                width += size * 0.6;
            }
        }
        max_width = max_width.max(width);
    }
    let pad = (size * 0.22).ceil().max(4.0);
    let width = (max_width + pad * 2.0).ceil().max(2.0) as u32;
    let height = (line_height * lines.len().max(1) as f32 + pad * 2.0)
        .ceil()
        .max(2.0) as u32;
    let mut out = RgbaImage::new(width, height);
    for (line_index, line) in lines.iter().enumerate() {
        let mut cursor_x = pad;
        let baseline = pad + size + line_index as f32 * line_height;
        for ch in line.chars() {
            let Some(font) = font_for_char(ch, text.bold) else {
                cursor_x += size * 0.6;
                continue;
            };
            let scale = PxScale::from(size);
            let scaled = font.as_scaled(scale);
            let glyph_id = font.glyph_id(ch);
            let advance = scaled.h_advance(glyph_id);
            let glyph = glyph_id.with_scale_and_position(scale, point(cursor_x, baseline));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x.floor() as i32 + gx as i32;
                    let py = bounds.min.y.floor() as i32 + gy as i32;
                    if px < 0 || py < 0 || px >= out.width() as i32 || py >= out.height() as i32 {
                        return;
                    }
                    let mut src = Rgba(text.color);
                    src[3] = ((src[3] as f32) * coverage).round().clamp(0.0, 255.0) as u8;
                    let dst = out.get_pixel_mut(px as u32, py as u32);
                    alpha_blend(dst, src, 1.0);
                });
            }
            cursor_x += advance;
        }
    }
    out
}

fn key_out_background(image: &mut RgbaImage, tolerance: f32) {
    let w = image.width().max(1);
    let h = image.height().max(1);
    let corners = [
        *image.get_pixel(0, 0),
        *image.get_pixel(w - 1, 0),
        *image.get_pixel(0, h - 1),
        *image.get_pixel(w - 1, h - 1),
    ];
    let mut avg = [0.0f32; 3];
    for px in corners {
        avg[0] += px[0] as f32;
        avg[1] += px[1] as f32;
        avg[2] += px[2] as f32;
    }
    for value in &mut avg {
        *value /= 4.0;
    }
    let tol = tolerance.clamp(1.0, 120.0);
    for px in image.pixels_mut() {
        let dr = px[0] as f32 - avg[0];
        let dg = px[1] as f32 - avg[1];
        let db = px[2] as f32 - avg[2];
        let dist = (dr * dr + dg * dg + db * db).sqrt();
        if dist < tol {
            px[3] = ((dist / tol) * px[3] as f32).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn auto_outline_color(image: &RgbaImage) -> Rgba<u8> {
    let mut sum = 0.0f64;
    let mut count = 0u64;
    let stride = ((image.width().max(image.height()) / 96).max(1)) as usize;
    for y in (0..image.height()).step_by(stride) {
        for x in (0..image.width()).step_by(stride) {
            let p = image.get_pixel(x, y);
            if p[3] < 128 {
                continue;
            }
            sum += 0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64;
            count += 1;
        }
    }
    let lum = if count == 0 {
        0.5
    } else {
        sum / count as f64 / 255.0
    };
    if lum >= 0.5 {
        Rgba([17, 26, 43, 255])
    } else {
        Rgba([255, 255, 255, 255])
    }
}

fn rotate_rgba(source: &RgbaImage, degrees: f32) -> RgbaImage {
    let angle = degrees.to_radians();
    if angle.abs() < 0.0001 {
        return source.clone();
    }
    let sin = angle.sin();
    let cos = angle.cos();
    let sw = source.width() as f32;
    let sh = source.height() as f32;
    let dw = (sw * cos.abs() + sh * sin.abs()).ceil().max(1.0) as u32;
    let dh = (sw * sin.abs() + sh * cos.abs()).ceil().max(1.0) as u32;
    let mut out = RgbaImage::new(dw, dh);
    let scx = (sw - 1.0) * 0.5;
    let scy = (sh - 1.0) * 0.5;
    let dcx = (dw as f32 - 1.0) * 0.5;
    let dcy = (dh as f32 - 1.0) * 0.5;
    for y in 0..dh {
        for x in 0..dw {
            let dx = x as f32 - dcx;
            let dy = y as f32 - dcy;
            let sx = cos * dx + sin * dy + scx;
            let sy = -sin * dx + cos * dy + scy;
            if sx >= 0.0 && sy >= 0.0 && sx < sw && sy < sh {
                let ix = sx.round().clamp(0.0, sw - 1.0) as u32;
                let iy = sy.round().clamp(0.0, sh - 1.0) as u32;
                out.put_pixel(x, y, *source.get_pixel(ix, iy));
            }
        }
    }
    out
}

fn overlay_image(dst: &mut RgbaImage, src: &RgbaImage, left: i32, top: i32, opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    for sy in 0..src.height() {
        let dy = top + sy as i32;
        if dy < 0 || dy >= dst.height() as i32 {
            continue;
        }
        for sx in 0..src.width() {
            let dx = left + sx as i32;
            if dx < 0 || dx >= dst.width() as i32 {
                continue;
            }
            let src_px = *src.get_pixel(sx, sy);
            if src_px[3] == 0 {
                continue;
            }
            let dst_px = dst.get_pixel_mut(dx as u32, dy as u32);
            alpha_blend(dst_px, src_px, opacity);
        }
    }
}

fn overlay_tinted_alpha(
    dst: &mut RgbaImage,
    src: &RgbaImage,
    left: i32,
    top: i32,
    opacity: f32,
    color: Rgba<u8>,
) {
    let opacity = opacity.clamp(0.0, 1.0);
    for sy in 0..src.height() {
        let dy = top + sy as i32;
        if dy < 0 || dy >= dst.height() as i32 {
            continue;
        }
        for sx in 0..src.width() {
            let dx = left + sx as i32;
            if dx < 0 || dx >= dst.width() as i32 {
                continue;
            }
            let alpha = src.get_pixel(sx, sy)[3];
            if alpha == 0 {
                continue;
            }
            let tinted = Rgba([color[0], color[1], color[2], alpha]);
            let dst_px = dst.get_pixel_mut(dx as u32, dy as u32);
            alpha_blend(dst_px, tinted, opacity);
        }
    }
}

fn alpha_blend(dst: &mut Rgba<u8>, src: Rgba<u8>, opacity: f32) {
    let sa = (src[3] as f32 / 255.0) * opacity;
    if sa <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.0 {
        *dst = Rgba([0, 0, 0, 0]);
        return;
    }
    for i in 0..3 {
        let sv = src[i] as f32 / 255.0;
        let dv = dst[i] as f32 / 255.0;
        let out = (sv * sa + dv * da * (1.0 - sa)) / out_a;
        dst[i] = (out * 255.0).round().clamp(0.0, 255.0) as u8;
    }
    dst[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_layer_moves_independently() {
        let mut designer = CardDesigner::default();
        designer.add_text();
        designer.layers[0].name = "A".to_string();
        designer.add_shape(ShapeKind::Rectangle);
        designer.layers[1].name = "B".to_string();

        designer.selected_layer = Some(0);
        let before_b = (designer.layers[1].x, designer.layers[1].y);
        designer.drag_selected(0.10, -0.05);

        assert!((designer.layers[0].x - 0.60).abs() < 0.001);
        assert!((designer.layers[0].y - 0.45).abs() < 0.001);
        assert_eq!((designer.layers[1].x, designer.layers[1].y), before_b);
    }

    #[test]
    fn layer_order_controls_preserve_selected_object() {
        let mut designer = CardDesigner::default();
        designer.add_text();
        designer.layers[0].name = "bottom".to_string();
        designer.add_shape(ShapeKind::Circle);
        designer.layers[1].name = "top".to_string();

        designer.selected_layer = Some(0);
        designer.bring_selected_to_front();
        assert_eq!(designer.layers.last().unwrap().name, "bottom");
        assert_eq!(designer.selected_layer, Some(1));

        designer.send_selected_to_back();
        assert_eq!(designer.layers.first().unwrap().name, "bottom");
        assert_eq!(designer.selected_layer, Some(0));
    }

    #[test]
    fn locked_layer_does_not_move() {
        let mut designer = CardDesigner::default();
        designer.add_text();
        designer.layers[0].locked = true;
        designer.selected_layer = Some(0);
        let before = (designer.layers[0].x, designer.layers[0].y);
        designer.drag_selected(0.2, 0.2);
        assert_eq!((designer.layers[0].x, designer.layers[0].y), before);
    }
}

// #--- V9 LAYER CARD DESIGNER END ---
