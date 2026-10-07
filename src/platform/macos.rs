//! macOS-only window behavior. Slint continues to own the visuals.
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::Rc,
    time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{AnyThread, MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSApplication, NSCursor, NSCursorFrameResizeDirections, NSCursorFrameResizePosition, NSEvent,
    NSEventMask, NSEventType, NSFloatingWindowLevel, NSScreen, NSTrackingArea,
    NSTrackingAreaOptions, NSView, NSWindow, NSWindowCollectionBehavior,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

use crate::{
    AppWindow,
    window_settings::{Geometry, MAX_WIDTH, MIN_WIDTH, RATIO, Store},
};
// Must match AppWindow's visual scale. The remaining 2% is a resize margin.
const BODY_FRACTION: f64 = 0.98;

pub struct ResizeMonitor {
    token: Retained<AnyObject>,
    window: Retained<NSWindow>,
    settings: Rc<RefCell<Store>>,
    timer: slint::Timer,
    hide_timer: Rc<slint::Timer>,
    tuck: Rc<RefCell<EdgeTuck>>,
    tracking_view: Retained<NSView>,
    tracking_area: Retained<NSTrackingArea>,
}

impl Drop for ResizeMonitor {
    fn drop(&mut self) {
        // SAFETY: token was returned by addLocalMonitorForEventsMatchingMask_handler.
        unsafe { NSEvent::removeMonitor(&self.token) };
        self.timer.stop();
        self.hide_timer.stop();
        self.tracking_view.removeTrackingArea(&self.tracking_area);
        self.settings.borrow_mut().record(
            self.tuck.borrow().shown_or(self.window.frame().into()),
            Instant::now(),
        );
        save(&self.settings);
    }
}

impl From<NSRect> for Geometry {
    fn from(frame: NSRect) -> Self {
        Self {
            x: frame.origin.x,
            y: frame.origin.y,
            width: frame.size.width,
            height: frame.size.height,
        }
    }
}
impl From<Geometry> for NSRect {
    fn from(frame: Geometry) -> Self {
        Self {
            origin: NSPoint {
                x: frame.x,
                y: frame.y,
            },
            size: NSSize {
                width: frame.width,
                height: frame.height,
            },
        }
    }
}
fn screens() -> Vec<Geometry> {
    NSScreen::screens(MainThreadMarker::new().expect("window settings run on the AppKit thread"))
        .iter()
        .map(|screen| screen.visibleFrame().into())
        .collect()
}
// Do not replace the backend's other Spaces/fullscreen/cycling flags. Restore
// its exact original behavior when unpinned, including any original join flag.
fn pinned_behavior(
    original: NSWindowCollectionBehavior,
    pinned: bool,
) -> NSWindowCollectionBehavior {
    if pinned {
        original | NSWindowCollectionBehavior::CanJoinAllSpaces
    } else {
        original
    }
}

fn save(settings: &RefCell<Store>) {
    if let Err(err) = settings.borrow_mut().flush() {
        eprintln!("Cannot save window preferences: {err}");
    }
}

/// Seed Slint's size before show; AppKit restores the global origin in the
/// existing RenderingSetup hook, before the backend reveals its first frame.
pub fn prepare(app: &AppWindow) -> Rc<RefCell<Store>> {
    let path = std::env::var_os("HOME").map(|home| {
        std::path::PathBuf::from(home)
            .join("Library/Application Support/io.github.haikeyidesu.ipod-player/window.json")
    });
    let mut settings = Store::load(path);
    let screens = screens();
    let initial = settings
        .preferences
        .geometry
        .unwrap_or_else(|| {
            let screen = screens.first().copied().unwrap_or(Geometry {
                x: 0.0,
                y: 0.0,
                width: 420.0,
                height: 640.0,
            });
            Geometry {
                x: screen.x + (screen.width - 420.0) / 2.0,
                y: screen.y + (screen.height - 640.0) / 2.0,
                width: 420.0,
                height: 640.0,
            }
        })
        .restored(&screens);
    settings.record(initial, Instant::now());
    app.set_window_pinned(settings.preferences.always_on_top);
    app.set_window_tuck_enabled(settings.preferences.edge_tuck_enabled);
    app.window().set_size(slint::LogicalSize::new(
        initial.width as f32,
        initial.height as f32,
    ));
    Rc::new(RefCell::new(settings))
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug)]
struct Dock {
    side: Side,
    shown: Geometry,
    screen: Geometry,
}
// Native window dragging can leave a margin between the pointer and the frame.
// A forgiving threshold makes edge placement deliberate without requiring pixel precision.
const DOCK_THRESHOLD: f64 = 96.0;
const TUCK_TAB: f64 = 36.0;

