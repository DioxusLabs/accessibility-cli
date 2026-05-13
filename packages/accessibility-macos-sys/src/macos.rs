mod ax;
mod display;
mod events;
mod image;
mod symbols;
mod types;
mod window;
mod workspace;

pub use ax::{
    AX_SEARCH_KEY_BUTTON, AX_SEARCH_KEY_CHECKBOX, AX_SEARCH_KEY_CONTROL, AX_SEARCH_KEY_GRAPHIC,
    AX_SEARCH_KEY_HEADING, AX_SEARCH_KEY_LINK, AX_SEARCH_KEY_LIST, AX_SEARCH_KEY_RADIO_GROUP,
    AX_SEARCH_KEY_STATIC_TEXT, AX_SEARCH_KEY_TABLE, AX_SEARCH_KEY_TEXT_FIELD, AxElement,
    AxObserver, AxSearchDirection, AxSearchPredicate, RunLoop, RunLoopSource,
    run_default_loop_slice,
};
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
