//! Headless visual check of the real UI, without MPD or native window hooks.
//! cargo run --example now_playing_preview -- /path/to/cover.png
use slint::{
    ComponentHandle,
    platform::{
        Platform, PlatformError, WindowAdapter,
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
    },
};
use std::rc::Rc;

slint::include_modules!();

struct PreviewPlatform(Rc<MinimalSoftwareWindow>);
impl Platform for PreviewPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn key(app: &AppWindow, text: slint::SharedString) {
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyPressed { text: text.clone() });
    app.window()
        .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
    slint::platform::update_timers_and_animations();
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(PreviewPlatform(window.clone())))?;
    let app = AppWindow::new()?;
    app.set_page_title("Now Playing".into());
    app.set_browse_active(false);
    app.set_player_title("Cornfield Chase".into());
    app.set_player_artist("Hans Zimmer".into());
    app.set_player_album("Interstellar (Original Motion Picture Soundtrack)".into());
    app.set_player_elapsed(83.0);
    app.set_player_duration(126.0);
    app.set_player_elapsed_label("1:23".into());
    app.set_player_remaining_label("-0:43".into());
    app.set_player_playing(true);
    app.set_player_song_id("42".into());
    app.set_player_queue_position(0);
    app.set_player_queue_length(14);
    let seeks = Rc::new(std::cell::RefCell::new(Vec::new()));
    let received = seeks.clone();
    app.on_seek_requested(move |id, seconds| received.borrow_mut().push((id.to_string(), seconds)));
    app.set_battery_level(85);
    if let Some(path) = std::env::args().nth(1) {
        app.set_player_art(slint::Image::load_from_path(std::path::Path::new(&path))?);
        app.set_player_has_art(true);
    }
    app.show()?;
    std::fs::create_dir_all("target/now-playing-preview")?;
    // 420x640 reference shell. Headless sizes deliberately exercise beyond the
    // native window limits, which this preview never modifies or installs.
    for (percent, width, height) in [(75, 315, 480), (100, 420, 640), (150, 630, 960)] {
        window.set_size(slint::PhysicalSize::new(width, height));
        app.window().request_redraw();
        let mut pixels = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height);
        let rendered = window.draw_if_needed(|renderer| {
            renderer.render(pixels.make_mut_slice(), width as usize);
        });
        assert!(rendered);
        let path = format!("target/now-playing-preview/{percent}.png");
        image::save_buffer(
            &path,
            pixels.as_bytes(),
            width,
            height,
            image::ColorType::Rgb8,
        )?;
        println!("{path}");
        // Exercise the real top-level key handler, not a separate preview state.
        key(&app, slint::platform::Key::Return.into());
        assert!(app.get_scrubbing());
        key(&app, "k".into());
        assert_eq!(app.get_scrub_position(), 88.0);
        key(&app, "j".into());
        assert_eq!(app.get_scrub_position(), 83.0);
        key(&app, "k".into());
        app.window().request_redraw();
        window.draw_if_needed(|renderer| {
            renderer.render(pixels.make_mut_slice(), width as usize);
        });
        image::save_buffer(
            format!("target/now-playing-preview/{percent}-scrub.png"),
            pixels.as_bytes(),
            width,
            height,
            image::ColorType::Rgb8,
        )?;
        key(&app, slint::platform::Key::Escape.into());
        assert!(!app.get_scrubbing());
        assert!(seeks.borrow().is_empty());
    }
    // Commit is exactly one command; the backend later reconciles elapsed time.
    key(&app, slint::platform::Key::Return.into());
    key(&app, "j".into());
    key(&app, slint::platform::Key::Return.into());
    assert_eq!(*seeks.borrow(), vec![("42".to_string(), 78.0)]);
    assert!(!app.get_scrubbing());
    // Clicking and dragging on the same scaled bar uses that same seek callback.
    let scale = 1.5 * 0.98;
    let x = 6.3 + (50.0 + 19.0 + 2.0 + 278.0 * 0.25) * scale;
    let y = 9.6 + (32.0 + 61.0 + 166.0) * scale;
    let position = slint::LogicalPosition::new(x, y);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerPressed {
            position,
            button: slint::platform::PointerEventButton::Left,
        });
    assert!(app.get_scrubbing());
    let end = slint::LogicalPosition::new(x + 278.0 * 0.5 * scale, y);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerMoved { position: end });
    assert!((app.get_scrub_position() - 94.5).abs() < 0.1);
    app.window()
        .dispatch_event(slint::platform::WindowEvent::PointerReleased {
            position: end,
            button: slint::platform::PointerEventButton::Left,
        });
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), 2);
    assert!((seeks.borrow()[1].1 - 94.5).abs() < 0.1);
    key(&app, slint::platform::Key::Return.into());
    assert!(app.get_scrubbing());
    app.set_player_song_id("43".into());
    slint::platform::update_timers_and_animations();
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), 2);
    app.set_player_duration(0.0);
    key(&app, slint::platform::Key::Return.into());
    assert!(!app.get_scrubbing());
    app.set_player_duration(126.0);
    app.set_player_playing(false);
    app.set_player_paused(true);
    key(&app, slint::platform::Key::Return.into());
    assert!(app.get_scrubbing());
    key(&app, slint::platform::Key::Escape.into());
    assert!(app.get_player_paused());
    // Wheel rotation begins directly and commits exactly once on release.
    use slint::platform::{PointerEventButton as Button, WindowEvent as Event};
    let wheel = |angle: f32| {
        slint::LogicalPosition::new(
            6.3 + (210.0 + 90.0 * angle.cos()) * scale,
            9.6 + (471.0 + 90.0 * angle.sin()) * scale,
        )
    };
    let down = |position| {
        app.window().dispatch_event(Event::PointerPressed {
            position,
            button: Button::Left,
        })
    };
    let up = |position| {
        app.window().dispatch_event(Event::PointerReleased {
            position,
            button: Button::Left,
        })
    };
    let moved = |position| {
        app.window()
            .dispatch_event(Event::PointerMoved { position })
    };
    let transports = Rc::new(std::cell::RefCell::new(Vec::new()));
    let received = transports.clone();
    app.on_transport_action(move |action| received.borrow_mut().push(action.to_string()));
    seeks.borrow_mut().clear();
    down(wheel(0.0));
    assert!(!app.get_scrubbing());
    moved(wheel(std::f32::consts::FRAC_PI_2));
    assert!(app.get_scrubbing());
    assert!(app.get_scrub_position() > 83.0);
    assert!(seeks.borrow().is_empty());
    up(wheel(std::f32::consts::FRAC_PI_2));
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), 1);
    assert!(transports.borrow().is_empty());

    // Space commits; a held/repeated press cannot also toggle playback.
    key(&app, slint::platform::Key::Return.into());
    key(&app, "j".into());
    for _ in 0..2 {
        app.window()
            .dispatch_event(Event::KeyPressed { text: " ".into() });
    }
    app.window()
        .dispatch_event(Event::KeyReleased { text: " ".into() });
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().last().unwrap().1, 78.0);
    assert!(transports.borrow().is_empty());
    key(&app, " ".into());
    assert_eq!(*transports.borrow(), ["play-pause"]);

    // Physical Play button follows the same contextual behavior.
    key(&app, slint::platform::Key::Return.into());
    key(&app, "k".into());
    down(wheel(std::f32::consts::FRAC_PI_2));
    up(wheel(std::f32::consts::FRAC_PI_2));
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().last().unwrap().1, 88.0);
    assert_eq!(transports.borrow().len(), 1);

    // Changing track during a held wheel drag must not restart its preview.
    let before = seeks.borrow().len();
    down(wheel(0.0));
    moved(wheel(std::f32::consts::FRAC_PI_4));
    assert!(app.get_scrubbing());
    app.set_player_song_id("44".into());
    slint::platform::update_timers_and_animations();
    assert!(!app.get_scrubbing());
    moved(wheel(std::f32::consts::FRAC_PI_2));
    assert!(!app.get_scrubbing());
    up(wheel(std::f32::consts::FRAC_PI_2));
    assert_eq!(seeks.borrow().len(), before);

    // h/l share Previous/Next semantics in playback and preview-only ±interval
    // in scrubbing. Neither submits a seek until Enter confirms.
    transports.borrow_mut().clear();
    key(&app, "h".into());
    key(&app, "l".into());
    assert_eq!(*transports.borrow(), ["previous", "next"]);
    let before = seeks.borrow().len();
    key(&app, slint::platform::Key::Return.into());
    key(&app, "h".into());
    assert_eq!(app.get_scrub_position(), 78.0);
    key(&app, "l".into());
    assert_eq!(app.get_scrub_position(), 83.0);
    assert!(app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), before);
    key(&app, slint::platform::Key::Return.into());
    assert!(!app.get_scrubbing());
    assert_eq!(seeks.borrow().len(), before + 1);
    assert_eq!(transports.borrow().len(), 2);

    let steps = Rc::new(std::cell::RefCell::new(Vec::new()));
    let received = steps.clone();
    app.on_browse_step(move |step| received.borrow_mut().push(step));
    app.set_browse_active(true);
    app.set_page_title("Music".into());
    key(&app, "j".into());
    key(&app, "k".into());
    assert_eq!(*steps.borrow(), [1, -1]);
    println!(
        "Keyboard, mouse, automatic wheel seek, contextual Play, cancellation and list navigation checks passed"
    );
    Ok(())
}