impl Dock {
    fn hidden(self) -> Geometry {
        Geometry {
            x: match self.side {
                Side::Left => self.screen.x + TUCK_TAB - self.shown.width,
                Side::Right => self.screen.x + self.screen.width - TUCK_TAB,
            },
            ..self.shown
        }
    }
}

// A screen shared with another display is not a safe tuck edge: the hidden
// body would merely move onto that display instead of disappearing.
#[cfg(test)]
fn dock_candidate(frame: Geometry, screen: Geometry, other_screens: &[Geometry]) -> Option<Dock> {
    dock_candidate_at(frame, screen, other_screens, None)
}
fn dock_candidate_at(
    frame: Geometry,
    screen: Geometry,
    other_screens: &[Geometry],
    pointer_x: Option<f64>,
) -> Option<Dock> {
    if frame.width > screen.width || frame.height > screen.height || screen.width <= TUCK_TAB {
        return None;
    }
    let edge = [
        (
            Side::Left,
            (frame.x - screen.x)
                .abs()
                .min(pointer_x.map_or(f64::INFINITY, |x| (x - screen.x).abs())),
        ),
        (
            Side::Right,
            (frame.x + frame.width - screen.x - screen.width)
                .abs()
                .min(pointer_x.map_or(f64::INFINITY, |x| (x - screen.x - screen.width).abs())),
        ),
    ]
    .into_iter()
    .min_by(|a, b| a.1.total_cmp(&b.1))?;
    if edge.1 > DOCK_THRESHOLD {
        return None;
    }
    let shown = Geometry {
        x: if edge.0 == Side::Left {
            screen.x
        } else {
            screen.x + screen.width - frame.width
        },
        y: frame
            .y
            .clamp(screen.y, screen.y + screen.height - frame.height),
        ..frame
    };
    let dock = Dock {
        side: edge.0,
        shown,
        screen,
    };
    let tucked = dock.hidden();
    let corridor = match edge.0 {
        Side::Left => (tucked.x, screen.x),
        Side::Right => (screen.x + screen.width, tucked.x + frame.width),
    };
    if other_screens.iter().any(|other| {
        let vertical = other.y < shown.y + shown.height && other.y + other.height > shown.y;
        vertical && other.x < corridor.1 && other.x + other.width > corridor.0
    }) {
        None
    } else {
        Some(dock)
    }
}

struct EdgeTuck {
    enabled: bool,
    dock: Option<Dock>,
    hidden: bool,
    revealed_at: Option<Instant>,
}
impl EdgeTuck {
    fn shown_or(&self, current: Geometry) -> Geometry {
        self.dock.map_or(current, |dock| dock.shown)
    }
    fn reveal(&mut self, window: &NSWindow) {
        if self.hidden {
            self.hidden = false;
            self.revealed_at = Some(Instant::now());
            if let Some(dock) = self.dock {
                window.setFrame_display_animate(dock.shown.into(), true, true);
            }
        }
    }
    fn hide(&mut self, window: &NSWindow) {
        if !self.hidden
            && let Some(dock) = self.dock
        {
            self.hidden = true;
            self.revealed_at = None;
            window.setFrame_display_animate(dock.hidden().into(), true, true);
        }
    }
}
fn point_in(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height
}

