//! Real GPU readback, not a mocked image. Linux CI uses Vulkan software rendering.
use gpui::{prelude::*, *};

#[cfg(target_os = "macos")]
#[path = "../src/background/macos.rs"]
mod native_background;
#[path = "../src/sprites.rs"]
#[allow(unused_imports)]
mod sprites;

struct Scene;
impl Render for Scene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x123456))
            .child(
                div()
                    .absolute()
                    .left(px(4.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0xff0000)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(24.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x00ff00)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(44.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x0000ff)),
            )
            .child(
                div()
                    .absolute()
                    .left(px(64.))
                    .top(px(4.))
                    .size(px(16.))
                    .bg(rgb(0x808080)),
            )
            // Later paint must win over the red square.
            .child(
                div()
                    .absolute()
                    .left(px(8.))
                    .top(px(8.))
                    .size(px(4.))
                    .bg(rgb(0xf08020)),
            )
            .child(
                canvas(
                    |_, _, _| {},
                    |_, _, window, _| {
                        for (x, width) in [(110., 9.), (119., 9.5)] {
                            for row in 0..2 {
                                sprites::paint(
                                    "⡇",
                                    Bounds::new(
                                        point(px(x), px(4. + row as f32 * 24.)),
                                        size(px(width), px(24.)),
                                    ),
                                    rgb(0xffffff).into(),
                                    window,
                                );
                            }
                        }
                    },
                )
                .size_full(),
            )
    }
}

fn check(window: &mut Window, cx: &mut App) {
    assert!(sprites::is_sprite("⡇"));
    let _ = window.draw(cx);
    let image = window.render_to_image().expect("real GPU screenshot");
    let scale = window.scale_factor();
    let viewport = window.viewport_size();
    assert_eq!(
        image.dimensions(),
        (
            (f32::from(viewport.width) * scale).round() as u32,
            (f32::from(viewport.height) * scale).round() as u32,
        )
    );
    for (x, y, expected) in [
        (2., 2., [0x12, 0x34, 0x56, 255]),
        (6., 6., [255, 0, 0, 255]),
        (26., 6., [0, 255, 0, 255]),
        (46., 6., [0, 0, 255, 255]),
        (66., 6., [128, 128, 128, 255]),
        (10., 10., [240, 128, 32, 255]),
        (100., 40., [0x12, 0x34, 0x56, 255]),
    ] {
        assert_eq!(
            image.get_pixel((x * scale) as u32, (y * scale) as u32).0,
            expected,
            "pixel ({x}, {y}) at scale {scale}"
        );
    }
    // The four dots in each Braille row should keep the same spacing across rows.
    for x in [112., 121.5] {
        for dot in 0..8 {
            let x = (x * scale) as u32;
            let y = ((7. + dot as f32 * 6.) * scale) as u32;
            assert_eq!(image.get_pixel(x, y).0, [255, 255, 255, 255], "dot {dot}");
            if dot < 7 {
                let gap = ((10. + dot as f32 * 6.) * scale) as u32;
                assert_eq!(
                    image.get_pixel(x, gap).0,
                    [0x12, 0x34, 0x56, 255],
                    "gap after dot {dot}"
                );
            }
        }
    }
    // Capture must not consume the frame or change the next result.
    assert_eq!(image, window.render_to_image().unwrap());
    println!(
        "shot pixel test passed: {}x{}, scale {scale}",
        image.width(),
        image.height()
    );
}

#[cfg(target_os = "macos")]
struct TransparencyScene {
    opacity: f32,
}

#[cfg(target_os = "macos")]
impl Render for TransparencyScene {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let opacity = self.opacity;
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                let rect = |x, y, width, height| {
                    Bounds::new(bounds.origin + point(px(x), px(y)), size(px(width), px(height)))
                };
                window.with_background_opacity(opacity, |window| {
                    window.paint_quad(fill(bounds, rgb(0xff0000)));
                    window.with_background_opacity(opacity, |window| {
                        window.paint_quad(fill(rect(4., 4., 40., 40.), rgb(0x00ff00)));
                        window.paint_quad(fill(rect(8., 8., 20., 20.), rgb(0x0000ff)));
                    });
                    window.paint_quad(
                        fill(rect(50., 4., 40., 40.), rgb(0x00ff00)).corner_radii(px(10.)),
                    );
                    window.with_background_opacity(1., |window| {
                        window.paint_quad(fill(rect(96., 4., 16., 16.), rgb(0xffff00)));
                    });
                    window.paint_quad(fill(rect(96., 24., 16., 16.), rgb(0x0000ff)));
                });
                window.paint_quad(fill(rect(116., 4., 10., 10.), rgb(0xffffff)));
            },
        )
        .size_full()
    }
}

