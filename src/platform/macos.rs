//! macOS-only window behavior. Slint continues to own the visuals.
use std::{
    cell::{Cell, RefCell},
    ptr::NonNull,
    rc::Rc,
    time::{Duration, Instant},
};

use block2::RcBlock;
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSApplication, NSBox, NSBoxType, NSColor, NSCursor, NSCursorFrameResizeDirections,
    NSCursorFrameResizePosition, NSEvent, NSEventMask, NSEventType, NSFloatingWindowLevel,
    NSScreen, NSTitlePosition, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowCollectionBehavior,
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
    reveal_timer: Rc<slint::Timer>,
    tuck: Rc<RefCell<EdgeTuck>>,
    tracking_view: Retained<NSView>,
    tracking_area: Retained<NSTrackingArea>,
    indicator: Retained<NSBox>,
}

impl Drop for ResizeMonitor {
    fn drop(&mut self) {
        // SAFETY: token was returned by addLocalMonitorForEventsMatchingMask_handler.
        unsafe { NSEvent::removeMonitor(&self.token) };
        self.timer.stop();
        self.hide_timer.stop();
        self.reveal_timer.stop();
        self.tracking_view.removeTrackingArea(&self.tracking_area);
        self.indicator.removeFromSuperview();
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

#[derive(Clone, Copy)]
struct MoveDrag {
    start_mouse: NSPoint,
    start_frame: Geometry,
}
impl MoveDrag {
    fn intended(self, end: NSPoint) -> Geometry {
        Geometry {
            x: self.start_frame.x + end.x - self.start_mouse.x,
            y: self.start_frame.y + end.y - self.start_mouse.y,
            ..self.start_frame
        }
    }
}

// Only a *shown dock* uses constrained manual movement. Free windows keep
// AppKit's native performWindowDragWithEvent path unchanged.
#[derive(Clone, Copy)]
struct DockedDrag {
    gesture: MoveDrag,
    prior: Dock,
    current: Dock,
    detached: Option<MoveDrag>,
    handle: bool,
    moved: bool,
}
impl DockedDrag {
    fn new(mouse: NSPoint, dock: Dock, handle: bool) -> Self {
        Self {
            gesture: MoveDrag {
                start_mouse: mouse,
                start_frame: dock.shown,
            },
            prior: dock,
            current: dock,
            detached: None,
            handle,
            moved: false,
        }
    }
    fn advance(mut self, mouse: NSPoint, others: &[Geometry]) -> (Self, Geometry, Option<Dock>) {
        self.moved |= (mouse.x - self.gesture.start_mouse.x)
            .hypot(mouse.y - self.gesture.start_mouse.y)
            >= 4.0;
        if !self.moved {
            return (self, self.current.shown, Some(self.current));
        }
        if let Some(free) = self.detached {
            return (self, free.intended(mouse), None);
        }
        if let Some(dock) = sticky_dock(self.prior, self.gesture.intended(mouse), others) {
            self.current = dock;
            return (self, dock.shown, Some(dock));
        }
        // Rebase at the detach point: no 100-point jump on the first free frame.
        self.detached = Some(MoveDrag {
            start_mouse: mouse,
            start_frame: self.current.shown,
        });
        (self, self.current.shown, None)
    }
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
    Bottom,
}

#[derive(Clone, Copy, Debug)]
struct Dock {
    side: Side,
    shown: Geometry,
    screen: Geometry,
}
// Dock only when the *unconstrained* native drag would put a third of the
// window beyond the edge. A small gap or a flush placement never qualifies.
const DOCK_FRACTION: f64 = 1.0 / 3.0;
const TUCK_TAB: f64 = 24.0;
const UNDOCK_PULL: f64 = 100.0;
const REHIDE_DELAY: Duration = Duration::from_millis(200);
const REVEAL_DURATION: Duration = Duration::from_millis(300);
const REVEAL_FRAME_STEP: Duration = Duration::from_millis(16);
const KEYBOARD_GRACE: Duration = Duration::from_millis(600);

impl Dock {
    fn hidden(self) -> Geometry {
        Geometry {
            x: match self.side {
                Side::Left => self.screen.x + TUCK_TAB - self.shown.width,
                Side::Right => self.screen.x + self.screen.width - TUCK_TAB,
                Side::Bottom => self.shown.x,
            },
            y: if self.side == Side::Bottom {
                self.screen.y + TUCK_TAB - self.shown.height
            } else {
                self.shown.y
            },
            ..self.shown
        }
    }
}

#[derive(Clone, Copy)]
struct TabDrag {
    start_mouse: NSPoint,
    start_dock: Dock,
    dragged: bool,
}
impl TabDrag {
    fn moved(self, mouse: NSPoint) -> bool {
        (mouse.x - self.start_mouse.x).hypot(mouse.y - self.start_mouse.y) >= 4.0
    }
    fn dock_at(self, mouse: NSPoint) -> Dock {
        let mut dock = self.start_dock;
        match dock.side {
            Side::Left | Side::Right => {
                dock.shown.y = (dock.shown.y + mouse.y - self.start_mouse.y).clamp(
                    dock.screen.y,
                    dock.screen.y + dock.screen.height - dock.shown.height,
                );
            }
            Side::Bottom => {
                dock.shown.x = (dock.shown.x + mouse.x - self.start_mouse.x).clamp(
                    dock.screen.x,
                    dock.screen.x + dock.screen.width - dock.shown.width,
                );
            }
        }
        dock
    }
    fn safe_dock_at(self, mouse: NSPoint, others: &[Geometry]) -> Option<Dock> {
        let dock = self.dock_at(mouse);
        dock_position(dock.side, dock.shown, dock.screen, others)
    }
}

// A shared display boundary is not a safe tuck edge: the hidden body would
// simply move onto that monitor. Check the entire offscreen corridor.
fn overlaps(a: Geometry, b: Geometry) -> bool {
    a.x < b.x + b.width && a.x + a.width > b.x && a.y < b.y + b.height && a.y + a.height > b.y
}
fn dock_position(
    side: Side,
    frame: Geometry,
    screen: Geometry,
    others: &[Geometry],
) -> Option<Dock> {
    if frame.width > screen.width
        || frame.height > screen.height
        || screen.width <= TUCK_TAB
        || screen.height <= TUCK_TAB
    {
        return None;
    }
    let shown = Geometry {
        x: if side == Side::Left {
            screen.x
        } else if side == Side::Right {
            screen.x + screen.width - frame.width
        } else {
            frame
                .x
                .clamp(screen.x, screen.x + screen.width - frame.width)
        },
        y: if side == Side::Bottom {
            screen.y
        } else {
            frame
                .y
                .clamp(screen.y, screen.y + screen.height - frame.height)
        },
        ..frame
    };
    let dock = Dock {
        side,
        shown,
        screen,
    };
    let hidden = dock.hidden();
    let corridor = match side {
        Side::Left => Geometry {
            x: hidden.x,
            width: screen.x - hidden.x,
            ..shown
        },
        Side::Right => Geometry {
            x: screen.x + screen.width,
            width: hidden.x + hidden.width - screen.x - screen.width,
            ..shown
        },
        Side::Bottom => Geometry {
            y: hidden.y,
            height: screen.y - hidden.y,
            ..shown
        },
    };
    (!others.iter().any(|other| overlaps(corridor, *other))).then_some(dock)
}

// A revealed dock stays anchored while sliding parallel to its edge. Require
// an intentional inward pull to detach; small perpendicular wobbles snap back.
fn inward_distance(prior: Dock, intended: Geometry) -> f64 {
    match prior.side {
        Side::Left => intended.x - prior.screen.x,
        Side::Right => prior.screen.x + prior.screen.width - intended.x - intended.width,
        Side::Bottom => intended.y - prior.screen.y,
    }
}
fn sticky_dock(prior: Dock, intended: Geometry, others: &[Geometry]) -> Option<Dock> {
    if inward_distance(prior, intended) >= UNDOCK_PULL {
        return None;
    }
    Some(dock_position(prior.side, intended, prior.screen, others).unwrap_or(prior))
}
fn own_edge_overshoot(prior: Dock, intended: Geometry) -> bool {
    -inward_distance(prior, intended)
        / match prior.side {
            Side::Bottom => intended.height,
            Side::Left | Side::Right => intended.width,
        }
        >= DOCK_FRACTION
}

// AppKit may clamp its *actual* frame onscreen. The start-frame + global mouse
// delta still records the user's deliberate overshoot without moving the view.
fn dock_candidate(
    intended: Geometry,
    actual: Geometry,
    screen: Geometry,
    others: &[Geometry],
) -> Option<Dock> {
    let edges = [
        (Side::Left, (screen.x - intended.x) / intended.width),
        (
            Side::Right,
            (intended.x + intended.width - screen.x - screen.width) / intended.width,
        ),
        (Side::Bottom, (screen.y - intended.y) / intended.height),
    ];
    let mut edges = edges
        .into_iter()
        .filter(|(_, depth)| *depth >= DOCK_FRACTION)
        .collect::<Vec<_>>();
    edges.sort_by(|a, b| b.1.total_cmp(&a.1));
    edges
        .into_iter()
        .find_map(|(side, _)| dock_position(side, actual, screen, others))
}

// Positive but sub-threshold overshoot enters an open dock. The deep-tuck
// boundary belongs exclusively to dock_candidate, never both modes.
fn open_dock_candidate(
    intended: Geometry,
    actual: Geometry,
    screen: Geometry,
    others: &[Geometry],
) -> Option<Dock> {
    let edges = [
        (Side::Left, (screen.x - intended.x) / intended.width),
        (
            Side::Right,
            (intended.x + intended.width - screen.x - screen.width) / intended.width,
        ),
        (Side::Bottom, (screen.y - intended.y) / intended.height),
    ];
    if edges.iter().any(|(_, depth)| *depth >= DOCK_FRACTION) {
        return None;
    }
    let mut edges = edges
        .into_iter()
        .filter(|(_, depth)| *depth > 0.0)
        .collect::<Vec<_>>();
    edges.sort_by(|a, b| b.1.total_cmp(&a.1));
    edges
        .into_iter()
        .find_map(|(side, _)| dock_position(side, actual, screen, others))
}

fn aligned_dock(
    side: Side,
    frame: Geometry,
    screen: Geometry,
    others: &[Geometry],
) -> Option<Dock> {
    let aligned = match side {
        Side::Left => (frame.x - screen.x).abs() < 3.0,
        Side::Right => (frame.x + frame.width - screen.x - screen.width).abs() < 3.0,
        Side::Bottom => (frame.y - screen.y).abs() < 3.0,
    };
    aligned
        .then(|| dock_position(side, frame, screen, others))
        .flatten()
}

struct EdgeTuck {
    enabled: bool,
    dock: Option<Dock>,
    hidden: bool,
    auto_hide_armed: bool,
}
impl EdgeTuck {
    fn shown_or(&self, current: Geometry) -> Geometry {
        self.dock.map_or(current, |dock| dock.shown)
    }
    fn reveal(&mut self) -> Option<Geometry> {
        let dock = self.hidden.then_some(self.dock).flatten()?;
        self.hidden = false;
        self.auto_hide_armed = true;
        Some(dock.shown)
    }
    fn hide(&mut self, window: &NSWindow) -> bool {
        if !self.hidden
            && let Some(dock) = self.dock
        {
            self.hidden = true;
            self.auto_hide_armed = false;
            // Go directly from the AppKit release frame to the tucked frame.
            window.setFrame_display_animate(dock.hidden().into(), true, true);
            return true;
        }
        false
    }
}
// Drop the mutable state borrow before asking AppKit to position the shown
// handle or starting the animation; a nested RefCell borrow aborts the app.
fn reveal_from_tab(tuck: &RefCell<EdgeTuck>) -> Option<(Geometry, Dock)> {
    let target = tuck.borrow_mut().reveal()?;
    let dock = tuck.borrow().dock?;
    Some((target, dock))
}
// The tiny native NSBox is visible only when tucked; no Slint geometry or
// renderer layers are changed. In window coordinates the bottom tab is the
// *top* of the iPod, so account for a flipped content view.
fn indicator_frame(side: Side, size: NSSize, flipped: bool, hidden: bool) -> NSRect {
    let thickness = 4.0;
    let length = 28.0;
    let inset = (TUCK_TAB - thickness) / 2.0;
    let (x, y, width, height) = match side {
        Side::Left => (
            if hidden {
                size.width - TUCK_TAB + inset
            } else {
                inset
            },
            (size.height - length) / 2.0,
            thickness,
            length,
        ),
        Side::Right => (
            if hidden {
                inset
            } else {
                size.width - TUCK_TAB + inset
            },
            (size.height - length) / 2.0,
            thickness,
            length,
        ),
        Side::Bottom => (
            (size.width - length) / 2.0,
            if flipped == hidden {
                inset
            } else {
                size.height - TUCK_TAB + inset
            },
            length,
            thickness,
        ),
    };
    NSRect {
        origin: NSPoint { x, y },
        size: NSSize { width, height },
    }
}
fn shown_handle_hit(side: Side, size: NSSize, flipped: bool, point: NSPoint) -> bool {
    let mark = indicator_frame(side, size, flipped, false);
    let target = match side {
        Side::Left | Side::Right => NSRect {
            origin: NSPoint {
                x: if side == Side::Left {
                    0.0
                } else {
                    size.width - TUCK_TAB
                },
                y: mark.origin.y + mark.size.height / 2.0 - 24.0,
            },
            size: NSSize {
                width: TUCK_TAB,
                height: 48.0,
            },
        },
        Side::Bottom => NSRect {
            origin: NSPoint {
                x: mark.origin.x + mark.size.width / 2.0 - 24.0,
                y: if flipped { size.height - TUCK_TAB } else { 0.0 },
            },
            size: NSSize {
                width: 48.0,
                height: TUCK_TAB,
            },
        },
    };
    point_in(target, point)
}
fn show_indicator(indicator: &NSBox, dock: Option<Dock>, hidden: bool) {
    if let (Some(dock), Some(view)) = (
        dock,
        indicator.window().and_then(|window| window.contentView()),
    ) {
        // Center in the actual visible content, including a bottom tab whose
        // content view may differ from the window frame.
        indicator.setFrame(indicator_frame(
            dock.side,
            view.bounds().size,
            view.isFlipped(),
            hidden,
        ));
        indicator.setHidden(false);
    } else {
        indicator.setHidden(true);
    }
}
fn should_passively_hide(
    interacting: bool,
    mouse_down: bool,
    since_key: Duration,
    cursor_inside: bool,
    app_busy: bool,
) -> bool {
    !interacting && !mouse_down && since_key >= KEYBOARD_GRACE && !cursor_inside && !app_busy
}
fn point_in(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x < frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y < frame.origin.y + frame.size.height
}

fn dock_for_window(
    window: &NSWindow,
    intended: Option<Geometry>,
    old_side: Option<Side>,
) -> Option<(Dock, bool)> {
    let active = window.screen()?;
    let visible = Geometry::from(active.visibleFrame());
    let others = NSScreen::screens(MainThreadMarker::new()?)
        .iter()
        .filter(|screen| screen.frame() != active.frame())
        .map(|screen| Geometry::from(screen.frame()))
        .collect::<Vec<_>>();
    let actual = window.frame().into();
    if let Some(intended) = intended {
        dock_candidate(intended, actual, visible, &others)
            .map(|dock| (dock, true))
            .or_else(|| {
                open_dock_candidate(intended, actual, visible, &others).map(|dock| (dock, false))
            })
    } else {
        old_side
            .and_then(|side| aligned_dock(side, actual, visible, &others))
            .map(|dock| (dock, false))
    }
}
// A deliberate release may hide immediately; a resize may only keep/release
// an existing dock. Text entry is never interrupted by an automatic hide.
fn should_tuck_on_release(deliberate: bool, naming_open: bool) -> bool {
    deliberate && !naming_open
}
fn snap_back_frame(actual: Geometry, screens: &[Geometry]) -> Option<Geometry> {
    let fitted = actual.restored(screens);
    (frame_difference(actual.into(), fitted.into()) > 0.5).then_some(fitted)
}
fn snap_shown_inside(window: &NSWindow) {
    if let Some(frame) = snap_back_frame(window.frame().into(), &screens()) {
        window.setFrame_display_animate(frame.into(), true, true);
    }
}
fn settle_dock(
    window: &NSWindow,
    tuck: &RefCell<EdgeTuck>,
    intended: Option<Geometry>,
    naming_open: bool,
) -> bool {
    let mut tuck = tuck.borrow_mut();
    if !tuck.enabled {
        if intended.is_some() {
            snap_shown_inside(window);
        }
        return false;
    }
    tuck.hidden = false;
    let prior = tuck.dock;
    let (candidate, deliberate) = if let (Some(prior), Some(intended)) = (prior, intended) {
        if screens().contains(&prior.screen) {
            let others = NSScreen::screens(
                MainThreadMarker::new().expect("dock settle runs on the AppKit thread"),
            )
            .iter()
            .filter(|screen| Geometry::from(screen.visibleFrame()) != prior.screen)
            .map(|screen| Geometry::from(screen.frame()))
            .collect::<Vec<_>>();
            (
                sticky_dock(prior, intended, &others),
                own_edge_overshoot(prior, intended),
            )
        } else {
            (None, false)
        }
    } else {
        dock_for_window(window, intended, prior.map(|dock| dock.side))
            .map_or((None, false), |(dock, deliberate)| (Some(dock), deliberate))
    };
    tuck.dock = candidate;
    if prior.is_none() || candidate.is_none() {
        // A new open dock must remain visible until the pointer has entered
        // its shown frame; the drop pointer may still be outside the screen.
        tuck.auto_hide_armed = false;
    }
    if let Some(dock) = tuck.dock {
        if should_tuck_on_release(deliberate, naming_open) {
            return tuck.hide(window);
        }
        window.setFrame_display_animate(dock.shown.into(), true, true);
    } else if intended.is_some() {
        // A non-tuck drag must not leave the shell cut off by the desktop edge.
        // Do not magnetically align an already-on-screen near-edge placement.
        snap_shown_inside(window);
    }
    false
}

fn finish_docked_drag(
    window: &NSWindow,
    tuck: &RefCell<EdgeTuck>,
    drag: DockedDrag,
    mouse: NSPoint,
    naming_open: bool,
) {
    if drag.detached.is_some() {
        snap_shown_inside(window);
    } else if drag.moved {
        settle_dock(
            window,
            tuck,
            Some(drag.gesture.intended(mouse)),
            naming_open,
        );
    }
}

fn reveal_frame(start: Geometry, target: Geometry, elapsed: Duration) -> Geometry {
    let t = (elapsed.as_secs_f64() / REVEAL_DURATION.as_secs_f64()).clamp(0.0, 1.0);
    let progress = 1.0 - (1.0 - t).powi(3);
    Geometry {
        x: start.x + (target.x - start.x) * progress,
        y: start.y + (target.y - start.y) * progress,
        ..target
    }
}

// AppKit's setFrame:display:animate: can finish a position-only move at once.
// Drive the existing NSWindow in points for a short, visible translation. This
// timer only runs during an explicit click reveal; never translate Slint layers.
fn animate_reveal(
    window: Retained<NSWindow>,
    tuck: Rc<RefCell<EdgeTuck>>,
    timer: Rc<slint::Timer>,
    target: Geometry,
) {
    let start: Geometry = window.frame().into();
    let started = Instant::now();
    let weak_timer = Rc::downgrade(&timer);
    timer.start(slint::TimerMode::Repeated, REVEAL_FRAME_STEP, move || {
        let Some(timer) = weak_timer.upgrade() else {
            return;
        };
        if tuck.borrow().hidden || tuck.borrow().dock.is_none_or(|dock| dock.shown != target) {
            timer.stop();
            return;
        }
        let elapsed = started.elapsed();
        window.setFrame_display(reveal_frame(start, target, elapsed).into(), true);
        if elapsed >= REVEAL_DURATION {
            timer.stop();
        }
    });
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
    let indicator = NSBox::initWithFrame(
        NSBox::alloc(MainThreadMarker::new().expect("indicator created on AppKit thread")),
        NSRect {
            origin: NSPoint { x: 0.0, y: 0.0 },
            size: NSSize {
                width: 4.0,
                height: 28.0,
            },
        },
    );
    indicator.setBoxType(NSBoxType::Custom);
    indicator.setTitlePosition(NSTitlePosition::NoTitle);
    indicator.setBorderWidth(0.0);
    indicator.setCornerRadius(2.0);
    indicator.setFillColor(&NSColor::colorWithSRGBRed_green_blue_alpha(
        0.40, 0.40, 0.40, 0.92,
    ));
    indicator.setHidden(true);
    tracking_view.addSubview(&indicator);
    let reveal_timer = Rc::new(slint::Timer::default());
    let tuck = Rc::new(RefCell::new(EdgeTuck {
        enabled: settings.borrow().preferences.edge_tuck_enabled,
        dock: None,
        hidden: false,
        auto_hide_armed: false,
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
    let reset_indicator = indicator.clone();
    let reset_reveal_timer = reveal_timer.clone();
    app.on_window_reset_requested(move || {
        reset_reveal_timer.stop();
        let frame = reset_tuck
            .borrow()
            .shown_or(reset_window.frame().into())
            .canonical()
            .restored(&screens());
        reset_indicator.setHidden(true);
        reset_tuck.borrow_mut().dock = None;
        reset_tuck.borrow_mut().hidden = false;
        reset_tuck.borrow_mut().auto_hide_armed = false;
        reset_window.setFrame_display(frame.into(), true);
        reset_settings
            .borrow_mut()
            .record(reset_window.frame().into(), Instant::now());
        save(&reset_settings);
    });
    let mode_tuck = tuck.clone();
    let mode_settings = settings.clone();
    let mode_window = window.clone();
    let mode_indicator = indicator.clone();
    let mode_reveal_timer = reveal_timer.clone();
    let weak = app.as_weak();
    app.on_window_tuck_requested(move || {
        let enabled = !mode_tuck.borrow().enabled;
        let mut state = mode_tuck.borrow_mut();
        if !enabled {
            mode_reveal_timer.stop();
            mode_indicator.setHidden(true);
            if let Some(dock) = state.dock {
                mode_window.setFrame_display(dock.shown.into(), true);
            }
            state.hidden = false;
            state.auto_hide_armed = false;
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
                    "Edge Tuck On — drag past edge"
                } else {
                    "Edge Tuck Off"
                }
                .into(),
            );
        }
    });

    let last_key = Rc::new(Cell::new(Instant::now() - KEYBOARD_GRACE));
    let hide_timer = Rc::new(slint::Timer::default());
    let pending_hide = Rc::new(Cell::new(false));
    let interacting = Rc::new(Cell::new(false));
    let tab_drag = Rc::new(Cell::new(None::<TabDrag>));
    let dock_drag = Rc::new(Cell::new(None::<DockedDrag>));
    let custom_cursor = Rc::new(Cell::new(false));
    let hide_window = window.clone();
    let hide_tuck = tuck.clone();
    let hide_pending = pending_hide.clone();
    let hide_interacting = interacting.clone();
    let hide_weak = app.as_weak();
    let hide_last_key = last_key.clone();
    let hide_indicator = indicator.clone();
    let hide_reveal_timer = reveal_timer.clone();
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
        let last_key = hide_last_key.clone();
        let indicator = hide_indicator.clone();
        let reveal_timer = hide_reveal_timer.clone();
        hide_clock.start(slint::TimerMode::SingleShot, REHIDE_DELAY, move || {
            pending.set(false);
            let busy = weak
                .upgrade()
                .is_some_and(|app| app.get_playlist_entry_open() || app.get_scrubbing());
            if !state.borrow().auto_hide_armed
                || reveal_timer.running()
                || !should_passively_hide(
                    interaction.get(),
                    NSEvent::pressedMouseButtons() != 0,
                    last_key.get().elapsed(),
                    point_in(window.frame(), NSEvent::mouseLocation()),
                    busy,
                )
            {
                return;
            }
            let tucked = state.borrow_mut().hide(&window);
            if tucked {
                reveal_timer.stop();
                show_indicator(&indicator, state.borrow().dock, true);
            }
        });
    });

    let moving = Rc::new(Cell::new(false));
    let move_gesture = Rc::new(Cell::new(None::<MoveDrag>));
    let finish_gesture = move_gesture.clone();
    let finish_moving = moving.clone();
    let finish_window = window.clone();
    let finish_tuck = tuck.clone();
    let finish_interacting = interacting.clone();
    let finish_indicator = indicator.clone();
    let finish_reveal_timer = reveal_timer.clone();
    let finish_weak = app.as_weak();
    let finish_move: Rc<dyn Fn()> = Rc::new(move || {
        if !finish_moving.replace(false) {
            return;
        }
        let window = finish_window.clone();
        let state = finish_tuck.clone();
        let interacting = finish_interacting.clone();
        let intended = finish_gesture
            .take()
            .map(|drag| drag.intended(NSEvent::mouseLocation()));
        // performWindowDragWithEvent may return before AppKit applies the final
        // mouse-up position. Settle on the next event-loop turn, not at mouse-down.
        let weak = finish_weak.clone();
        let indicator = finish_indicator.clone();
        let reveal_timer = finish_reveal_timer.clone();
        slint::Timer::single_shot(Duration::from_millis(60), move || {
            let naming_open = weak
                .upgrade()
                .is_some_and(|app| app.get_playlist_entry_open());
            reveal_timer.stop();
            settle_dock(&window, &state, intended, naming_open);
            let state = state.borrow();
            show_indicator(&indicator, state.dock, state.hidden);
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
    let observed_indicator = indicator.clone();
    let observed_reveal_timer = reveal_timer.clone();
    let observed_last_key = last_key.clone();
    let observed_interacting = interacting.clone();
    let observed_tab_drag = tab_drag.clone();
    let observed_dock_drag = dock_drag.clone();
    let observed_custom_cursor = custom_cursor.clone();
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
            // A native drag may lose its mouse-up; never discard a pending
            // *click* here, or a timer racing mouse-up would swallow reveal.
            if NSEvent::pressedMouseButtons() & 1 == 0
                && observed_tab_drag.get().is_some_and(|tab| tab.dragged)
            {
                observed_tab_drag.take();
                observed_interacting.set(false);
                if observed_tuck.borrow().hidden
                    && point_in(observed_window.frame(), NSEvent::mouseLocation())
                {
                    NSCursor::openHandCursor().set();
                    observed_custom_cursor.set(true);
                } else {
                    NSCursor::arrowCursor().set();
                    observed_custom_cursor.set(false);
                }
            }
            if NSEvent::pressedMouseButtons() & 1 == 0
                && let Some(drag) = observed_dock_drag.get()
                && drag.moved
            {
                observed_dock_drag.take();
                observed_interacting.set(false);
                observed_reveal_timer.stop();
                let naming_open = observed_weak
                    .upgrade()
                    .is_some_and(|app| app.get_playlist_entry_open());
                finish_docked_drag(
                    &observed_window,
                    &observed_tuck,
                    drag,
                    NSEvent::mouseLocation(),
                    naming_open,
                );
                let state = observed_tuck.borrow();
                show_indicator(&observed_indicator, state.dock, state.hidden);
                NSCursor::arrowCursor().set();
                observed_custom_cursor.set(false);
            }
            let current_screens = screens();
            if previous_screens != current_screens {
                observed_reveal_timer.stop();
                let frame = observed_tuck
                    .borrow()
                    .shown_or(observed_window.frame().into())
                    .restored(&current_screens);
                observed_indicator.setHidden(true);
                observed_tuck.borrow_mut().dock = None;
                observed_tuck.borrow_mut().hidden = false;
                observed_tuck.borrow_mut().auto_hide_armed = false;
                observed_window.setFrame_display(frame.into(), true);
                previous_screens = current_screens;
            }
            let busy = observed_weak
                .upgrade()
                .is_some_and(|app| app.get_playlist_entry_open() || app.get_scrubbing());
            let shown_dock = {
                let state = observed_tuck.borrow();
                state.dock.is_some() && !state.hidden
            };
            if shown_dock && point_in(observed_window.frame(), NSEvent::mouseLocation()) {
                observed_tuck.borrow_mut().auto_hide_armed = true;
            }
            if !observed_pending.get()
                && observed_tuck.borrow().auto_hide_armed
                && !observed_reveal_timer.running()
                && observed_tuck.borrow().dock.is_some()
                && !observed_tuck.borrow().hidden
                && should_passively_hide(
                    observed_interacting.get(),
                    NSEvent::pressedMouseButtons() != 0,
                    observed_last_key.get().elapsed(),
                    point_in(observed_window.frame(), NSEvent::mouseLocation()),
                    busy,
                )
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
    let resize_moving = moving.clone();
    let resize_move_gesture = move_gesture.clone();
    let resize_finish_move = finish_move.clone();
    let resize_window = window.clone();
    let resize_tuck = tuck.clone();
    let resize_interacting = interacting.clone();
    let resize_schedule = schedule_hide.clone();
    let resize_pending = pending_hide.clone();
    let resize_hide_timer = hide_timer.clone();
    let resize_indicator = indicator.clone();
    let resize_reveal_timer = reveal_timer.clone();
    let resize_last_key = last_key.clone();
    let resize_tab_drag = tab_drag.clone();
    let resize_dock_drag = dock_drag.clone();
    let resize_tracking_view = tracking_view.clone();
    let focus_weak = app.as_weak();
    let block: RcBlock<dyn Fn(NonNull<NSEvent>) -> *mut NSEvent> = RcBlock::new(
        move |event_ptr: NonNull<NSEvent>| -> *mut NSEvent {
            // SAFETY: AppKit provides the event pointer for this callback's duration.
            let event = unsafe { event_ptr.as_ref() };
            let event_type = event.r#type();

            if event_type == NSEventType::LeftMouseDragged {
                if let Some(mut tab) = resize_tab_drag.get() {
                    NSCursor::closedHandCursor().set();
                    custom_cursor.set(true);
                    let mouse = NSEvent::mouseLocation();
                    if tab.dragged || tab.moved(mouse) {
                        tab.dragged = true;
                        let others = NSScreen::screens(
                            MainThreadMarker::new().expect("tab drag runs on the AppKit thread"),
                        )
                        .iter()
                        .filter(|screen| {
                            Geometry::from(screen.visibleFrame()) != tab.start_dock.screen
                        })
                        .map(|screen| Geometry::from(screen.frame()))
                        .collect::<Vec<_>>();
                        let mut state = resize_tuck.borrow_mut();
                        if state.hidden
                            && state.dock.is_some_and(|current| {
                                current.side == tab.start_dock.side
                                    && current.screen == tab.start_dock.screen
                            })
                            && let Some(dock) = tab.safe_dock_at(mouse, &others)
                        {
                            state.dock = Some(dock);
                            resize_window.setFrame_display(dock.hidden().into(), true);
                        }
                    }
                    resize_tab_drag.set(Some(tab));
                    return std::ptr::null_mut();
                }
                if let Some(drag) = resize_dock_drag.get() {
                    let mouse = NSEvent::mouseLocation();
                    if !resize_tuck.borrow().enabled || !screens().contains(&drag.prior.screen) {
                        resize_dock_drag.take();
                        resize_interacting.set(false);
                        return std::ptr::null_mut();
                    }
                    let others = NSScreen::screens(
                        MainThreadMarker::new().expect("dock drag runs on the AppKit thread"),
                    )
                    .iter()
                    .filter(|screen| Geometry::from(screen.visibleFrame()) != drag.prior.screen)
                    .map(|screen| Geometry::from(screen.frame()))
                    .collect::<Vec<_>>();
                    let (next, frame, dock) = drag.advance(mouse, &others);
                    if next.moved {
                        let mut state = resize_tuck.borrow_mut();
                        state.dock = dock;
                        if dock.is_none() {
                            state.auto_hide_armed = false;
                        }
                        drop(state);
                        resize_window.setFrame_display(frame.into(), true);
                        if dock.is_none() {
                            resize_indicator.setHidden(true);
                        }
                        NSCursor::closedHandCursor().set();
                        custom_cursor.set(true);
                    }
                    resize_dock_drag.set(Some(next));
                    return std::ptr::null_mut();
                }
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
                if let Some(tab) = resize_tab_drag.take() {
                    resize_interacting.set(false);
                    let over_tab = point_in(resize_window.frame(), NSEvent::mouseLocation());
                    if !tab.dragged && over_tab {
                        let application = NSApplication::sharedApplication(
                            MainThreadMarker::new().expect("AppKit events run on the main thread"),
                        );
                        if !application.isActive() || !resize_window.isKeyWindow() {
                            application.activate();
                            resize_window.makeKeyWindow();
                        }
                        if let Some(app) = focus_weak.upgrade() {
                            app.invoke_refocus_navigation();
                        }
                        if let Some((target, dock)) = reveal_from_tab(&resize_tuck) {
                            show_indicator(&resize_indicator, Some(dock), false);
                            animate_reveal(
                                resize_window.clone(),
                                resize_tuck.clone(),
                                resize_reveal_timer.clone(),
                                target,
                            );
                        }
                    }
                    if tab.dragged && over_tab && resize_tuck.borrow().hidden {
                        NSCursor::openHandCursor().set();
                        custom_cursor.set(true);
                    } else {
                        NSCursor::arrowCursor().set();
                        custom_cursor.set(false);
                    }
                    return std::ptr::null_mut();
                }
                if let Some(drag) = resize_dock_drag.take() {
                    resize_interacting.set(false);
                    resize_reveal_timer.stop();
                    let naming_open = focus_weak
                        .upgrade()
                        .is_some_and(|app| app.get_playlist_entry_open());
                    if drag.moved {
                        finish_docked_drag(
                            &resize_window,
                            &resize_tuck,
                            drag,
                            NSEvent::mouseLocation(),
                            naming_open,
                        );
                        let state = resize_tuck.borrow();
                        show_indicator(&resize_indicator, state.dock, state.hidden);
                    } else if drag.handle {
                        let dock = resize_tuck.borrow().dock;
                        let clicked = point_in(resize_window.frame(), NSEvent::mouseLocation())
                            && dock.is_some_and(|dock| {
                                shown_handle_hit(
                                    dock.side,
                                    resize_tracking_view.bounds().size,
                                    resize_tracking_view.isFlipped(),
                                    resize_tracking_view
                                        .convertPoint_fromView(event.locationInWindow(), None),
                                )
                            });
                        if clicked && !naming_open {
                            let tucked = resize_tuck.borrow_mut().hide(&resize_window);
                            if tucked {
                                show_indicator(&resize_indicator, resize_tuck.borrow().dock, true);
                            }
                        }
                    }
                    NSCursor::arrowCursor().set();
                    custom_cursor.set(false);
                    return std::ptr::null_mut();
                }
                if drag.take().is_some() {
                    resize_interacting.set(false);
                    resize_reveal_timer.stop();
                    settle_dock(&resize_window, &resize_tuck, None, false);
                    let state = resize_tuck.borrow();
                    show_indicator(&resize_indicator, state.dock, state.hidden);
                    return std::ptr::null_mut();
                }
                resize_finish_move();
            }

            if event.windowNumber() != resize_window.windowNumber() {
                return event_ptr.as_ptr();
            }
            if event_type == NSEventType::KeyDown {
                resize_last_key.set(Instant::now());
            }
            if event_type == NSEventType::MouseExited {
                if resize_tab_drag.get().is_none() && custom_cursor.replace(false) {
                    NSCursor::arrowCursor().set();
                }
                resize_schedule();
            } else if event_type == NSEventType::MouseEntered {
                if resize_tuck.borrow().hidden {
                    NSCursor::openHandCursor().set();
                    custom_cursor.set(true);
                    return std::ptr::null_mut();
                }
                if let Some(dock) = resize_tuck.borrow().dock
                    && shown_handle_hit(
                        dock.side,
                        resize_tracking_view.bounds().size,
                        resize_tracking_view.isFlipped(),
                        resize_tracking_view.convertPoint_fromView(event.locationInWindow(), None),
                    )
                {
                    NSCursor::pointingHandCursor().set();
                    custom_cursor.set(true);
                    return std::ptr::null_mut();
                }
            }
            if event_type == NSEventType::LeftMouseDown
                && !resize_tuck.borrow().hidden
                && resize_reveal_timer.running()
            {
                // A second press during the slide completes it before any
                // native drag/resizing can take control of the window frame.
                resize_reveal_timer.stop();
                if let Some(dock) = resize_tuck.borrow().dock {
                    resize_window.setFrame_display(dock.shown.into(), true);
                }
                return std::ptr::null_mut();
            }
            if event_type == NSEventType::LeftMouseDown
                && let Some(dock) = resize_tuck.borrow().dock
                && resize_tuck.borrow().hidden
            {
                resize_interacting.set(true);
                resize_hide_timer.stop();
                resize_pending.set(false);
                resize_tab_drag.set(Some(TabDrag {
                    start_mouse: NSEvent::mouseLocation(),
                    start_dock: dock,
                    dragged: false,
                }));
                NSCursor::closedHandCursor().set();
                custom_cursor.set(true);
                return std::ptr::null_mut();
            }
            if event_type == NSEventType::LeftMouseDown
                && let Some(dock) = resize_tuck.borrow().dock
                && !resize_tuck.borrow().hidden
                && shown_handle_hit(
                    dock.side,
                    resize_tracking_view.bounds().size,
                    resize_tracking_view.isFlipped(),
                    resize_tracking_view.convertPoint_fromView(event.locationInWindow(), None),
                )
            {
                resize_interacting.set(true);
                resize_hide_timer.stop();
                resize_pending.set(false);
                resize_dock_drag.set(Some(DockedDrag::new(NSEvent::mouseLocation(), dock, true)));
                return std::ptr::null_mut();
            }
            let hit = shell_hit(event.locationInWindow(), resize_window.frame().size);
            if event_type == NSEventType::LeftMouseDown && !matches!(hit, ShellHit::Outside) {
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
            }
            if event_type == NSEventType::MouseMoved {
                if resize_tuck.borrow().hidden {
                    NSCursor::openHandCursor().set();
                    custom_cursor.set(true);
                    return std::ptr::null_mut();
                }
                if let Some(dock) = resize_tuck.borrow().dock
                    && shown_handle_hit(
                        dock.side,
                        resize_tracking_view.bounds().size,
                        resize_tracking_view.isFlipped(),
                        resize_tracking_view.convertPoint_fromView(event.locationInWindow(), None),
                    )
                {
                    NSCursor::pointingHandCursor().set();
                    custom_cursor.set(true);
                    return std::ptr::null_mut();
                }
                if let ShellHit::Resize(edge) = hit {
                    NSCursor::frameResizeCursorFromPosition_inDirections(
                        edge.cursor(),
                        NSCursorFrameResizeDirections::All,
                    )
                    .set();
                    custom_cursor.set(true);
                    return std::ptr::null_mut(); // Don't let winit replace our cursor.
                }
                if custom_cursor.replace(false) {
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
                    resize_interacting.set(true);
                    resize_hide_timer.stop();
                    resize_pending.set(false);
                    if let Some(dock) = resize_tuck.borrow().dock
                        && !resize_tuck.borrow().hidden
                    {
                        // A shown dock is locked to its edge *during* the drag.
                        // Free windows still use AppKit's native movement below.
                        resize_dock_drag.set(Some(DockedDrag::new(
                            NSEvent::mouseLocation(),
                            dock,
                            false,
                        )));
                    } else {
                        resize_move_gesture.set(Some(MoveDrag {
                            start_mouse: NSEvent::mouseLocation(),
                            start_frame: resize_window.frame().into(),
                        }));
                        resize_moving.set(true);
                        resize_window.performWindowDragWithEvent(event);
                        if NSEvent::pressedMouseButtons() & 1 == 0 {
                            resize_finish_move();
                        }
                    }
                    return std::ptr::null_mut();
                }
            }
            event_ptr.as_ptr()
        },
    );

    let mask = NSEventMask::KeyDown
        | NSEventMask::MouseMoved
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
        reveal_timer,
        window,
        settings,
        timer,
        hide_timer,
        tuck,
        tracking_view,
        tracking_area,
        indicator,
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
    fn deliberate_outer_edge_tuck_preserves_the_orthogonal_position() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let actual = Geometry {
            x: 7.0,
            y: 110.0,
            width: 420.0,
            height: 640.0,
        };
        // Even a flush or nearly flush window does not dock without a gesture.
        for x in [5.0, 0.0, -actual.width * DOCK_FRACTION + 0.1] {
            assert!(dock_candidate(Geometry { x, ..actual }, actual, screen, &[]).is_none());
        }
        let left = dock_candidate(
            Geometry {
                x: -141.0,
                ..actual
            },
            actual,
            screen,
            &[],
        )
        .unwrap();
        assert_eq!(left.side, Side::Left);
        assert_eq!((left.shown.x, left.shown.y), (0.0, 110.0));
        assert_eq!(left.hidden().x + left.hidden().width, TUCK_TAB);
        assert_eq!((left.hidden().width, left.hidden().height), (420.0, 640.0));
        let right_actual = Geometry {
            x: 1017.0,
            ..actual
        };
        let right = dock_candidate(
            Geometry {
                x: 1161.0,
                ..right_actual
            },
            right_actual,
            screen,
            &[],
        )
        .unwrap();
        assert_eq!(right.side, Side::Right);
        assert_eq!((right.shown.x, right.shown.y), (1020.0, 110.0));
        assert_eq!(right.hidden().x, 1440.0 - TUCK_TAB);
        let bottom_actual = Geometry {
            x: 307.0,
            y: 26.0,
            ..actual
        };
        assert!(
            dock_candidate(
                Geometry {
                    y: -188.0,
                    ..bottom_actual
                },
                bottom_actual,
                screen,
                &[]
            )
            .is_none()
        );
        let bottom = dock_candidate(
            Geometry {
                y: -190.0,
                ..bottom_actual
            },
            bottom_actual,
            screen,
            &[],
        )
        .unwrap();
        assert_eq!(bottom.side, Side::Bottom);
        assert_eq!((bottom.shown.x, bottom.shown.y), (307.0, 24.0));
        assert_eq!(bottom.hidden().y + bottom.hidden().height, 24.0 + TUCK_TAB);
        assert_eq!(bottom.hidden().x, bottom.shown.x);
        assert_eq!(
            EdgeTuck {
                enabled: true,
                dock: Some(bottom),
                hidden: true,
                auto_hide_armed: false,
            }
            .shown_or(bottom.hidden()),
            bottom.shown
        );
        assert_eq!(
            aligned_dock(Side::Bottom, bottom.shown, screen, &[])
                .unwrap()
                .shown,
            bottom.shown
        );
        assert!(
            aligned_dock(
                Side::Bottom,
                Geometry {
                    y: 29.0,
                    ..bottom.shown
                },
                screen,
                &[]
            )
            .is_none()
        );
        let drag = MoveDrag {
            start_mouse: NSPoint { x: 400.0, y: 600.0 },
            start_frame: actual,
        };
        assert_eq!(
            drag.intended(NSPoint { x: 260.0, y: 386.0 }),
            Geometry {
                x: -133.0,
                y: -104.0,
                ..actual
            }
        );
    }

    #[test]
    fn unqualified_drag_snaps_whole_window_onscreen_without_magnetic_edges() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let frame = Geometry {
            x: 12.0,
            y: 100.0,
            width: 420.0,
            height: 640.0,
        };
        assert_eq!(snap_back_frame(frame, &[screen]), None); // Nearby, not flush.
        assert_eq!(
            snap_back_frame(Geometry { x: -30.0, ..frame }, &[screen]),
            Some(Geometry {
                x: screen.x,
                ..frame
            })
        );
        assert_eq!(
            snap_back_frame(Geometry { x: 1400.0, ..frame }, &[screen]),
            Some(Geometry {
                x: screen.width - frame.width,
                ..frame
            })
        );
        assert_eq!(
            snap_back_frame(Geometry { y: 0.0, ..frame }, &[screen]),
            Some(Geometry {
                y: screen.y,
                ..frame
            })
        );
        assert_eq!(
            snap_back_frame(Geometry { y: 400.0, ..frame }, &[screen]),
            Some(Geometry {
                y: screen.y + screen.height - frame.height,
                ..frame
            })
        );
        let second = Geometry {
            x: 1440.0,
            ..screen
        };
        let on_second = Geometry { x: 1460.0, ..frame };
        assert_eq!(snap_back_frame(on_second, &[screen, second]), None);
    }

    #[test]
    fn revealing_a_tab_releases_mutable_state_before_reading_dock() {
        let shown = Geometry {
            x: 0.0,
            y: 24.0,
            width: 420.0,
            height: 640.0,
        };
        let dock = Dock {
            side: Side::Left,
            shown,
            screen: Geometry {
                x: 0.0,
                y: 24.0,
                width: 1440.0,
                height: 876.0,
            },
        };
        let tuck = RefCell::new(EdgeTuck {
            enabled: true,
            dock: Some(dock),
            hidden: true,
            auto_hide_armed: false,
        });
        assert_eq!(reveal_from_tab(&tuck).map(|(frame, _)| frame), Some(shown));
        assert!(!tuck.borrow().hidden);
        assert!(reveal_from_tab(&tuck).is_none());
    }

    #[test]
    fn partial_overshoot_opens_dock_and_one_third_overshoot_hides() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let frame = Geometry {
            x: 8.0,
            y: 100.0,
            width: 420.0,
            height: 640.0,
        };
        for (side, actual) in [
            (Side::Left, frame),
            (Side::Right, Geometry { x: 1012.0, ..frame }),
            (
                Side::Bottom,
                Geometry {
                    x: 300.0,
                    y: 30.0,
                    ..frame
                },
            ),
        ] {
            let beyond = |depth: f64| match side {
                Side::Left => Geometry {
                    x: screen.x - depth,
                    ..actual
                },
                Side::Right => Geometry {
                    x: screen.x + screen.width - actual.width + depth,
                    ..actual
                },
                Side::Bottom => Geometry {
                    y: screen.y - depth,
                    ..actual
                },
            };
            let length = if side == Side::Bottom {
                actual.height
            } else {
                actual.width
            };
            assert!(open_dock_candidate(beyond(0.0), actual, screen, &[]).is_none());
            assert!(open_dock_candidate(beyond(1.0), actual, screen, &[]).is_some());
            assert!(
                dock_candidate(beyond(length * DOCK_FRACTION - 0.01), actual, screen, &[])
                    .is_none()
            );
            assert!(
                open_dock_candidate(beyond(length * DOCK_FRACTION - 0.01), actual, screen, &[])
                    .is_some()
            );
            assert!(
                open_dock_candidate(beyond(length * DOCK_FRACTION), actual, screen, &[]).is_none()
            );
            assert!(dock_candidate(beyond(length * DOCK_FRACTION), actual, screen, &[]).is_some());
        }
        let neighbour = Geometry {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert!(
            open_dock_candidate(Geometry { x: -10.0, ..frame }, frame, screen, &[neighbour])
                .is_none()
        );
    }

    #[test]
    fn docked_drag_is_locked_live_then_detaches_without_a_jump() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        for (side, start) in [
            (
                Side::Left,
                Geometry {
                    x: 0.0,
                    y: 100.0,
                    width: 420.0,
                    height: 640.0,
                },
            ),
            (
                Side::Right,
                Geometry {
                    x: 1020.0,
                    y: 100.0,
                    width: 420.0,
                    height: 640.0,
                },
            ),
            (
                Side::Bottom,
                Geometry {
                    x: 200.0,
                    y: 24.0,
                    width: 420.0,
                    height: 640.0,
                },
            ),
        ] {
            let mouse = NSPoint { x: 300.0, y: 400.0 };
            let dock = Dock {
                side,
                shown: start,
                screen,
            };
            let drag = DockedDrag::new(mouse, dock, false);
            let parallel = if side == Side::Bottom {
                NSPoint { x: 340.0, y: 400.0 }
            } else {
                NSPoint { x: 300.0, y: 440.0 }
            };
            let (drag, locked, docked) = drag.advance(parallel, &[]);
            assert!(docked.is_some());
            assert_eq!(locked, docked.unwrap().shown);
            let almost = match side {
                Side::Left => NSPoint {
                    x: mouse.x + UNDOCK_PULL - 1.0,
                    y: parallel.y,
                },
                Side::Right => NSPoint {
                    x: mouse.x - UNDOCK_PULL + 1.0,
                    y: parallel.y,
                },
                Side::Bottom => NSPoint {
                    x: parallel.x,
                    y: mouse.y + UNDOCK_PULL - 1.0,
                },
            };
            let (drag, still_locked, docked) = drag.advance(almost, &[]);
            assert_eq!(still_locked, docked.unwrap().shown);
            let threshold = match side {
                Side::Left => NSPoint {
                    x: almost.x + 1.0,
                    ..almost
                },
                Side::Right => NSPoint {
                    x: almost.x - 1.0,
                    ..almost
                },
                Side::Bottom => NSPoint {
                    y: almost.y + 1.0,
                    ..almost
                },
            };
            let (drag, first_free, docked) = drag.advance(threshold, &[]);
            assert!(docked.is_none());
            assert_eq!(first_free, still_locked); // No 100-point visual jump.
            let outward = NSPoint {
                x: threshold.x + 10.0,
                y: threshold.y + 10.0,
            };
            let (_, next_free, docked) = drag.advance(outward, &[]);
            assert_eq!(next_free.x, first_free.x + 10.0);
            assert_eq!(next_free.y, first_free.y + 10.0);
            assert!(docked.is_none()); // Cannot re-dock during this drag.
        }
    }

    #[test]
    fn click_reveal_has_visible_intermediate_frames_on_each_side() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let frame = Geometry {
            x: 0.0,
            y: 100.0,
            width: 420.0,
            height: 640.0,
        };
        for side in [Side::Left, Side::Right, Side::Bottom] {
            let dock = dock_position(side, frame, screen, &[]).unwrap();
            let shown = dock.shown;
            let start = dock.hidden();
            assert_eq!(reveal_frame(start, shown, Duration::ZERO), start);
            let midway = reveal_frame(start, shown, REVEAL_DURATION / 2);
            assert_ne!(midway, start);
            assert_ne!(midway, shown);
            assert_eq!(reveal_frame(start, shown, REVEAL_DURATION), shown);
        }
    }

    #[test]
    fn revealed_dock_slides_along_edge_until_pulled_inward_100_points() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let actual = Geometry {
            x: 0.0,
            y: 100.0,
            width: 420.0,
            height: 640.0,
        };
        for (side, shown) in [
            (Side::Left, actual),
            (
                Side::Right,
                Geometry {
                    x: 1020.0,
                    ..actual
                },
            ),
            (
                Side::Bottom,
                Geometry {
                    x: 300.0,
                    y: 24.0,
                    ..actual
                },
            ),
        ] {
            let prior = Dock {
                side,
                shown,
                screen,
            };
            let along = match side {
                Side::Bottom => Geometry {
                    x: shown.x + 80.0,
                    ..shown
                },
                _ => Geometry {
                    y: shown.y + 40.0,
                    ..shown
                },
            };
            assert_eq!(sticky_dock(prior, along, &[]).unwrap().shown, along);
            assert!(!own_edge_overshoot(prior, along));
            // Even reaching another edge while sliding parallel does not
            // switch or tuck the current dock side.
            let crossed = match side {
                Side::Bottom => Geometry { x: -200.0, ..shown },
                _ => Geometry { y: -200.0, ..shown },
            };
            assert_eq!(sticky_dock(prior, crossed, &[]).unwrap().side, side);
            assert!(!own_edge_overshoot(prior, crossed));
            let almost = match side {
                Side::Left => Geometry {
                    x: screen.x + UNDOCK_PULL - 1.0,
                    ..along
                },
                Side::Right => Geometry {
                    x: shown.x - UNDOCK_PULL + 1.0,
                    ..along
                },
                Side::Bottom => Geometry {
                    y: screen.y + UNDOCK_PULL - 1.0,
                    ..along
                },
            };
            assert_eq!(sticky_dock(prior, almost, &[]).unwrap().shown, along);
            let detached = match side {
                Side::Left => Geometry {
                    x: almost.x + 1.0,
                    ..almost
                },
                Side::Right => Geometry {
                    x: almost.x - 1.0,
                    ..almost
                },
                Side::Bottom => Geometry {
                    y: almost.y + 1.0,
                    ..almost
                },
            };
            assert!(sticky_dock(prior, detached, &[]).is_none());
            let outward = match side {
                Side::Left => Geometry {
                    x: shown.x - shown.width * DOCK_FRACTION - 1.0,
                    ..shown
                },
                Side::Right => Geometry {
                    x: shown.x + shown.width * DOCK_FRACTION + 1.0,
                    ..shown
                },
                Side::Bottom => Geometry {
                    y: shown.y - shown.height * DOCK_FRACTION - 1.0,
                    ..shown
                },
            };
            assert!(own_edge_overshoot(prior, outward));
        }
    }

