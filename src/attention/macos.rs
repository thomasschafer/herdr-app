use objc2::{class, msg_send, runtime::AnyObject};
use std::ffi::CString;

/// `NSApp.dockTile.badgeLabel`: drawn by the Dock, nil removes it. On the
/// main thread, as AppKit requires (the window calls it while publishing).
pub(super) fn set_badge(label: Option<&str>) {
    let text = label.map(|l| CString::new(l).expect("a badge label is digits, no NUL"));
    unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let tile: *mut AnyObject = msg_send![app, dockTile];
        if tile.is_null() {
            tracing::warn!("no Dock tile for the badge");
            return;
        }
        let label: *mut AnyObject = match &text {
            Some(text) => msg_send![class!(NSString), stringWithUTF8String: text.as_ptr()],
            None => std::ptr::null_mut(),
        };
        let _: () = msg_send![tile, setBadgeLabel: label];
    }
}

/// Run from an app bundle (`NSBundle.mainBundle.bundleIdentifier` is set), as
/// the installed Herdr is.
pub(super) fn bundled() -> bool {
    unsafe {
        let bundle: *mut AnyObject = msg_send![class!(NSBundle), mainBundle];
        if bundle.is_null() {
            return false;
        }
        let id: *mut AnyObject = msg_send![bundle, bundleIdentifier];
        !id.is_null()
    }
}