#[cfg(target_os = "macos")]
fn check_transparency(root: AnyView, window: &mut Window, cx: &mut App) {
    let root = root.downcast::<TransparencyScene>().unwrap();
    // Reuse one window to catch stale render state after changing preferences.
    for opacity in [0.8_f32, 0., 1., 0.8] {
        root.update(cx, |scene, cx| {
            scene.opacity = opacity;
            cx.notify();
        });
        window.set_background_appearance(if opacity == 1. {
            WindowBackgroundAppearance::Opaque
        } else {
            WindowBackgroundAppearance::Transparent
        });
        window.refresh();
        let _ = window.draw(cx);
        let image = window.render_to_image().expect("transparent GPU screenshot");
        let scale = window.scale_factor();
        let alpha = (opacity * 255.).round() as u8;
        for (x, y, expected) in [
            (2., 2., [alpha, 0, 0, alpha]),
            (6., 6., [0, alpha, 0, alpha]),
            (10., 10., [0, 0, alpha, alpha]),
            (50., 4., [alpha, 0, 0, alpha]),
            (70., 24., [0, alpha, 0, alpha]),
            (98., 6., [255, 255, 0, 255]),
            (98., 26., [0, 0, alpha, alpha]),
            (118., 6., [255, 255, 255, 255]),
        ] {
            let actual = image.get_pixel((x * scale) as u32, (y * scale) as u32).0;
            assert!(
                actual.iter().zip(expected).all(|(got, want)| got.abs_diff(want) <= 1),
                "opacity {opacity}, pixel ({x}, {y}) at scale {scale}: {actual:?}, expected {expected:?}"
            );
        }
        if opacity > 0. {
            let mut antialiased = 0;
            for y in (4. * scale) as u32..(15. * scale) as u32 {
                for x in (50. * scale) as u32..(61. * scale) as u32 {
                    let [red, green, blue, actual_alpha] = image.get_pixel(x, y).0;
                    if red > 0 && green > 0 {
                        antialiased += 1;
                        assert_eq!(blue, 0);
                        assert!(actual_alpha.abs_diff(alpha) <= 1, "rounded edge alpha at ({x}, {y}): {actual_alpha}, expected {alpha}");
                        assert!((red as u16 + green as u16).abs_diff(alpha as u16) <= 2,
                            "rounded edge must blend premultiplied colors at ({x}, {y})");
                    }
                }
            }
            assert!(antialiased > 0, "rounded corner must include fractional coverage at scale {scale}");
        }
        assert_eq!(image, window.render_to_image().unwrap());
    }
    println!("transparent shot pixel test passed");
}

#[cfg(target_os = "macos")]
fn check_native_transparency(root: AnyView, window: &mut Window, cx: &mut App) {
    use objc2::{msg_send, runtime::AnyObject};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    for (opacity, blur, fullscreen, expected_opaque) in [
        (0.8, 80, false, false),
        (0.8, 0, false, false),
        (0.8, 80, true, true),
        (1., 80, false, true),
        (0.8, 80, false, false),
    ] {
        native_background::apply(window, opacity, blur, fullscreen).expect("apply native transparency and numeric blur");
        let handle = HasWindowHandle::window_handle(window).unwrap();
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            panic!("macOS test requires an AppKit window");
        };
        let actual_opaque: bool = unsafe {
            let view = handle.ns_view.as_ptr() as *mut AnyObject;
            let native: *mut AnyObject = msg_send![view, window];
            msg_send![native, isOpaque]
        };
        assert_eq!(actual_opaque, expected_opaque,
            "native opacity {opacity}, blur {blur}, fullscreen {fullscreen}");
    }
    check_transparency(root, window, cx);
    native_background::apply(window, 1., 0, false).expect("reset native transparency and blur");
}

#[cfg(not(windows))]
fn headless() {
    let mut cx = HeadlessAppContext::with_platform(
        gpui_platform::current_platform(true).text_system(),
        std::sync::Arc::new(()),
        gpui_platform::current_headless_renderer,
    );
    // TestPlatform uses 2x HiDPI; these physical widths force row padding.
    for width in [129., 131.] {
        let window = cx
            .open_window(size(px(width), px(54.)), |_, cx| cx.new(|_| Scene))
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| check(window, cx))
            .unwrap();
    }
    #[cfg(target_os = "macos")]
    {
        let window = cx
            .open_window(size(px(131.), px(54.)), |_, cx| {
                cx.new(|_| TransparencyScene { opacity: 0.8 })
            })
            .unwrap();
        cx.update_window(window.into(), check_transparency).unwrap();
    }
}

fn native() {
    gpui_platform::application().run(|cx| {
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(259.), px(107.)),
                    ))),
                    show: false,
                    focus: false,
                    titlebar: None,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| Scene),
            )
            .unwrap();
        #[cfg(target_os = "macos")]
        let transparent_window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                        point(px(0.), px(0.)),
                        size(px(259.), px(107.)),
                    ))),
                    window_background: WindowBackgroundAppearance::Transparent,
                    show: false,
                    focus: false,
                    titlebar: None,
                    ..Default::default()
                },
                |_, cx| cx.new(|_| TransparencyScene { opacity: 0.8 }),
            )
            .unwrap();
        cx.spawn(async move |cx| {
            cx.update_window(window.into(), |_, window, cx| check(window, cx))
                .unwrap();
            #[cfg(target_os = "macos")]
            cx.update_window(transparent_window.into(), check_native_transparency).unwrap();
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

fn main() {
    #[cfg(not(windows))]
    if std::env::var_os("HERDR_SHOT_NATIVE_TEST").is_none() {
        return headless();
    }
    native();
}
