mod ax;
mod display;
mod events;
mod image;
mod symbols;
mod types;
mod window;
mod workspace;

pub use ax::{AxElement, AxObserver, RunLoop, RunLoopSource, run_default_loop_slice};
pub use display::{capture_main_display, main_display_bounds};
pub use events::{
    current_mouse_location, post_keyboard_event, post_mouse_event, post_scroll_event,
};
pub use types::{
    AxErrorCode, ModifierFlags, MouseButton, MouseEventKind, PngImage, Point, Rect,
    RunningApplication, ScreenSpace, Size, WindowId,
};
pub use window::{capture_window, set_window_alpha};
pub use workspace::{frontmost_application_pid, is_process_trusted, running_applications};

#[cfg(test)]
mod tests;
