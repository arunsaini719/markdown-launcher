#![cfg(target_os = "macos")]

use objc2_app_kit::NSView;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Orders one AppKit window out without hiding the rest of the application.
pub fn hide_window(window: &impl HasWindowHandle) -> Result<(), String> {
    let handle = window
        .window_handle()
        .map_err(|error| format!("failed to access the launcher window: {error}"))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("the launcher does not expose an AppKit window".into());
    };

    hide_appkit_view_window(handle.ns_view)
}

#[allow(unsafe_code)]
fn hide_appkit_view_window(view: core::ptr::NonNull<core::ffi::c_void>) -> Result<(), String> {
    // SAFETY: raw-window-handle guarantees that `ns_view` points to a valid NSView
    // for at least the lifetime of the borrowed window handle above. This function
    // does not retain the pointer beyond the call.
    let view = unsafe { view.cast::<NSView>().as_ref() };
    let window = view
        .window()
        .ok_or_else(|| "the launcher view is not attached to an AppKit window".to_string())?;
    window.orderOut(None);
    Ok(())
}
