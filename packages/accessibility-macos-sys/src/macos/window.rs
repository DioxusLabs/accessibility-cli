use super::image::encode_cg_image_as_png;
use super::symbols::{
    cgs_main_connection_id, cgs_set_window_alpha, sls_main_connection_id, sls_set_window_alpha,
};
use super::{PngImage, WindowId};
use anyhow::Result;
use objc2_core_foundation::CGRect;
use objc2_core_graphics::{CGWindowImageOption, CGWindowListOption};

pub fn capture_window(window_id: WindowId) -> Result<Option<PngImage>> {
    #[allow(deprecated)]
    let image = objc2_core_graphics::CGWindowListCreateImage(
        CGRect::ZERO,
        CGWindowListOption::OptionIncludingWindow,
        window_id.0,
        CGWindowImageOption::BoundsIgnoreFraming | CGWindowImageOption::BestResolution,
    );

    image.as_deref().map(encode_cg_image_as_png).transpose()
}

pub fn set_window_alpha(window_id: WindowId, alpha: f32) -> bool {
    if window_id.0 == 0 {
        return false;
    }

    let alpha = alpha.clamp(0.0, 1.0);

    unsafe {
        if let (Some(connection_id), Some(set_window_alpha)) =
            (cgs_main_connection_id(), cgs_set_window_alpha())
            && set_window_alpha(connection_id(), window_id.0, alpha) == 0
        {
            return true;
        }

        if let (Some(connection_id), Some(set_window_alpha)) =
            (sls_main_connection_id(), sls_set_window_alpha())
        {
            return set_window_alpha(connection_id(), window_id.0, alpha) == 0;
        }
    }

    false
}
