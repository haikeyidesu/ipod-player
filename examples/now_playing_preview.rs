//! Headless checks of the real UI, without MPD, HTTP or native window hooks.
//! cargo run --example now_playing_preview
use slint::{
    ComponentHandle, ModelRc, SharedString, VecModel,
    platform::{
        Platform, PlatformError, PointerEventButton as Button, WindowAdapter, WindowEvent as Event,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};
slint::include_modules!();
#[test]
fn lcd_tiles_are_translucent_repeatable_rgb_cells() {
    for (size, bytes) in [
        (5, &include_bytes!("../assets/lcd/rgb-cell-5.png")[..]),
        (6, &include_bytes!("../assets/lcd/rgb-cell-6.png")[..]),
        (9, &include_bytes!("../assets/lcd/rgb-cell-9.png")[..]),
    ] {
        let tile = image::load_from_memory(bytes).unwrap().to_rgba8();
        assert_eq!(tile.dimensions(), (size, size));
        let first = tile.get_pixel(0, 1).0;
        let middle = tile.get_pixel(size / 2, 1).0;
        let last = tile.get_pixel(size - 2, 1).0;
        assert!(first[0] > first[1] && middle[1] > middle[0] && last[2] > last[0]);
        assert!(tile.pixels().all(|p| p.0[3] <= 10));
        // Spacing is lighter than the RGB apertures, not an opaque mesh.
        assert!(tile.get_pixel(size - 1, 1).0[3] < first[3]);
        // A backlit RGB stripe has no horizontal grid or scanline boundaries.
        assert_eq!(tile.get_pixel(0, size - 1), tile.get_pixel(0, 0));
        assert_eq!(
            tile.get_pixel(size - 1, size - 1),
            tile.get_pixel(size - 1, 0)
        );
    }
}
#[test]
fn gloss_overlay_matches_device_geometry_and_leaves_wheel_clear() {
    let overlay = image::load_from_memory(include_bytes!("../assets/gloss/reflection-overlay.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(overlay.dimensions(), (840, 1280));
    let alpha = |x: u32, y: u32| overlay.get_pixel(x * 2, y * 2).0[3];
    for (x, y) in [(210, 471), (95, 471), (325, 471), (210, 356), (210, 586)] {
        assert_eq!(alpha(x, y), 0, "the full wheel and rim must stay clear");
    }
    assert_eq!(alpha(0, 0), 0);
    assert!(alpha(210, 100) <= 8);
    assert!(alpha(355, 55) > alpha(210, 165));
    assert!(alpha(13, 280) > alpha(210, 165));
    assert!(alpha(407, 280) > alpha(210, 165));
}
#[test]
fn headless_input_and_rendering() {
    main().unwrap();
}

struct PreviewPlatform(Rc<MinimalSoftwareWindow>, Instant);
impl Platform for PreviewPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
    fn duration_since_start(&self) -> Duration {
        self.1.elapsed()
    }
}
fn key(app: &AppWindow, text: impl Into<SharedString>) {
    let text = text.into();
    app.window()
        .dispatch_event(Event::KeyPressed { text: text.clone() });
    app.window().dispatch_event(Event::KeyReleased { text });
    slint::platform::update_timers_and_animations();
}
fn point(x: f32, y: f32) -> slint::LogicalPosition {
    // Reference 420x640 window, including the 2% native resize margin.
    slint::LogicalPosition::new(4.2 + x * 0.98, 6.4 + y * 0.98)
}
fn down(app: &AppWindow, p: slint::LogicalPosition) {
    app.window().dispatch_event(Event::PointerPressed {
        position: p,
        button: Button::Left,
    });
}
fn up(app: &AppWindow, p: slint::LogicalPosition) {
    app.window().dispatch_event(Event::PointerReleased {
        position: p,
        button: Button::Left,
    });
}
fn click(app: &AppWindow, p: slint::LogicalPosition) {
    down(app, p);
    up(app, p);
}
fn settle() {
    std::thread::sleep(Duration::from_millis(240));
    slint::platform::update_timers_and_animations();
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(PreviewPlatform(window.clone(), Instant::now())))?;
    let app = AppWindow::new()?;
    app.set_page_title("Now Playing".into());
    app.set_player_title("Evening Light".into());
    app.set_player_artist("Example Artist".into());
    app.set_player_album("Sample Album".into());
    app.set_player_elapsed(83.0);
    app.set_player_duration(126.0);
    app.set_player_elapsed_label("1:23".into());
    app.set_player_remaining_label("-0:43".into());
    app.set_player_playing(true);
    app.set_player_song_id("42".into());
    app.set_player_queue_position(0);
    app.set_player_queue_length(14);
    app.set_battery_level(85);
    let seeks = Rc::new(RefCell::new(Vec::new()));
    let received = seeks.clone();
    app.on_seek_requested(move |id, seconds| received.borrow_mut().push((id.to_string(), seconds)));
    let transports = Rc::new(RefCell::new(Vec::new()));
    let received = transports.clone();
    app.on_transport_action(move |action| received.borrow_mut().push(action.to_string()));
    let activations = Rc::new(RefCell::new(Vec::new()));
    let received = activations.clone();
    app.on_browse_open(move |index| received.borrow_mut().push(index));
    app.show()?;
    window.set_size(slint::PhysicalSize::new(420, 640));
    slint::platform::update_timers_and_animations();

    // Direct ±5s seeking, no activation or special mode.
    key(&app, "j");
    key(&app, "k");
    assert_eq!(
        *seeks.borrow(),
        vec![("42".into(), 78.0), ("42".into(), 83.0)]
    );
    assert!(!app.get_scrubbing());
    key(&app, "l");
    assert!(app.get_carousel_mode());
    key(&app, "j");
    assert_eq!(app.get_carousel_page(), 1);
    key(&app, "f");
    key(&app, "b");
    assert_eq!(
        &seeks.borrow()[2..],
        &[("42".into(), 88.0), ("42".into(), 83.0)]
    );
    key(&app, "n"); // Unbound, including in Lyrics.
    key(&app, "p");
    key(&app, "<");
    key(&app, ">");
    key(&app, " ");
    assert_eq!(*transports.borrow(), ["play-pause", "previous", "next"]);
    assert_eq!(seeks.borrow().len(), 4);
    key(&app, "h");
    assert_eq!(app.get_carousel_page(), 0);
    key(&app, "h");
    assert!(!app.get_carousel_mode());
    settle();

    // Transport seeking works from a browser too, without activating a row.
    app.set_page_title("Songs".into());
    app.set_browse_active(true);
    let before = seeks.borrow().len();
    let before_activations = activations.borrow().len();
    key(&app, "f");
    key(&app, "b");
    assert_eq!(seeks.borrow().len(), before + 2);
    assert_eq!(activations.borrow().len(), before_activations);
    app.set_page_title("Now Playing".into());
    app.set_browse_active(false);

    // Continuous bar control remains a single ID-bound seek on release.
    let p = point(50.0 + 19.0 + 2.0 + 278.0 * 0.25, 32.0 + 61.0 + 166.0);
    down(&app, p);
    assert!(app.get_scrubbing());
    let end = point(50.0 + 19.0 + 2.0 + 278.0 * 0.75, 32.0 + 61.0 + 166.0);
    app.window()
        .dispatch_event(Event::PointerMoved { position: end });
    up(&app, end);
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), 7);
    assert!((seeks.borrow()[6].1 - 94.5).abs() < 0.1);
    down(&app, p);
    assert!(app.get_scrubbing());
    app.set_player_song_id("43".into());
    slint::platform::update_timers_and_animations();
    up(&app, p);
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), 7);

    // Mouse activation of the playback view, then wheel scrolling to lyrics.
    let artwork = point(110.0, 130.0);
    click(&app, artwork);
    assert!(!app.get_carousel_mode());
    click(&app, artwork);
    assert!(app.get_carousel_mode());
    app.window().dispatch_event(Event::PointerScrolled {
        position: artwork,
        delta_x: 0.0,
        delta_y: 30.0,
    });
    assert_eq!(app.get_carousel_page(), 1);
    key(&app, "h");
    key(&app, "h");

    // The virtual centre button invokes the same activation, once.
    click(&app, point(210.0, 471.0));
    assert!(app.get_carousel_mode());
    // Rotating the ring switches panels rather than seeking.
    let before = seeks.borrow().len();
    let wheel = |a: f32| point(210.0 + 90.0 * a.cos(), 471.0 + 90.0 * a.sin());
    down(&app, wheel(0.0));
    app.window().dispatch_event(Event::PointerMoved {
        position: wheel(std::f32::consts::FRAC_PI_2),
    });
    up(&app, wheel(std::f32::consts::FRAC_PI_2));
    assert_eq!(app.get_carousel_page(), 1);
    assert_eq!(seeks.borrow().len(), before);

    // Plain lyrics use the same browse mode, but retain their position on exit.
    app.set_lyrics_plain(
        (0..30)
            .map(|i| format!("Example plain lyric line {i}\n"))
            .collect::<String>()
            .into(),
    );
    app.set_lyrics_message("".into());
    settle();
    key(&app, slint::platform::Key::Return);
    assert!(app.get_lyrics_manual());
    key(&app, "j");
    assert!(app.get_lyrics_scroll() > 0);
    let position = app.get_lyrics_scroll();
    key(&app, "h");
    assert!(!app.get_lyrics_manual());
    assert_eq!(app.get_lyrics_scroll(), position);
    assert_eq!(app.get_carousel_page(), 1);
    key(&app, slint::platform::Key::Return);
    assert!(app.get_lyrics_manual());
    assert_eq!(app.get_lyrics_scroll(), position);
    key(&app, slint::platform::Key::DownArrow);
    assert_eq!(app.get_lyrics_scroll(), position + 24);
    key(&app, slint::platform::Key::UpArrow);
    assert_eq!(app.get_lyrics_scroll(), position);
    app.window().dispatch_event(Event::PointerScrolled {
        position: point(150.0, 200.0),
        delta_x: 0.0,
        delta_y: 30.0,
    });
    assert_eq!(app.get_lyrics_scroll(), position + 24);
    down(&app, wheel(0.0));
    app.window().dispatch_event(Event::PointerMoved {
        position: wheel(std::f32::consts::FRAC_PI_2),
    });
    up(&app, wheel(std::f32::consts::FRAC_PI_2));
    assert!(app.get_lyrics_scroll() > position + 24);
    for _ in 0..app.get_lyrics_scroll_max() / 24 + 2 {
        key(&app, "j");
    }
    assert_eq!(app.get_lyrics_scroll(), app.get_lyrics_scroll_max());
    key(&app, "j");
    assert_eq!(app.get_lyrics_scroll(), app.get_lyrics_scroll_max());
    for _ in 0..app.get_lyrics_scroll_max() / 24 + 2 {
        key(&app, "k");
    }
    assert_eq!(app.get_lyrics_scroll(), 0);
    key(&app, "k");
    assert_eq!(app.get_lyrics_scroll(), 0);
    key(&app, slint::platform::Key::Return);
    assert!(!app.get_lyrics_manual());
    let lyric_point = point(150.0, 200.0);
    click(&app, lyric_point);
    click(&app, lyric_point);
    assert!(app.get_lyrics_manual());
    click(&app, point(210.0, 471.0));
    assert!(!app.get_lyrics_manual());
    key(&app, "h");
    assert_eq!(app.get_carousel_page(), 0);
    app.set_lyrics_plain("".into());
    app.set_lyrics_scroll(0);
    app.set_lyrics_lines(ModelRc::new(VecModel::from(
        vec![
            "The quiet evening settles",
            "A light across the water",
            "We follow it home",
        ]
        .into_iter()
        .map(SharedString::from)
        .collect::<Vec<_>>(),
    )));
    app.set_lyrics_active(1);
    let long_line = "When the light falls on the water and the long evening finally returns, we can find our way back home without missing a single word";
    let transcript = (0..24)
        .map(|n| format!("{n}: {long_line}\n\n"))
        .collect::<String>();
    app.set_lyrics_transcript(transcript.into());
    app.set_lyrics_prefix(format!("0: {long_line}\n\n").into());
    std::fs::create_dir_all("target/now-playing-preview")?;
    for (percent, width, height) in [(75, 315, 480), (100, 420, 640), (150, 630, 960)] {
        window.set_size(slint::PhysicalSize::new(width, height));
        for page in [0, 1] {
            app.set_carousel_page(page);
            settle();
            app.window().request_redraw();
            let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
            assert!(window.draw_if_needed(|renderer| {
                renderer.render(pixels.make_mut_slice(), width as usize);
            }));
            let path = format!("target/now-playing-preview/{percent}-page-{page}.png");
            image::save_buffer(
                &path,
                pixels.as_bytes(),
                width,
                height,
                image::ColorType::Rgb8,
            )?;
            println!("{path}");
        }
    }

    window.set_size(slint::PhysicalSize::new(420, 640));
    app.set_lyrics_lines(ModelRc::new(VecModel::from(vec![
        SharedString::from("Previous lyric"),
        SharedString::from(long_line),
        SharedString::from("Upcoming lyric"),
    ])));
    app.set_carousel_page(1);
    settle();
    app.window().request_redraw();
    let mut long_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(long_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/long-lyric.png",
        long_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    // Enter on synced lyrics freezes the viewport for manual scrolling. Paused
    // or changing elapsed must not alter this view; Enter/MENU resumes following.
    app.set_carousel_page(1);
    let lines = (0..24)
        .map(|i| {
            SharedString::from(if i % 2 == 0 {
                format!("{i}: {long_line}")
            } else {
                format!("Short lyric {i}")
            })
        })
        .collect::<Vec<_>>();
    app.set_lyrics_transcript(
        lines
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            .into(),
    );
    app.set_lyrics_prefix(
        format!(
            "{}\n\n",
            lines[..12]
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n\n")
        )
        .into(),
    );
    app.set_lyrics_lines(ModelRc::new(VecModel::from(lines)));
    app.set_lyrics_active(12);
    assert!(app.get_lyrics_active_position() > 0);
    key(&app, slint::platform::Key::Return);
    assert!(app.get_lyrics_manual());
    settle();
    app.window().request_redraw();
    let mut browse_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(browse_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/manual-lyrics.png",
        browse_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    let initial_scroll = app.get_lyrics_scroll();
    key(&app, "j");
    assert!(app.get_lyrics_scroll() > initial_scroll);
    // Verify painted transcript movement, not just a changing public offset.
    settle();
    app.window().request_redraw();
    let mut scrolled_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    window.draw_if_needed(|renderer| {
        renderer.render(scrolled_pixels.make_mut_slice(), 420);
    });
    // The transcript moved, but the compact song header did not.
    let region =
        |pixels: &slint::SharedPixelBuffer<slint::Rgb8Pixel>, top: usize, bottom: usize| {
            (top..bottom)
                .flat_map(|y| {
                    pixels.as_bytes()[(y * 420 + 75) * 3..(y * 420 + 345) * 3]
                        .iter()
                        .copied()
                })
                .collect::<Vec<_>>()
        };
    assert_ne!(
        region(&browse_pixels, 150, 260),
        region(&scrolled_pixels, 150, 260)
    );
    assert_eq!(
        region(&browse_pixels, 100, 135),
        region(&scrolled_pixels, 100, 135)
    );
    let before_playback = app.get_lyrics_scroll();
    app.set_lyrics_active(13);
    app.set_lyrics_prefix(format!("{}\n\n", app.get_lyrics_transcript()).into());
    app.set_player_elapsed(95.0);
    assert!(app.get_lyrics_manual());
    assert_eq!(app.get_lyrics_scroll(), before_playback);
    // All inputs use the same bounded transcript offset, never carousel/seek.
    key(&app, slint::platform::Key::DownArrow);
    assert_eq!(app.get_lyrics_scroll(), before_playback + 24);
    key(&app, slint::platform::Key::UpArrow);
    assert_eq!(app.get_lyrics_scroll(), before_playback);
    app.window().dispatch_event(Event::PointerScrolled {
        position: point(150.0, 200.0),
        delta_x: 0.0,
        delta_y: 30.0,
    });
    assert_eq!(app.get_lyrics_scroll(), before_playback + 24);
    let before_rotation = app.get_lyrics_scroll();
    down(&app, wheel(0.0));
    app.window().dispatch_event(Event::PointerMoved {
        position: wheel(std::f32::consts::FRAC_PI_2),
    });
    up(&app, wheel(std::f32::consts::FRAC_PI_2));
    assert!(app.get_lyrics_scroll() > before_rotation);
    for _ in 0..app.get_lyrics_scroll_max() / 24 + 2 {
        key(&app, "j");
    }
    assert_eq!(app.get_lyrics_scroll(), app.get_lyrics_scroll_max());
    key(&app, "j");
    assert_eq!(app.get_lyrics_scroll(), app.get_lyrics_scroll_max());
    for _ in 0..app.get_lyrics_scroll_max() / 24 + 2 {
        key(&app, "k");
    }
    assert_eq!(app.get_lyrics_scroll(), 0);
    key(&app, "k");
    assert_eq!(app.get_lyrics_scroll(), 0);
    assert_eq!(app.get_carousel_page(), 1);
    assert_eq!(seeks.borrow().len(), before);
    // Metadata is live and does not reset a manual viewport.
    app.set_player_title(
        "A replacement title that is long enough to need the existing marquee".into(),
    );
    app.set_player_artist("Replacement artist".into());
    assert_eq!(app.get_lyrics_scroll(), 0);
    key(&app, slint::platform::Key::Return);
    assert!(!app.get_lyrics_manual());
    assert_eq!(app.get_lyrics_scroll(), 0);
    click(&app, point(210.0, 471.0));
    assert!(app.get_lyrics_manual());
    click(&app, point(210.0, 471.0));
    assert!(!app.get_lyrics_manual());
    let lyric_point = point(150.0, 200.0);
    click(&app, lyric_point);
    click(&app, lyric_point);
    assert!(app.get_lyrics_manual());
    key(&app, "h");
    assert!(!app.get_lyrics_manual());
    assert_eq!(app.get_carousel_page(), 1);
    key(&app, "h");
    assert_eq!(app.get_carousel_page(), 0);

    // Real shared row template: single clicks select, double clicks activate
    // exactly once. Keyboard and centre use the same selected index.
    window.set_size(slint::PhysicalSize::new(420, 640));
    app.set_page_title("Album Actions".into());
    app.set_browse_active(true);
    app.set_browser_items(ModelRc::new(VecModel::from(vec![
        SharedString::from("Browse Songs"),
        SharedString::from("Play Album With A Very Long Name That Cannot Fit In One Row"),
    ])));
    app.set_browser_leaves(ModelRc::new(VecModel::from(vec![false, true])));
    settle();
    let row = point(150.0, 32.0 + 7.0 + 34.0 + 34.0 + 17.0);
    click(&app, row);
    assert_eq!(app.get_selected_index(), 1);
    assert!(activations.borrow().is_empty());
    std::thread::sleep(Duration::from_millis(1750));
    slint::platform::update_timers_and_animations();
    app.window().request_redraw();
    let mut marquee_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(marquee_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/selected-marquee.png",
        marquee_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    std::thread::sleep(Duration::from_millis(500));
    slint::platform::update_timers_and_animations();
    std::thread::sleep(Duration::from_millis(500));
    slint::platform::update_timers_and_animations();
    app.window().request_redraw();
    let mut moving_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(moving_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/moving-marquee.png",
        moving_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    click(&app, row);
    click(&app, row);
    assert_eq!(*activations.borrow(), [1]);
    key(&app, slint::platform::Key::Return);
    key(&app, "l");
    key(&app, slint::platform::Key::RightArrow);
    click(&app, point(210.0, 471.0));
    assert_eq!(*activations.borrow(), [1, 1, 1, 1, 1]);
    let steps = Rc::new(RefCell::new(Vec::new()));
    let received = steps.clone();
    app.on_browse_step(move |step| received.borrow_mut().push(step));
    key(&app, "j");
    key(&app, "k");
    key(&app, slint::platform::Key::DownArrow);
    key(&app, slint::platform::Key::UpArrow);
    // Small mouse/trackpad deltas accumulate to exactly one menu step.
    for n in 0..4 {
        app.window().dispatch_event(Event::PointerScrolled {
            position: row,
            delta_x: 0.0,
            delta_y: 12.0,
        });
        assert_eq!(steps.borrow().len(), 4 + usize::from(n == 3));
    }
    for n in 0..4 {
        app.window().dispatch_event(Event::PointerScrolled {
            position: row,
            delta_x: 0.0,
            delta_y: -12.0,
        });
        assert_eq!(steps.borrow().len(), 5 + usize::from(n == 3));
    }
    // A single coarse wheel notch is capped to one item.
    app.window().dispatch_event(Event::PointerScrolled {
        position: row,
        delta_x: 0.0,
        delta_y: 120.0,
    });
    assert_eq!(*steps.borrow(), [1, -1, 1, -1, 1, -1, 1]);
    down(&app, wheel(0.0));
    app.window().dispatch_event(Event::PointerMoved {
        position: wheel(std::f32::consts::FRAC_PI_2),
    });
    up(&app, wheel(std::f32::consts::FRAC_PI_2));
    assert!(steps.borrow().len() > 7);
    // Rotation batches detents into a signed delta; the browser owns selection.
    assert!(steps.borrow()[7..].iter().all(|step| *step > 0));
    assert_eq!(*activations.borrow(), [1, 1, 1, 1, 1]);
    let jumps = Rc::new(RefCell::new(Vec::new()));
    let received = jumps.clone();
    app.on_browse_jump(move |last| received.borrow_mut().push(last));
    key(&app, "g");
    key(&app, "G");
    assert_eq!(*jumps.borrow(), [false, true]);
    // A dockless native window can survive a workspace change while Slint's
    // keyboard scope has lost focus; an explicit user click refocuses it.
    app.window()
        .dispatch_event(Event::WindowActiveChanged(false));
    app.window()
        .dispatch_event(Event::WindowActiveChanged(true));
    app.invoke_refocus_navigation();
    let before_refocus = steps.borrow().len();
    key(&app, "j");
    assert_eq!(steps.borrow().len(), before_refocus + 1);

    // The MPD volume shortcut is global, independent of system volume. The
    // slider and keyboard feed one callback; unavailable mixers disable it.
    let volumes = Rc::new(RefCell::new(Vec::new()));
    let received = volumes.clone();
    let weak = app.as_weak();
    app.on_volume_requested(move |value| {
        received.borrow_mut().push(value);
        if let Some(app) = weak.upgrade() {
            app.set_volume_target(value);
        }
    });
    app.set_player_volume(40);
    key(&app, ",");
    key(&app, ".");
    assert_eq!(*volumes.borrow(), [35, 40]);
    app.set_volume_target(-1);
    app.set_page_title("Volume".into());
    app.set_volume_page(true);
    key(&app, "j");
    key(&app, "k");
    assert_eq!(*volumes.borrow(), [35, 40, 35, 40]);
    app.window().request_redraw();
    let mut volume_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(volume_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/volume-slider.png",
        volume_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    let slider = point(210.0, 185.0);
    down(&app, slider);
    up(&app, slider);
    assert_eq!(volumes.borrow().len(), 5);
    assert!((volumes.borrow()[4] - 50).abs() <= 2);
    app.set_volume_target(-1);
    app.set_player_volume(-1);
    key(&app, ".");
    click(&app, slider);
    assert_eq!(volumes.borrow().len(), 5);
    app.set_volume_page(false);
    app.set_browse_active(false);
    app.set_page_title("Now Playing".into());
    app.set_player_title("An Extremely Long Song Title That Needs To Scroll Across The iPod Display After A Short Pause".into());
    std::thread::sleep(Duration::from_millis(1750));
    slint::platform::update_timers_and_animations();
    app.window().request_redraw();
    let mut title_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(title_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/long-title.png",
        title_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    // Playlist naming keeps keyboard editing separate from Vim/menu navigation.
    let names = Rc::new(RefCell::new(Vec::new()));
    let received = names.clone();
    let weak = app.as_weak();
    app.on_playlist_name_submitted(move |name| {
        received.borrow_mut().push(name.to_string());
        weak.upgrade().unwrap().set_playlist_entry_open(false);
    });
    let cancelled = Rc::new(RefCell::new(0));
    let received = cancelled.clone();
    let weak = app.as_weak();
    app.on_playlist_name_cancelled(move || {
        *received.borrow_mut() += 1;
        weak.upgrade().unwrap().set_playlist_entry_open(false);
    });
    app.set_playlist_entry_text("".into());
    app.set_playlist_entry_open(true);
    let selected_before_entry = app.get_selected_index();
    for letter in ["M", "i", "j", "k", "x"] {
        key(&app, letter);
    }
    key(&app, slint::platform::Key::Backspace);
    key(&app, slint::platform::Key::Return);
    assert_eq!(*names.borrow(), ["Mijk"]);
    assert_eq!(app.get_selected_index(), selected_before_entry);
    assert!(!app.get_playlist_entry_open());
    app.set_playlist_entry_open(true);
    key(&app, slint::platform::Key::Escape);
    assert_eq!(*cancelled.borrow(), 1);
    assert!(!app.get_playlist_entry_open());

    // One passive strip replaces its message and restarts the dismissal timer.
    app.invoke_show_status("Added to queue".into());
    assert!(app.get_status_showing());
    std::thread::sleep(Duration::from_millis(210));
    slint::platform::update_timers_and_animations();
    app.window().request_redraw();
    let mut status_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(status_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/status-strip.png",
        status_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;
    let before_transport = transports.borrow().len();
    click(&app, point(210.0, 560.0));
    assert_eq!(transports.borrow().len(), before_transport + 1);
    std::thread::sleep(Duration::from_millis(1100));
    slint::platform::update_timers_and_animations();
    app.invoke_show_status("Volume 45%".into());
    assert_eq!(app.get_status_message().as_str(), "Volume 45%");
    std::thread::sleep(Duration::from_millis(850));
    slint::platform::update_timers_and_animations();
    assert!(app.get_status_showing());
    std::thread::sleep(Duration::from_millis(1000));
    slint::platform::update_timers_and_animations();
    assert!(!app.get_status_showing());
    // Render an unfocused marked song beside a focused unmarked song.
    app.set_page_title("Select Music".into());
    app.set_browse_active(true);
    app.set_picker_active(true);
    app.set_browser_items(ModelRc::from(Rc::new(VecModel::from(vec![
        SharedString::from("Add Selected (1)"),
        SharedString::from("Marked track"),
        SharedString::from("Focused track"),
    ]))));
    app.set_browser_leaves(ModelRc::from(Rc::new(VecModel::from(vec![true; 3]))));
    app.set_browser_songs(ModelRc::from(Rc::new(VecModel::from(vec![
        false, true, true,
    ]))));
    app.set_browser_marks(ModelRc::from(Rc::new(VecModel::from(vec![
        false, true, false,
    ]))));
    app.set_browser_queue_positions(ModelRc::from(Rc::new(VecModel::from(vec![-1; 3]))));
    app.set_selected_index(2);
    app.window().request_redraw();
    let mut picker_pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(420, 640);
    assert!(window.draw_if_needed(|renderer| {
        renderer.render(picker_pixels.make_mut_slice(), 420);
    }));
    image::save_buffer(
        "target/now-playing-preview/picker-marks.png",
        picker_pixels.as_bytes(),
        420,
        640,
        image::ColorType::Rgb8,
    )?;

    let picker_events = Rc::new(RefCell::new(Vec::new()));
    let received = picker_events.clone();
    app.on_browse_pick_space(move || received.borrow_mut().push("space".to_string()));
    let received = picker_events.clone();
    app.on_browse_bulk_select(move |invert| {
        received
            .borrow_mut()
            .push(if invert { "invert" } else { "all" }.into())
    });
    app.set_picker_active(true);
    let before_transport = transports.borrow().len();
    key(&app, slint::platform::Key::Space);
    key(&app, "a");
    key(&app, "A");
    assert_eq!(&*picker_events.borrow(), &["space", "all", "invert"]);
    assert_eq!(transports.borrow().len(), before_transport);
    app.set_picker_active(false);
    println!(
        "PASS: direct seek, carousel, lyrics, status strip, progress drag, LCD activation and virtual wheel"
    );
    Ok(())
}