    #[test]
    fn deliberate_drop_hides_immediately_and_hover_does_not_reveal() {
        assert!(should_tuck_on_release(true, false));
        assert!(!should_tuck_on_release(true, true));
        assert!(!should_tuck_on_release(false, false));
        let state = EdgeTuck {
            enabled: true,
            dock: None,
            hidden: true,
            auto_hide_armed: false,
        };
        assert!(state.hidden); // Only the slit click handler calls reveal.
    }

    #[test]
    fn tucked_tab_drag_moves_only_along_the_edge_and_clamps_to_screen() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let shown = Geometry {
            x: 0.0,
            y: 110.0,
            width: 420.0,
            height: 640.0,
        };
        let start_mouse = NSPoint { x: 12.0, y: 400.0 };
        for (side, x) in [(Side::Left, 0.0), (Side::Right, 1020.0)] {
            let tab = TabDrag {
                start_mouse,
                start_dock: Dock {
                    side,
                    shown: Geometry { x, ..shown },
                    screen,
                },
                dragged: false,
            };
            assert!(!tab.moved(NSPoint { x: 14.0, y: 402.0 }));
            assert!(tab.moved(NSPoint { x: 19.0, y: 400.0 }));
            let shifted = tab.dock_at(NSPoint { x: 80.0, y: 440.0 });
            assert_eq!(shifted.shown.x, x);
            assert_eq!(shifted.shown.y, 150.0);
            assert_eq!(shifted.hidden().y, 150.0);
            assert_eq!(
                tab.dock_at(NSPoint { x: 12.0, y: -999.0 }).shown.y,
                screen.y
            );
            assert_eq!(
                tab.dock_at(NSPoint { x: 12.0, y: 9999.0 }).shown.y,
                screen.y + screen.height - shown.height
            );
        }
        let tab = TabDrag {
            start_mouse,
            start_dock: Dock {
                side: Side::Bottom,
                shown: Geometry {
                    y: screen.y,
                    ..shown
                },
                screen,
            },
            dragged: false,
        };
        let shifted = tab.dock_at(NSPoint { x: 112.0, y: 800.0 });
        assert_eq!(shifted.shown.x, 100.0);
        assert_eq!(shifted.shown.y, screen.y);
        assert_eq!(shifted.hidden().x, 100.0);
        assert_eq!(
            tab.dock_at(NSPoint {
                x: 9999.0,
                y: 400.0
            })
            .shown
            .x,
            screen.width - shown.width
        );
    }

    #[test]
    fn tucked_drag_cannot_slide_hidden_body_onto_another_monitor() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 1200.0,
        };
        let tab = TabDrag {
            start_mouse: NSPoint { x: 12.0, y: 600.0 },
            start_dock: Dock {
                side: Side::Left,
                shown: Geometry {
                    x: 0.0,
                    y: 500.0,
                    width: 420.0,
                    height: 640.0,
                },
                screen,
            },
            dragged: true,
        };
        let neighbour = Geometry {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 300.0,
        };
        assert!(
            tab.safe_dock_at(NSPoint { x: 12.0, y: 500.0 }, &[neighbour])
                .is_some()
        );
        assert!(
            tab.safe_dock_at(NSPoint { x: 12.0, y: 150.0 }, &[neighbour])
                .is_none()
        );
    }

    #[test]
    fn small_tab_mark_stays_inside_each_exposed_edge() {
        let size = NSSize {
            width: 420.0,
            height: 640.0,
        };
        let left = indicator_frame(Side::Left, size, false, true);
        assert!(left.origin.x >= size.width - TUCK_TAB);
        assert!(left.origin.x + left.size.width <= size.width);
        assert_eq!(
            left.origin.x + left.size.width / 2.0,
            size.width - TUCK_TAB / 2.0
        );
        let right = indicator_frame(Side::Right, size, false, true);
        assert!(right.origin.x >= 0.0);
        assert!(right.origin.x + right.size.width <= TUCK_TAB);
        assert_eq!(right.origin.x + right.size.width / 2.0, TUCK_TAB / 2.0);
        assert_eq!(right.size.width, 4.0);
        for flipped in [false, true] {
            let bottom = indicator_frame(Side::Bottom, size, flipped, true);
            assert!(bottom.origin.y >= if flipped { 0.0 } else { size.height - TUCK_TAB });
            assert!(
                bottom.origin.y + bottom.size.height
                    <= if flipped { TUCK_TAB } else { size.height }
            );
            assert_eq!(
                bottom.origin.y + bottom.size.height / 2.0,
                if flipped {
                    TUCK_TAB / 2.0
                } else {
                    size.height - TUCK_TAB / 2.0
                }
            );
            assert_eq!(bottom.size.height, 4.0);
            let shown = indicator_frame(Side::Bottom, size, flipped, false);
            assert_eq!(shown.origin.x + shown.size.width / 2.0, size.width / 2.0);
            assert!(shown_handle_hit(
                Side::Bottom,
                size,
                flipped,
                NSPoint {
                    x: size.width / 2.0,
                    y: if flipped {
                        size.height - TUCK_TAB / 2.0
                    } else {
                        TUCK_TAB / 2.0
                    }
                }
            ));
        }
        for side in [Side::Left, Side::Right] {
            let shown = indicator_frame(side, size, false, false);
            let x = if side == Side::Left {
                TUCK_TAB / 2.0
            } else {
                size.width - TUCK_TAB / 2.0
            };
            assert_eq!(shown.origin.x + shown.size.width / 2.0, x);
            assert!(shown_handle_hit(
                side,
                size,
                false,
                NSPoint {
                    x,
                    y: size.height / 2.0
                }
            ));
            assert!(!shown_handle_hit(
                side,
                size,
                false,
                NSPoint {
                    x: size.width / 2.0,
                    y: size.height / 2.0
                }
            ));
        }
    }

    #[test]
    fn leaving_revealed_tab_hides_even_if_still_key_except_during_interaction() {
        // Focus is intentionally not part of the passive-hide decision.
        assert!(should_passively_hide(
            false,
            false,
            KEYBOARD_GRACE,
            false,
            false
        ));
        assert!(!should_passively_hide(
            true,
            false,
            KEYBOARD_GRACE,
            false,
            false
        ));
        assert!(!should_passively_hide(
            false,
            true,
            KEYBOARD_GRACE,
            false,
            false
        ));
        assert!(!should_passively_hide(
            false,
            false,
            KEYBOARD_GRACE / 2,
            false,
            false
        ));
        assert!(!should_passively_hide(
            false,
            false,
            KEYBOARD_GRACE,
            true,
            false
        ));
        assert!(!should_passively_hide(
            false,
            false,
            KEYBOARD_GRACE,
            false,
            true
        ));
    }

    #[test]
    fn tuck_does_not_slide_onto_an_adjacent_display() {
        let screen = Geometry {
            x: 0.0,
            y: 24.0,
            width: 1440.0,
            height: 876.0,
        };
        let actual = Geometry {
            x: 10.0,
            y: 110.0,
            width: 420.0,
            height: 640.0,
        };
        let intended = Geometry {
            x: -141.0,
            ..actual
        };
        let left_neighbour = Geometry {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        assert!(dock_candidate(intended, actual, screen, &[left_neighbour]).is_none());
        let right_neighbour = Geometry {
            x: 1440.0,
            ..left_neighbour
        };
        assert!(
            dock_candidate(
                Geometry {
                    x: 1161.0,
                    ..actual
                },
                actual,
                screen,
                &[right_neighbour]
            )
            .is_none()
        );
        let below = Geometry {
            x: 0.0,
            y: -1080.0,
            width: 1440.0,
            height: 1104.0,
        };
        assert!(
            dock_candidate(
                Geometry {
                    y: -190.0,
                    ..actual
                },
                actual,
                screen,
                &[below]
            )
            .is_none()
        );
        assert!(
            dock_candidate(
                intended,
                actual,
                screen,
                &[Geometry {
                    y: 901.0,
                    ..left_neighbour
                }]
            )
            .is_some()
        );
        assert!(
            dock_candidate(
                intended,
                Geometry {
                    width: 630.0,
                    height: 960.0,
                    ..actual
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