fn dock_for_window(window: &NSWindow) -> Option<Dock> {
    let active = window.screen()?;
    let visible = Geometry::from(active.visibleFrame());
    let others = NSScreen::screens(MainThreadMarker::new()?)
        .iter()
        .filter(|screen| screen.frame() != active.frame())
        .map(|screen| Geometry::from(screen.frame()))
        .collect::<Vec<_>>();
    dock_candidate_at(
        window.frame().into(),
        visible,
        &others,
        Some(NSEvent::mouseLocation().x),
    )
}
fn settle_dock(window: &NSWindow, tuck: &RefCell<EdgeTuck>) {
    let mut tuck = tuck.borrow_mut();
    if !tuck.enabled {
        return;
    }
    tuck.hidden = false;
    tuck.revealed_at = None;
    tuck.dock = dock_for_window(window);
    if let Some(dock) = tuck.dock {
        window.setFrame_display_animate(dock.shown.into(), true, true);
    }
}

fn frame_difference(a: NSRect, b: NSRect) -> f64 {
    (a.origin.x - b.origin.x)
        .abs()
        .max((a.origin.y - b.origin.y).abs())
        .max((a.size.width - b.size.width).abs())
        .max((a.size.height - b.size.height).abs())
}

pub fn install(app: &AppWindow, settings: Rc<RefCell<Store>>) -> Result<ResizeMonitor, String> {
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

    let tracking_view = window
        .contentView()
        .ok_or("AppKit window has no content view")?;
    // ActiveAlways delivers enter/exit for the visible tab even when this
    // LSUIElement app is not active. AppKit owns the hit area as the frame moves.
    let tracking_area = unsafe {
        NSTrackingArea::initWithRect_options_owner_userInfo(
            NSTrackingArea::alloc(),
            NSRect {
                origin: NSPoint { x: 0.0, y: 0.0 },
                size: NSSize {
                    width: 0.0,
                    height: 0.0,
                },
            },
            NSTrackingAreaOptions::MouseEnteredAndExited
                | NSTrackingAreaOptions::MouseMoved
                | NSTrackingAreaOptions::ActiveAlways
                | NSTrackingAreaOptions::InVisibleRect,
            Some(&tracking_view),
            None,
        )
    };
    tracking_view.addTrackingArea(&tracking_area);
    let tuck = Rc::new(RefCell::new(EdgeTuck {
        enabled: settings.borrow().preferences.edge_tuck_enabled,
        dock: None,
        hidden: false,
        revealed_at: None,
    }));
    let original_level = window.level();
    let original_behavior = window.collectionBehavior();
    if let Some(frame) = settings.borrow().preferences.geometry {
        window.setFrame_display(frame.restored(&screens()).into(), false);
    }
    if settings.borrow().preferences.always_on_top {
        window.setCollectionBehavior(pinned_behavior(original_behavior, true));
        window.setLevel(NSFloatingWindowLevel);
    }
    let pin_window = window.clone();
    let pin_settings = settings.clone();
    let pin_tuck = tuck.clone();
    let weak = app.as_weak();
    app.on_window_pin_requested(move || {
        let Some(app) = weak.upgrade() else {
            return;
        };
        let pinned = !pin_settings.borrow().preferences.always_on_top;
        // No makeKey/orderFront or frame changes: joining Spaces and floating
        // must not activate the app or disturb native dragging/resizing.
        pin_window.setCollectionBehavior(pinned_behavior(original_behavior, pinned));
        pin_window.setLevel(if pinned {
            NSFloatingWindowLevel
        } else {
            original_level
        });
        let mut settings = pin_settings.borrow_mut();
        settings.pin(pinned);
        settings.record(
            pin_tuck.borrow().shown_or(pin_window.frame().into()),
            Instant::now(),
        );
        drop(settings);
        app.set_window_pinned(pinned);
        save(&pin_settings);
        app.invoke_show_status(
            if pinned {
                "Always on Top On"
            } else {
                "Always on Top Off"
            }
            .into(),
        );
    });
    let reset_window = window.clone();
    let reset_settings = settings.clone();
    let reset_tuck = tuck.clone();
    app.on_window_reset_requested(move || {
        let frame = reset_tuck
            .borrow()
            .shown_or(reset_window.frame().into())
            .canonical()
            .restored(&screens());
        reset_tuck.borrow_mut().dock = None;
        reset_tuck.borrow_mut().hidden = false;
        reset_tuck.borrow_mut().revealed_at = None;
        reset_window.setFrame_display(frame.into(), true);
        reset_settings
            .borrow_mut()
            .record(reset_window.frame().into(), Instant::now());
        save(&reset_settings);
    });
    let mode_tuck = tuck.clone();
    let mode_settings = settings.clone();
    let mode_window = window.clone();
    let weak = app.as_weak();
    app.on_window_tuck_requested(move || {
        let enabled = !mode_tuck.borrow().enabled;
        let mut state = mode_tuck.borrow_mut();
        if !enabled {
            if let Some(dock) = state.dock {
                mode_window.setFrame_display(dock.shown.into(), true);
            }
            state.hidden = false;
            state.revealed_at = None;
            state.dock = None;
        }
        state.enabled = enabled;
        drop(state);
        mode_settings.borrow_mut().edge_tuck(enabled);
        save(&mode_settings);
        if let Some(app) = weak.upgrade() {
            app.set_window_tuck_enabled(enabled);
            app.invoke_show_status(
                if enabled {
                    "Edge Tuck On — drag to edge"
                } else {
                    "Edge Tuck Off"
                }
                .into(),
            );
        }
    });

    let hide_timer = Rc::new(slint::Timer::default());
    let pending_hide = Rc::new(Cell::new(false));
    let interacting = Rc::new(Cell::new(false));
    let hide_window = window.clone();
    let hide_tuck = tuck.clone();
    let hide_pending = pending_hide.clone();
    let hide_interacting = interacting.clone();
    let hide_weak = app.as_weak();
    let hide_clock = hide_timer.clone();
    let schedule_hide: Rc<dyn Fn()> = Rc::new(move || {
        if hide_pending.replace(true) {
            return;
        }
        let pending = hide_pending.clone();
        let state = hide_tuck.clone();
        let window = hide_window.clone();
        let interaction = hide_interacting.clone();
        let weak = hide_weak.clone();
        hide_clock.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(450),
            move || {
                pending.set(false);
                if window.isKeyWindow()
                    || interaction.get()
                    || point_in(window.frame(), NSEvent::mouseLocation())
                {
                    return;
                }
                if weak
                    .upgrade()
                    .is_some_and(|app| app.get_playlist_entry_open())
                {
                    return;
                }
                state.borrow_mut().hide(&window);
            },
        );
    });

    let moving = Rc::new(Cell::new(false));
    let finish_moving = moving.clone();
    let finish_window = window.clone();
    let finish_tuck = tuck.clone();
    let finish_interacting = interacting.clone();
    let finish_move: Rc<dyn Fn()> = Rc::new(move || {
        if !finish_moving.replace(false) {
            return;
        }
        let window = finish_window.clone();
        let state = finish_tuck.clone();
        let interacting = finish_interacting.clone();
        // performWindowDragWithEvent may return before AppKit applies the final
        // mouse-up position. Settle on the next event-loop turn, not at mouse-down.
        slint::Timer::single_shot(Duration::from_millis(60), move || {
            settle_dock(&window, &state);
            interacting.set(false);
        });
    });
    let timer = slint::Timer::default();
    let observed_moving = moving.clone();
    let observed_finish_move = finish_move.clone();
    let observed_window = window.clone();
    let observed_settings = settings.clone();
    let observed_tuck = tuck.clone();
    let observed_pending = pending_hide.clone();
    let observed_schedule = schedule_hide.clone();
    let observed_weak = app.as_weak();
    let mut previous_screens = screens();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(250),
        move || {
            // A native AppKit drag can swallow its mouse-up before the local
            // monitor sees it. Only observe button state while a drag is live.
            if observed_moving.get() && NSEvent::pressedMouseButtons() & 1 == 0 {
                observed_finish_move();
            }
            let current_screens = screens();
            if previous_screens != current_screens {
                let frame = observed_tuck
                    .borrow()
                    .shown_or(observed_window.frame().into())
                    .restored(&current_screens);
                observed_tuck.borrow_mut().dock = None;
                observed_tuck.borrow_mut().hidden = false;
                observed_tuck.borrow_mut().revealed_at = None;
                observed_window.setFrame_display(frame.into(), true);
                previous_screens = current_screens;
            }
            if !observed_window.isKeyWindow()
                && !observed_pending.get()
                && observed_tuck.borrow().dock.is_some()
                && !observed_tuck.borrow().hidden
                && !point_in(observed_window.frame(), NSEvent::mouseLocation())
                && observed_weak
                    .upgrade()
                    .is_some_and(|app| !app.get_playlist_entry_open())
            {
                observed_schedule();
            }
            let now = Instant::now();
            let mut settings = observed_settings.borrow_mut();
            settings.record(
                observed_tuck
                    .borrow()
                    .shown_or(observed_window.frame().into()),
                now,
            );
            let due = settings.due(now);
            drop(settings);
            if due {
                save(&observed_settings);
            }
        },
    );

    let drag = Cell::new(None::<Drag>);
    let last_request = Cell::new(None::<NSRect>);
    let reported_frame_adjustment = Cell::new(false);
    let debug_resize = std::env::var_os("IPOD_RESIZE_DEBUG").is_some();
    let was_edge = Cell::new(false);
    let resize_moving = moving.clone();
    let resize_finish_move = finish_move.clone();
    let resize_window = window.clone();
    let resize_tuck = tuck.clone();
    let resize_interacting = interacting.clone();
    let resize_schedule = schedule_hide.clone();
    let resize_pending = pending_hide.clone();
    let resize_hide_timer = hide_timer.clone();
    let focus_weak = app.as_weak();
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
            } else if event_type == NSEventType::LeftMouseUp {
                if drag.take().is_some() {
                    resize_interacting.set(false);
                    settle_dock(&resize_window, &resize_tuck);
                    return std::ptr::null_mut();
                }
                resize_finish_move();
            }

            if event.windowNumber() != resize_window.windowNumber() {
                return event_ptr.as_ptr();
            }
            if event_type == NSEventType::MouseEntered
                || (event_type == NSEventType::MouseMoved && resize_tuck.borrow().hidden)
            {
                resize_hide_timer.stop();
                resize_pending.set(false);
                resize_tuck.borrow_mut().reveal(&resize_window);
            } else if event_type == NSEventType::MouseExited {
                if was_edge.replace(false) {
                    NSCursor::arrowCursor().set();
                }
                resize_schedule();
            }
            let was_hidden_click = event_type == NSEventType::LeftMouseDown
                && (resize_tuck.borrow().hidden
                    || resize_tuck
                        .borrow()
                        .revealed_at
                        .is_some_and(|at| at.elapsed() < Duration::from_millis(300)));
            let hit = shell_hit(event.locationInWindow(), resize_window.frame().size);
            if event_type == NSEventType::LeftMouseDown
                && (was_hidden_click || !matches!(hit, ShellHit::Outside))
            {
                // An LSUIElement window can stay visible after another app becomes
                // active. Only an explicit click may reclaim the keyboard; never
                // activate it merely because the user switched workspaces.
                let application = NSApplication::sharedApplication(
                    MainThreadMarker::new().expect("AppKit events run on the main thread"),
                );
                if !application.isActive() || !resize_window.isKeyWindow() {
                    application.activate();
                    resize_window.makeKeyWindow();
                }
                // Slint's FocusScope can have lost focus even if AppKit still
                // considers this window key after a workspace transition.
                if let Some(app) = focus_weak.upgrade() {
                    app.invoke_refocus_navigation();
                }
                if was_hidden_click {
                    resize_hide_timer.stop();
                    resize_pending.set(false);
                    resize_tuck.borrow_mut().reveal(&resize_window);
                    if let Some(dock) = resize_tuck.borrow().dock {
                        resize_window.setFrame_display(dock.shown.into(), true);
                    }
                    resize_tuck.borrow_mut().revealed_at = None;
                    return std::ptr::null_mut();
                }
            }
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
                    resize_interacting.set(true);
                    resize_hide_timer.stop();
                    resize_pending.set(false);
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
                    resize_interacting.set(true);
                    resize_hide_timer.stop();
                    resize_pending.set(false);
                    resize_moving.set(true);
                    resize_window.performWindowDragWithEvent(event);
                    if NSEvent::pressedMouseButtons() & 1 == 0 {
                        resize_finish_move();
                    }
                    return std::ptr::null_mut();
                }
            }
            event_ptr.as_ptr()
        },
    );

    let mask = NSEventMask::MouseMoved
        | NSEventMask::MouseEntered
        | NSEventMask::MouseExited
        | NSEventMask::LeftMouseDown
        | NSEventMask::LeftMouseDragged
        | NSEventMask::LeftMouseUp;
    // SAFETY: We return either the event AppKit supplied or null to consume it.
    // The block retains its window; ResizeMonitor removes the monitor on drop.
    let token = unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &block) }
        .ok_or("AppKit could not install the resize event monitor")?;
    window.setAcceptsMouseMovedEvents(true);
    Ok(ResizeMonitor {
        token,
        window,
        settings,
        timer,
        hide_timer,
        tuck,
        tracking_view,
        tracking_area,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_joins_spaces_and_restores_all_original_flags() {
        use NSWindowCollectionBehavior as B;
        for original in [
            B::Default,
            B::Managed | B::IgnoresCycle | B::FullScreenNone,
            B::Stationary | B::CanJoinAllSpaces,
        ] {
            let pinned = pinned_behavior(original, true);
            assert!(pinned.contains(B::CanJoinAllSpaces));
            assert!(pinned.contains(original));
            assert_eq!(pinned_behavior(original, false), original);
        }
    }

    #[test]
    fn edge_tuck_preserves_shown_geometry_and_avoids_adjacent_displays() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let frame = Geometry {
            x: 18.0,
            y: 110.0,
            width: 420.0,
            height: 640.0,
        };
        let left = dock_candidate(frame, screen, &[]).unwrap();
        assert_eq!(left.side, Side::Left);
        assert_eq!(left.shown.x, 0.0);
        assert_eq!(left.shown.y, frame.y);
        assert_eq!(left.hidden().x + left.hidden().width, TUCK_TAB);
        assert_eq!(left.hidden().y, frame.y);
        assert_eq!(
            (left.hidden().width, left.hidden().height),
            (frame.width, frame.height)
        );
        let right_frame = Geometry {
            x: 1440.0 - 420.0 - 12.0,
            ..frame
        };
        let right = dock_candidate(right_frame, screen, &[]).unwrap();
        assert_eq!(right.side, Side::Right);
        assert_eq!(right.shown.x, 1020.0);
        assert_eq!(right.hidden().x, 1440.0 - TUCK_TAB);
        assert_eq!(
            EdgeTuck {
                enabled: true,
                dock: Some(right),
                hidden: true,
                revealed_at: None
            }
            .shown_or(right.hidden()),
            right.shown
        );
        assert!(
            dock_candidate(
                Geometry {
                    x: DOCK_THRESHOLD + 1.0,
                    ..frame
                },
                screen,
                &[]
            )
            .is_none()
        );
        let inboard = Geometry { x: 140.0, ..frame };
        assert_eq!(
            dock_candidate_at(inboard, screen, &[], Some(0.0))
                .unwrap()
                .side,
            Side::Left
        );
        assert!(dock_candidate_at(inboard, screen, &[], Some(120.0)).is_none());
        let neighbour = Geometry {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert!(dock_candidate(frame, screen, &[neighbour]).is_none());
        assert!(dock_candidate(right_frame, screen, &[neighbour]).is_some());
        let right_neighbour = Geometry {
            x: 1440.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert!(dock_candidate(right_frame, screen, &[right_neighbour]).is_none());
        assert!(
            dock_candidate(
                frame,
                screen,
                &[Geometry {
                    y: 901.0,
                    ..neighbour
                }]
            )
            .is_some()
        );
        assert!(
            dock_candidate(
                Geometry {
                    width: 630.0,
                    height: 960.0,
                    ..frame
                },
                screen,
                &[]
            )
            .is_none()
        );
    }

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
        assert_eq!(next.size.width, 315.0);
        assert_eq!(next.size.height, 480.0);
    }
}
