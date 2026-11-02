use anyhow::{Context, Result, ensure};
use gpui::{Window, WindowBackgroundAppearance};
use objc2::{msg_send, runtime::AnyObject};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

pub(super) fn apply(window: &Window, opacity: f32, blur: u32, fullscreen: bool) -> Result<()> {
    let transparent = opacity < 1.0 && !fullscreen;
    window.set_background_appearance(if transparent {
        WindowBackgroundAppearance::Transparent
    } else {
        WindowBackgroundAppearance::Opaque
    });
    set_blur(window, if transparent { blur } else { 0 })
}

fn set_blur(window: &Window, radius: u32) -> Result<()> {
    type Connection = unsafe extern "C" fn() -> *mut c_void;
    type SetBlur = unsafe extern "C" fn(*mut c_void, usize, i32) -> i32;
    // Ghostty uses these WindowServer APIs for a numeric blur radius. Resolve
    // them dynamically so an OS removing the private API cannot prevent launch.
    unsafe {
        let connection = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"CGSDefaultConnectionForThread".as_ptr(),
        );
        let set_blur = libc::dlsym(
            libc::RTLD_DEFAULT,
            c"CGSSetWindowBackgroundBlurRadius".as_ptr(),
        );
        if connection.is_null() || set_blur.is_null() {
            ensure!(
                radius == 0,
                "macOS does not provide the numeric background blur API"
            );
            return Ok(());
        }
        let handle = match HasWindowHandle::window_handle(window) {
            Ok(handle) => handle,
            Err(_) if radius == 0 => return Ok(()),
            Err(error) => {
                anyhow::bail!("cannot get the native window for background blur: {error}")
            }
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            anyhow::bail!("background blur requires an AppKit window");
        };
        let view = handle.ns_view.as_ptr() as *mut AnyObject;
        let native: *mut AnyObject = msg_send![view, window];
        ensure!(!native.is_null(), "background blur view has no window");
        let number: isize = msg_send![native, windowNumber];
        ensure!(
            number > 0,
            "background blur window has no WindowServer number"
        );
        let connection: Connection = std::mem::transmute(connection);
        let set_blur: SetBlur = std::mem::transmute(set_blur);
        let radius =
            i32::try_from(radius).context("background blur exceeds the native radius range")?;
        let status = set_blur(connection(), number as usize, radius);
        ensure!(
            status == 0,
            "macOS rejected background blur (WindowServer error {status})"
        );
    }
    Ok(())
}
