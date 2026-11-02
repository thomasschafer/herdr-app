//! Window background paint is separate from foreground opacity: backgrounds
//! can reveal the desktop while text and icons remain readable.

use gpui::*;

#[cfg(target_os = "macos")]
mod macos;

pub fn apply(window: &Window, opacity: f32, blur: u32, fullscreen: bool) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    return macos::apply(window, opacity, blur, fullscreen);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (window, fullscreen);
        anyhow::ensure!(
            opacity == 1.0 && blur == 0,
            "background transparency is currently supported only on macOS"
        );
        Ok(())
    }
}

pub fn with_opacity<R>(
    window: &mut Window,
    opacity: f32,
    paint: impl FnOnce(&mut Window) -> R,
) -> R {
    #[cfg(target_os = "macos")]
    return window.with_background_opacity(opacity, paint);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = opacity;
        paint(window)
    }
}

/// A cell painted by a terminal app replaces the background beneath it rather
/// than adding a second translucent layer. Selections and cursors use normal paint.
pub fn paint(window: &mut Window, quad: PaintQuad, cell: bool) {
    let settings = crate::settings::get();
    let opacity = if window.is_fullscreen() || (cell && !settings.background_opacity_cells) {
        1.0
    } else {
        settings.background_opacity
    };
    with_opacity(window, opacity, |window| window.paint_quad(quad));
}

pub fn wrap(child: impl IntoElement, opacity: f32) -> impl IntoElement {
    Background {
        child: child.into_any_element(),
        opacity,
    }
}

struct Background {
    child: AnyElement,
    opacity: f32,
}

impl IntoElement for Background {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Background {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        with_opacity(window, self.opacity, |window| self.child.paint(window, cx));
    }
}
