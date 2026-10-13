//! Coordinate spaces (PRD section 5).
//!
//! The broker's canonical space is *desktop logical*: one unified plane in logical (scaled)
//! pixels, where each display has an origin that may be negative. Every other space maps into
//! it, and every mapping is tied to a layout generation so stale coordinates are rejected.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Rotation {
    #[default]
    Normal,
    Rotate90,
    Rotate180,
    Rotate270,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    /// Position and size in desktop logical pixels, after rotation and scaling.
    pub logical: Rect,
    /// Physical pixels per logical pixel.
    pub scale: f64,
    pub rotation: Rotation,
    pub primary: bool,
}

impl DisplayInfo {
    /// Physical pixel size of the display as captured (already rotated as displayed).
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            (self.logical.width * self.scale).round() as u32,
            (self.logical.height * self.scale).round() as u32,
        )
    }
}

/// Which space a point is expressed in. A point without an explicit space is rejected.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "space", rename_all = "snake_case")]
pub enum Space {
    /// Desktop logical pixels.
    Desktop,
    /// Logical pixels relative to the display's top-left corner.
    Display { display: String },
    /// Logical pixels relative to the window's top-left corner.
    Window { window: String },
    /// Pixels of a specific screenshot. Only valid while that capture's layout generation holds.
    Capture { capture: String },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    #[serde(flatten)]
    pub space: Space,
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn desktop(x: f64, y: f64) -> Self {
        Self { space: Space::Desktop, x, y }
    }
    pub fn display(display: impl Into<String>, x: f64, y: f64) -> Self {
        Self { space: Space::Display { display: display.into() }, x, y }
    }
    pub fn window(window: impl Into<String>, x: f64, y: f64) -> Self {
        Self { space: Space::Window { window: window.into() }, x, y }
    }
    pub fn capture(capture: impl Into<String>, x: f64, y: f64) -> Self {
        Self { space: Space::Capture { capture: capture.into() }, x, y }
    }
}

/// What a screenshot was taken of, kept by the broker so capture-space points can be mapped.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureMeta {
    pub capture: String,
    pub display: String,
    pub pixel_width: u32,
    pub pixel_height: u32,
    /// The display's logical rectangle at capture time.
    pub logical: Rect,
    pub layout_generation: u64,
    pub taken_at_ms: u64,
    pub expires_at_ms: u64,
}

impl CaptureMeta {
    /// Map a pixel in this capture to desktop logical coordinates.
    pub fn to_desktop(&self, px: f64, py: f64) -> Option<(f64, f64)> {
        if px < 0.0 || py < 0.0 || px >= self.pixel_width as f64 || py >= self.pixel_height as f64 {
            return None;
        }
        let sx = self.logical.width / self.pixel_width as f64;
        let sy = self.logical.height / self.pixel_height as f64;
        Some((self.logical.x + px * sx, self.logical.y + py * sy))
    }
}
