use euclid::{Point2D, Rect as EuclidRect, Size2D};
use objc2_application_services::AXError;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenSpace;

pub type Point = Point2D<f64, ScreenSpace>;
pub type Size = Size2D<f64, ScreenSpace>;
pub type Rect = EuclidRect<f64, ScreenSpace>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PngImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WindowId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningApplication {
    pub pid: u32,
    pub localized_name: Option<String>,
    pub activation_policy: isize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AxErrorCode(pub i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventKind {
    Move,
    Down,
    Up,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ModifierFlags {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl AxErrorCode {
    pub const SUCCESS: Self = Self(0);
    pub const FAILURE: Self = Self(-25200);

    pub(crate) fn from_ax_error(error: AXError) -> Self {
        Self(error.0)
    }

    pub fn is_success(self) -> bool {
        self == Self::SUCCESS
    }
}

impl fmt::Display for AxErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AXError({})", self.0)
    }
}

impl std::error::Error for AxErrorCode {}
