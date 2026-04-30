//! Screenshot annotation utilities for the SkyVM API.

use ab_glyph::{FontRef, PxScale};
use image::{ImageReader, Rgba, RgbaImage};
use imageproc::drawing::draw_text_mut;
use std::io::Cursor;
use std::path::Path;
use std::sync::OnceLock;

use crate::accessibility::{Element, Rect, Screenshot};

// Platform-specific font data - embedded at compile time
const FONT_DATA: &[u8] = include_bytes!("../../assets/Arial Bold.ttf");

// Cached font reference - parsed once and reused
static CACHED_FONT: OnceLock<FontRef<'static>> = OnceLock::new();

/// Get the cached font reference, parsing it once on first access.
fn get_font() -> &'static FontRef<'static> {
    CACHED_FONT
        .get_or_init(|| FontRef::try_from_slice(FONT_DATA).expect("Failed to load embedded font"))
}

/// Decode a screenshot's PNG data to an RgbaImage.
pub fn decode_screenshot(screenshot: &Screenshot) -> RgbaImage {
    ImageReader::new(Cursor::new(&screenshot.data))
        .with_guessed_format()
        .expect("Failed to read image format")
        .decode()
        .expect("Failed to decode image")
        .into_rgba8()
}

/// Annotate elements on an image with colored boxes.
///
/// # Arguments
/// * `img` - The image to annotate
/// * `elements` - The elements to draw boxes around
/// * `screen_bounds` - The screen bounds for coordinate conversion
/// * `screenshot` - The screenshot for dimensions
/// * `draw_labels` - Whether to draw numbered labels on elements
pub fn annotate_elements(
    img: &mut RgbaImage,
    elements: &[&Element],
    screen_bounds: &Rect,
    screenshot: &Screenshot,
    draw_labels: bool,
) {
    let scale_x = screenshot.width as f64 / screen_bounds.size.width;
    let scale_y = screenshot.height as f64 / screen_bounds.size.height;
    let red = Rgba([255u8, 0, 0, 255]);
    let white = Rgba([255u8, 255, 255, 255]);
    let box_thickness = 3u32;

    let font = get_font();
    let base_font_size = 24.0;
    let font_scale = PxScale::from(base_font_size);

    for elem in elements {
        if let Some(bounds) = &elem.bounds {
            let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x).round() as i32;
            let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y).round() as i32;
            let pw = (bounds.size.width * scale_x).round() as i32;
            let ph = (bounds.size.height * scale_y).round() as i32;

            // Skip elements outside the image bounds
            if px < 0 || py < 0 || px >= img.width() as i32 || py >= img.height() as i32 {
                continue;
            }

            // Draw rectangle border
            draw_rect_border(img, px, py, pw, ph, red, box_thickness);

            // Draw label if requested
            if draw_labels {
                // Use the actual element ID
                let label_text = format!("{}", elem.id.to_ffi());
                let label_h = (base_font_size * 1.1) as u32;
                let label_w = (base_font_size * 0.6 * label_text.len() as f32) as u32 + 6;

                let label_x = (px.max(0) + 2) as u32;
                let label_y = (py.max(0) + 2) as u32;

                // Draw label background
                for lx in label_x..=(label_x + label_w).min(img.width() - 1) {
                    for ly in label_y..=(label_y + label_h).min(img.height() - 1) {
                        img.put_pixel(lx, ly, red);
                    }
                }

                // Draw white text
                draw_text_mut(
                    img,
                    white,
                    label_x as i32 + 4,
                    label_y as i32 + 2,
                    font_scale,
                    font,
                    &label_text,
                );
            }
        }
    }
}

/// Draw a coordinate grid overlay on an image.
///
/// # Arguments
/// * `img` - The image to draw on
/// * `grid_size` - The grid cell size in screen points
/// * `screen_bounds` - The screen bounds for coordinate conversion
/// * `screenshot_width` - The screenshot width in pixels
/// * `screenshot_height` - The screenshot height in pixels
pub fn draw_grid_overlay(
    img: &mut RgbaImage,
    grid_size: u32,
    screen_bounds: &Rect,
    screenshot_width: u32,
    screenshot_height: u32,
) {
    let scale_x = screenshot_width as f64 / screen_bounds.size.width;
    let scale_y = screenshot_height as f64 / screen_bounds.size.height;

    let grid_color = Rgba([128u8, 128, 128, 180]);
    let label_bg = Rgba([0u8, 0, 0, 180]);
    let label_text_color = Rgba([255u8, 255, 255, 255]);

    let font = get_font();
    let font_size = 12.0;
    let font_scale = PxScale::from(font_size);

    let grid_px_x = (grid_size as f64 * scale_x) as u32;
    let grid_px_y = (grid_size as f64 * scale_y) as u32;

    // Draw vertical lines
    let mut x_px: u32 = 0;
    while x_px < img.width() {
        for y in 0..img.height() {
            img.put_pixel(x_px, y, grid_color);
        }
        x_px += grid_px_x;
    }

    // Draw horizontal lines
    let mut y_px: u32 = 0;
    while y_px < img.height() {
        for x in 0..img.width() {
            img.put_pixel(x, y_px, grid_color);
        }
        y_px += grid_px_y;
    }

    // Draw coordinate labels
    let mut y_screen = screen_bounds.origin.y as u32;
    let mut y_px: u32 = 0;
    while y_px < img.height() {
        let mut x_screen = screen_bounds.origin.x as u32;
        let mut label_x_px: u32 = 0;
        while label_x_px < img.width() {
            let label = format!("{},{}", x_screen, y_screen);

            let label_w = (font_size * 0.6 * label.len() as f32) as u32 + 4;
            let label_h = (font_size * 1.2) as u32;

            let cell_center_x = label_x_px + grid_px_x / 2;
            let cell_center_y = y_px + grid_px_y / 2;

            let bg_x = cell_center_x.saturating_sub(label_w / 2);
            let bg_y = cell_center_y.saturating_sub(label_h / 2);

            // Draw background
            for lx in bg_x..(bg_x + label_w).min(img.width()) {
                for ly in bg_y..(bg_y + label_h).min(img.height()) {
                    img.put_pixel(lx, ly, label_bg);
                }
            }

            // Draw text
            draw_text_mut(
                img,
                label_text_color,
                (bg_x + 2) as i32,
                (bg_y + 1) as i32,
                font_scale,
                font,
                &label,
            );

            x_screen += grid_size;
            label_x_px += grid_px_x;
        }
        y_screen += grid_size;
        y_px += grid_px_y;
    }
}

