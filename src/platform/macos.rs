//! macOS-only window behavior. Slint continues to own the visuals.
use std::{cell::Cell, ptr::NonNull};

use block2::RcBlock;
use objc2::{rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition, NSEvent, NSEventMask,
    NSEventType, NSView,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

use crate::AppWindow;

const RATIO: f64 = 420.0 / 640.0;
const MIN_WIDTH: f64 = 294.0;
const MAX_WIDTH: f64 = 504.0;
// Must match AppWindow's visual scale. The remaining 2% is a resize margin.
const BODY_FRACTION: f64 = 0.98;

pub struct ResizeMonitor {
    token: Retained<AnyObject>,
}

impl Drop for ResizeMonitor {
    fn drop(&mut self) {
        // SAFETY: token was returned by addLocalMonitorForEventsMatchingMask_handler.
        unsafe { NSEvent::removeMonitor(&self.token) };
    }
}

#[derive(Clone, Copy)]
struct Edges {
    left: bool,
    right: bool,
    top: bool,
    bottom: bool,
}

impl Edges {
    fn cursor(self) -> NSCursorFrameResizePosition {
        use NSCursorFrameResizePosition as Position;
        match (self.left, self.right, self.top, self.bottom) {
            (true, _, true, _) => Position::TopLeft,
            (_, true, true, _) => Position::TopRight,
            (true, _, _, true) => Position::BottomLeft,
            (_, true, _, true) => Position::BottomRight,
            (true, _, _, _) => Position::Left,
            (_, true, _, _) => Position::Right,
            (_, _, true, _) => Position::Top,
            _ => Position::Bottom,
        }
    }
}

// Hit testing uses Slint's reference coordinates (origin at the body's top-left).
// Keep these shapes in sync with DeviceShell: 420×640, radius 32, border 4,
// display (50,32,320,300), wheel center (210,471), radius 115.
enum ShellHit {
    Outside,
    Control,
    Move,
    Resize(Edges),
}

fn in_rounded_body(x: f64, y: f64, inset: f64) -> bool {
    if x < inset || y < inset || x > 420.0 - inset || y > 640.0 - inset {
        return false;
    }
    let cx = x.clamp(32.0, 388.0);
    let cy = y.clamp(32.0, 608.0);
    (x - cx).powi(2) + (y - cy).powi(2) <= (32.0 - inset).powi(2)
}

fn shell_hit(point: NSPoint, size: NSSize) -> ShellHit {
    let scale = (size.width / 420.0).min(size.height / 640.0) * BODY_FRACTION;
    if scale <= 0.0 {
        return ShellHit::Outside;
    }
    let x = (point.x - (size.width - 420.0 * scale) / 2.0) / scale;
    let y = (size.height - point.y - (size.height - 640.0 * scale) / 2.0) / scale;
    if !in_rounded_body(x, y, 0.0) {
        return ShellHit::Outside;
    }
    if !in_rounded_body(x, y, 4.0) {
        return ShellHit::Resize(Edges {
            left: x < 32.0,
            right: x > 388.0,
            top: y < 32.0,
            bottom: y > 608.0,
        });
    }
    if ((50.0..=370.0).contains(&x) && (32.0..=332.0).contains(&y))
        || (x - 210.0).powi(2) + (y - 471.0).powi(2) <= 115.0_f64.powi(2)
    {
        ShellHit::Control
    } else {
        ShellHit::Move
    }
}

#[derive(Clone, Copy)]
struct Drag {
    edges: Edges,
    start_mouse: NSPoint,
    start_frame: NSRect,
    max_width: f64,
}

// Freeze this cap at mouse-down. Never switch screens/anchors mid-gesture.
fn screen_width_limit(frame: NSRect, edges: Edges, visible: NSRect) -> f64 {
    let available_width = if edges.left {
        frame.origin.x + frame.size.width - visible.origin.x
    } else {
        visible.origin.x + visible.size.width - frame.origin.x
    };
    let available_height = if edges.bottom {
        frame.origin.y + frame.size.height - visible.origin.y
    } else {
        visible.origin.y + visible.size.height - frame.origin.y
    };
    // Do not snap an already partially off-screen window inward at mouse-down.
    available_width
        .min(available_height * RATIO)
        .max(frame.size.width)
        .clamp(MIN_WIDTH, MAX_WIDTH)
}

fn resized_frame(drag: Drag, mouse: NSPoint) -> NSRect {
    let dx = mouse.x - drag.start_mouse.x;
    let dy = mouse.y - drag.start_mouse.y;
    let old = drag.start_frame;
    let e = drag.edges;
    let horizontal = e.left || e.right;
    let vertical = e.top || e.bottom;

    let from_x = old.size.width + if e.left { -dx } else { dx };
    let from_y = (old.size.height + if e.bottom { -dy } else { dy }) * RATIO;
    // Orthogonal projection onto the fixed ratio diagonal. Unlike choosing the
    // dominant axis, this is continuous even when dx and dy have opposite signs.
    let width = if horizontal && vertical {
        (from_x + from_y / (RATIO * RATIO)) / (1.0 + 1.0 / (RATIO * RATIO))
    } else if horizontal {
        from_x
    } else {
        from_y
    }
    .clamp(MIN_WIDTH, drag.max_width);
    let height = width / RATIO;

    NSRect {
        origin: NSPoint {
            x: if e.left {
                old.origin.x + old.size.width - width
            } else {
                old.origin.x
            },
            y: if e.bottom {
                old.origin.y + old.size.height - height
            } else {
                old.origin.y
            },
        },
        size: NSSize { width, height },
    }
}

fn frame_difference(a: NSRect, b: NSRect) -> f64 {
    (a.origin.x - b.origin.x)
        .abs()
        .max((a.origin.y - b.origin.y).abs())
        .max((a.size.width - b.size.width).abs())
        .max((a.size.height - b.size.height).abs())
}

pub fn install(app: &AppWindow) -> Result<ResizeMonitor, String> {
    let handle = app.window().window_handle();
    let raw = handle
        .window_handle()
        .map_err(|err| err.to_string())?
        .as_raw();
    let RawWindowHandle::AppKit(appkit) = raw else {
        return Err("Slint did not provide an AppKit window handle".into());
    };

    // SAFETY: The raw handle's NSView is alive while the Slint window is alive.
    // RenderingSetup and the local event monitor both run on the AppKit UI thread.
    let view = unsafe { appkit.ns_view.as_ptr().cast::<NSView>().as_ref() }
        .ok_or("AppKit window handle has no NSView")?;
    let window = view
        .window()
        .ok_or("NSView is not attached to an NSWindow")?;
    window.setContentAspectRatio(NSSize {
        width: 420.0,
        height: 640.0,
    });
    let actual = window.contentAspectRatio();
    if (actual.width / actual.height - RATIO).abs() > 0.0001 {
        return Err(format!("AppKit rejected the aspect ratio: {actual:?}"));
    }

    window.setContentMinSize(NSSize {
        width: MIN_WIDTH,
        height: MIN_WIDTH / RATIO,
    });
    window.setContentMaxSize(NSSize {
        width: MAX_WIDTH,
        height: MAX_WIDTH / RATIO,
    });

    let drag = Cell::new(None::<Drag>);
    let last_request = Cell::new(None::<NSRect>);
    let reported_frame_adjustment = Cell::new(false);
    let debug_resize = std::env::var_os("IPOD_RESIZE_DEBUG").is_some();
    let was_edge = Cell::new(false);
    let resize_window = window.clone();
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> = RcBlock::new(
        move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit provides the event pointer for this callback's duration.
            let event = unsafe { event_ptr.as_ref() };
            let event_type = event.r#type();

            if event_type == NSEventType::LeftMouseDragged {
                if let Some(start) = drag.get() {
                    // Sample global AppKit points only when a drag event arrives.
                    // Converting queued window-local events through the *new* frame
                    // can feed frame movement back into the next resize calculation.
                    // This is event-driven, not a polling timer.
                    let mouse = NSEvent::mouseLocation();
                    let requested = resized_frame(start, mouse);
                    if last_request.get() != Some(requested) {
                        last_request.set(Some(requested));
                        resize_window.setFrame_display(requested, true);
                        let accepted = resize_window.frame();
                        if debug_resize
                            && !reported_frame_adjustment.get()
                            && frame_difference(requested, accepted) > 1.0
                        {
                            eprintln!(
                                "Resize frame adjusted by AppKit: requested={requested:?}, accepted={accepted:?}, limit={}",
                                start.max_width
                            );
                            reported_frame_adjustment.set(true);
                        }
                    }
                    return std::ptr::null_mut();
                }
            } else if event_type == NSEventType::LeftMouseUp && drag.take().is_some() {
                return std::ptr::null_mut();
            }

            if event.windowNumber() != resize_window.windowNumber() {
                return event_ptr.as_ptr();
            }
            let hit = shell_hit(event.locationInWindow(), resize_window.frame().size);
            if event_type == NSEventType::MouseMoved {
                if let ShellHit::Resize(edge) = hit {
                    NSCursor::frameResizeCursorFromPosition_inDirections(
                        edge.cursor(),
                        NSCursorFrameResizeDirections::All,
                    )
                    .set();
                    was_edge.set(true);
                    return std::ptr::null_mut(); // Don't let winit replace our cursor.
                }
                if was_edge.replace(false) {
                    NSCursor::arrowCursor().set();
                }
            } else if event_type == NSEventType::LeftMouseDown {
                if let ShellHit::Resize(edges) = hit {
                    let start_frame = resize_window.frame();
                    let max_width = resize_window.screen().map_or(MAX_WIDTH, |screen| {
                        screen_width_limit(start_frame, edges, screen.visibleFrame())
                    });
                    drag.set(Some(Drag {
                        edges,
                        start_mouse: NSEvent::mouseLocation(),
                        start_frame,
                        max_width,
                    }));
                    last_request.set(Some(start_frame));
                    reported_frame_adjustment.set(false);
                    if debug_resize {
                        eprintln!("Resize start: frame={start_frame:?}, width limit={max_width}");
                    }
                    return std::ptr::null_mut(); // Don't forward a resize press to Slint.
                }
                if matches!(hit, ShellHit::Move) {
                    // AppKit owns movement; no hand-written coordinate updates.
                    // This includes painted corners and the space outside the circular wheel.
                    resize_window.performWindowDragWithEvent(event);
                    return std::ptr::null_mut();
                }
            }
            event_ptr.as_ptr()
        },
    );

    let mask = NSEventMask::MouseMoved
        | NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp;
    // SAFETY: We return either the event AppKit supplied or null to consume it.
    // The block retains its window; ResizeMonitor removes the monitor on drop.
    let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) }
        .ok_or("AppKit could not install the resize event monitor")?;
    window.setAcceptsMouseMovedEvents(true);
    Ok(ResizeMonitor { token })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_hit_regions_follow_painted_shapes_at_all_sizes() {
        for width in [MIN_WIDTH, 357.0, MAX_WIDTH] {
            let size = NSSize {
                width,
                height: width / RATIO,
            };
            let scale = width / 420.0 * BODY_FRACTION;
            let hit = |x: f64, y: f64| {
                shell_hit(
                    NSPoint {
                        x: (size.width - 420.0 * scale) / 2.0 + x * scale,
                        y: size.height - (size.height - 640.0 * scale) / 2.0 - y * scale,
                    },
                    size,
                )
            };
            for (x, y) in [
                (210.0, 2.0),
                (2.0, 320.0),
                (418.0, 320.0),
                (210.0, 638.0),
                (10.0, 10.0),
                (410.0, 10.0),
                (10.0, 630.0),
                (410.0, 630.0),
            ] {
                assert!(matches!(hit(x, y), ShellHit::Resize(_)), "border {x},{y}");
            }
            // Interior rounded corners and the four corners of the wheel's bounding box.
            for (x, y) in [
                (20.0, 20.0),
                (400.0, 20.0),
                (20.0, 620.0),
                (400.0, 620.0),
                (100.0, 361.0),
                (320.0, 361.0),
                (100.0, 581.0),
                (320.0, 581.0),
            ] {
                assert!(matches!(hit(x, y), ShellHit::Move), "body {x},{y}");
            }
            for (x, y) in [
                (210.0, 100.0),
                (210.0, 471.0),
                (210.0, 365.0),
                (105.0, 471.0),
                (315.0, 471.0),
                (210.0, 576.0),
            ] {
                assert!(matches!(hit(x, y), ShellHit::Control), "control {x},{y}");
            }
            for (x, y) in [(-1.0, 320.0), (421.0, 320.0), (0.0, 0.0), (420.0, 640.0)] {
                assert!(matches!(hit(x, y), ShellHit::Outside));
            }
        }
    }

    #[test]
    fn corner_keeps_opposite_corner_and_ratio() {
        let old = NSRect {
            origin: NSPoint { x: 100.0, y: 100.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        let next = resized_frame(
            Drag {
                edges: Edges {
                    left: true,
                    right: false,
                    top: true,
                    bottom: false,
                },
                start_mouse: NSPoint { x: 100.0, y: 740.0 },
                start_frame: old,
                max_width: MAX_WIDTH,
            },
            NSPoint { x: 60.0, y: 780.0 },
        );
        assert!((next.size.width / next.size.height - RATIO).abs() < 0.0001);
        assert!((next.origin.x + next.size.width - 520.0).abs() < 0.0001); // right edge anchored
        assert_eq!(next.origin.y, 100.0); // bottom edge anchored
    }

    #[test]
    fn all_edges_keep_the_ratio_and_opposite_edge() {
        let frame = NSRect {
            origin: NSPoint { x: 100.0, y: 100.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        for (edges, mouse, expected_x, expected_y) in [
            (
                Edges {
                    left: true,
                    right: false,
                    top: false,
                    bottom: false,
                },
                NSPoint { x: -40.0, y: 0.0 },
                60.0,
                100.0,
            ),
            (
                Edges {
                    left: false,
                    right: true,
                    top: false,
                    bottom: false,
                },
                NSPoint { x: 40.0, y: 0.0 },
                100.0,
                100.0,
            ),
            (
                Edges {
                    left: false,
                    right: false,
                    top: true,
                    bottom: false,
                },
                NSPoint { x: 0.0, y: 40.0 },
                100.0,
                100.0,
            ),
            (
                Edges {
                    left: false,
                    right: false,
                    top: false,
                    bottom: true,
                },
                NSPoint { x: 0.0, y: -40.0 },
                100.0,
                60.0,
            ),
        ] {
            let next = resized_frame(
                Drag {
                    edges,
                    start_mouse: NSPoint { x: 0.0, y: 0.0 },
                    start_frame: frame,
                    max_width: MAX_WIDTH,
                },
                mouse,
            );
            assert!((next.size.width / next.size.height - RATIO).abs() < 0.0001);
            assert_eq!(next.origin.x, expected_x);
            // A bottom-edge resize holds the top edge; others hold the bottom.
            if edges.bottom {
                assert_eq!(next.origin.y + next.size.height, 740.0);
            } else {
                assert_eq!(next.origin.y, expected_y);
            }
            if edges.left {
                assert_eq!(next.origin.x + next.size.width, 520.0);
            }
        }
    }

    #[test]
    fn corner_projection_is_continuous_and_limits_are_anchored() {
        let frame = NSRect {
            origin: NSPoint { x: 100.0, y: 100.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        for left in [false, true] {
            for bottom in [false, true] {
                let drag = Drag {
                    edges: Edges {
                        left,
                        right: !left,
                        bottom,
                        top: !bottom,
                    },
                    start_mouse: NSPoint { x: 0.0, y: 0.0 },
                    start_frame: frame,
                    max_width: MAX_WIDTH,
                };
                // Opposing movements close to the old dominant-axis switch.
                let a = resized_frame(
                    drag,
                    NSPoint {
                        x: 60.0,
                        y: -60.0 / RATIO,
                    },
                );
                let b = resized_frame(
                    drag,
                    NSPoint {
                        x: 60.001,
                        y: -60.0 / RATIO,
                    },
                );
                assert!((a.size.width - b.size.width).abs() < 0.002);
                for amount in [-2000.0, 2000.0] {
                    let next = resized_frame(
                        drag,
                        NSPoint {
                            x: if left { -amount } else { amount },
                            y: if bottom { -amount } else { amount },
                        },
                    );
                    assert!((next.size.width / next.size.height - RATIO).abs() < 1e-9);
                    assert_eq!(
                        next.size.width,
                        if amount > 0.0 { MAX_WIDTH } else { MIN_WIDTH }
                    );
                    assert!(
                        (next.origin.x + if left { next.size.width } else { 0.0 }
                            - (frame.origin.x + if left { frame.size.width } else { 0.0 }))
                        .abs()
                            < 1e-9
                    );
                    assert!(
                        (next.origin.y + if bottom { next.size.height } else { 0.0 }
                            - (frame.origin.y + if bottom { frame.size.height } else { 0.0 }))
                        .abs()
                            < 1e-9
                    );
                }
            }
        }
    }

    #[test]
    fn screen_cap_does_not_snap_and_releases_when_pointer_returns() {
        let frame = NSRect {
            origin: NSPoint { x: 100.0, y: 100.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        let edges = Edges {
            left: false,
            right: true,
            top: false,
            bottom: false,
        };
        let visible = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: 1200.0,
                height: 850.0,
            },
        };
        let cap = screen_width_limit(frame, edges, visible);
        assert_eq!(cap, 750.0 * RATIO);
        let drag = Drag {
            edges,
            start_mouse: NSPoint { x: 0.0, y: 0.0 },
            start_frame: frame,
            max_width: cap,
        };
        let capped = resized_frame(drag, NSPoint { x: 1000.0, y: 0.0 });
        assert_eq!(capped.size.width, cap);
        let returned = resized_frame(drag, NSPoint { x: 10.0, y: 0.0 });
        assert_eq!(returned.size.width, 430.0);
        let small_screen = NSRect {
            size: NSSize {
                width: 500.0,
                height: 600.0,
            },
            ..visible
        };
        assert_eq!(
            screen_width_limit(frame, edges, small_screen),
            frame.size.width
        );
    }

    #[test]
    fn maximum_is_stable_and_reverse_motion_preserves_anchor() {
        let start_frame = NSRect {
            origin: NSPoint { x: 100.0, y: 100.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        let drag = Drag {
            edges: Edges {
                left: true,
                right: false,
                top: false,
                bottom: true,
            },
            start_mouse: NSPoint { x: 0.0, y: 0.0 },
            start_frame,
            max_width: MAX_WIDTH,
        };
        let at_max = resized_frame(
            drag,
            NSPoint {
                x: -1000.0,
                y: -1000.0,
            },
        );
        let beyond = resized_frame(
            drag,
            NSPoint {
                x: -2000.0,
                y: -2000.0,
            },
        );
        assert_eq!(at_max, beyond);
        assert_eq!(at_max.size.width, MAX_WIDTH);
        for delta in [-85.0, -84.0, -83.0, -40.0, 0.0] {
            let frame = resized_frame(
                drag,
                NSPoint {
                    x: delta,
                    y: delta / RATIO,
                },
            );
            assert!((frame.origin.x + frame.size.width - 520.0).abs() < 1e-9);
            assert!((frame.origin.y + frame.size.height - 740.0).abs() < 1e-9);
            assert!((frame.size.width / frame.size.height - RATIO).abs() < 1e-9);
        }
    }

    #[test]
    fn minimum_stays_readable() {
        let frame = NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: 420.0,
                height: 640.0,
            },
        };
        let next = resized_frame(
            Drag {
                edges: Edges {
                    left: false,
                    right: true,
                    top: false,
                    bottom: false,
                },
                start_mouse: NSPoint { x: 0.0, y: 0.0 },
                start_frame: frame,
                max_width: MAX_WIDTH,
            },
            NSPoint { x: -500.0, y: 0.0 },
        );
        assert_eq!(next.size.width, 294.0);
        assert_eq!(next.size.height, 448.0);
    }
}