/// Draw a rectangular border on an image.
pub fn draw_rect_border(
    img: &mut RgbaImage,
    px: i32,
    py: i32,
    pw: i32,
    ph: i32,
    color: Rgba<u8>,
    thickness: u32,
) {
    for t in 0..thickness as i32 {
        // Top edge
        for x in px.max(0)..(px + pw).min(img.width() as i32) {
            let y = py + t;
            if y >= 0 && y < img.height() as i32 {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
        // Bottom edge
        for x in px.max(0)..(px + pw).min(img.width() as i32) {
            let y = py + ph - 1 - t;
            if y >= 0 && y < img.height() as i32 {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
        // Left edge
        for y in py.max(0)..(py + ph).min(img.height() as i32) {
            let x = px + t;
            if x >= 0 && x < img.width() as i32 {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
        // Right edge
        for y in py.max(0)..(py + ph).min(img.height() as i32) {
            let x = px + pw - 1 - t;
            if x >= 0 && x < img.width() as i32 {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
    }
}

/// An annotated screenshot with element boxes drawn.
///
/// This provides a builder pattern for creating annotated screenshots.
pub struct AnnotatedScreenshot {
    /// The underlying image.
    image: RgbaImage,

    /// The original screenshot dimensions.
    width: u32,
    height: u32,

    /// Screen bounds used for coordinate conversion.
    screen_bounds: Rect,

    /// Element labels (if labels were drawn).
    labels: Vec<ElementLabel>,
}

/// A label drawn on an annotated screenshot.
#[derive(Debug, Clone)]
pub struct ElementLabel {
    /// The label number.
    pub number: u32,

    /// The element ID.
    pub element_id: crate::accessibility::ElementKey,

    /// The element role.
    pub role: accesskit::Role,

    /// The element's display label.
    pub label: String,

    /// The pixel coordinates of the label.
    pub pixel_x: i32,
    pub pixel_y: i32,
}

impl AnnotatedScreenshot {
    /// Create a new annotated screenshot.
    pub fn new(
        screenshot: Screenshot,
        screen_bounds: Rect,
        elements: Vec<&Element>,
        draw_labels: bool,
    ) -> Self {
        let mut image = decode_screenshot(&screenshot);
        let width = screenshot.width;
        let height = screenshot.height;

        let mut labels = Vec::new();

        if draw_labels {
            // Collect labels before annotating
            let scale_x = screenshot.width as f64 / screen_bounds.size.width;
            let scale_y = screenshot.height as f64 / screen_bounds.size.height;

            for elem in &elements {
                if let Some(bounds) = &elem.bounds {
                    let px = ((bounds.origin.x - screen_bounds.origin.x) * scale_x).round() as i32;
                    let py = ((bounds.origin.y - screen_bounds.origin.y) * scale_y).round() as i32;

                    if px >= 0 && py >= 0 && px < image.width() as i32 && py < image.height() as i32
                    {
                        labels.push(ElementLabel {
                            number: elem.id.to_ffi() as u32,
                            element_id: elem.id,
                            role: elem.role,
                            label: elem.display_label(),
                            pixel_x: px,
                            pixel_y: py,
                        });
                    }
                }
            }
        }

        annotate_elements(
            &mut image,
            &elements,
            &screen_bounds,
            &screenshot,
            draw_labels,
        );

        Self {
            image,
            width,
            height,
            screen_bounds,
            labels,
        }
    }

    /// Add a grid overlay to the screenshot.
    pub fn with_grid(mut self, grid_size: u32) -> Self {
        draw_grid_overlay(
            &mut self.image,
            grid_size,
            &self.screen_bounds,
            self.width,
            self.height,
        );
        self
    }

    /// Get the image dimensions.
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Get the element labels.
    pub fn labels(&self) -> &[ElementLabel] {
        &self.labels
    }

    /// Get the underlying image.
    pub fn image(&self) -> &RgbaImage {
        &self.image
    }

    /// Save the image to a file.
    pub fn save(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        self.image
            .save(path.as_ref())
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Encode the image as PNG bytes.
    pub fn to_png(&self) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        self.image
            .write_to(&mut buffer, image::ImageFormat::Png)
            .expect("Failed to encode PNG");
        buffer.into_inner()
    }
}
