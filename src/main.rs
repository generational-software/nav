//! `nav` opens the working directory as a 3D plaza of folders.

mod commands;
mod geom;
mod jobs;
mod mark;
mod render;
mod scan;
mod scene;
mod sound;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window};

use crate::render::{Floor, Gpu};
use crate::scene::{Click, Navigator};
use crate::sound::{Clicks, Tap};

fn main() {
    if let Err(err) = run() {
        eprintln!("nav: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if asks_for_help(&args) {
        print!("{}", help_text());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--tag") {
        return tag_current(&args[1..]);
    }
    let options = options(&args)?;
    let path = start_path(options.path.as_deref())?;
    let event_loop = winit::event_loop::EventLoop::new()
        .map_err(|err| explain_event_loop(&err.to_string(), &wayland_env()))?;
    warn_if_bash_missing();
    let mut app = App {
        nav: Navigator::open(path, options.show_dots),
        gpu: None,
        cursor: (0.0, 0.0),
        press: None,
        started: Instant::now(),
        title: String::new(),
        clicks: if options.sounds { Clicks::start() } else { Clicks::silent() },
        hovered: None,
        floor: options.floor,
        startup_error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|err| explain_event_loop(&err.to_string(), &wayland_env()))?;
    match app.startup_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// What the process can see of the Wayland socket. Tests build this directly.
struct WaylandEnv {
    display: Option<String>,
    runtime_dir: Option<String>,
    socket_exists: bool,
}

fn env_string(key: &str) -> Option<String> {
    std::env::var_os(key).map(|value| value.to_string_lossy().into_owned())
}

fn wayland_env() -> WaylandEnv {
    let display = env_string("WAYLAND_DISPLAY");
    let runtime_dir = env_string("XDG_RUNTIME_DIR");
    let socket_exists = compositor_socket(display.as_deref(), runtime_dir.as_deref())
        .is_some_and(|path| path.exists());
    WaylandEnv {
        display,
        runtime_dir,
        socket_exists,
    }
}

/// Socket path `wayland-client` would open, when the two variables name one.
fn compositor_socket(display: Option<&str>, runtime_dir: Option<&str>) -> Option<PathBuf> {
    let display = display.filter(|value| !value.is_empty())?;
    let name = Path::new(display);
    if name.is_absolute() {
        return Some(name.to_path_buf());
    }
    let runtime_dir = runtime_dir.filter(|value| !value.is_empty())?;
    let runtime = Path::new(runtime_dir);
    if !runtime.is_absolute() {
        return None;
    }
    Some(runtime.join(name))
}

/// Turn a winit event-loop failure into a message that names the missing display.
fn explain_event_loop(detail: &str, env: &WaylandEnv) -> String {
    let detail = short_os_error(detail);
    let lower = detail.to_ascii_lowercase();
    if lower.contains("wayland") || lower.contains("compositor") {
        missing_wayland(detail, env)
    } else {
        format!("the window could not start. ({detail})")
    }
}

/// `os error at file:line: message` keeps the message and drops the crate path.
fn short_os_error(detail: &str) -> &str {
    let Some(rest) = detail.strip_prefix("os error at ") else {
        return detail;
    };
    rest.split_once(": ").map(|(_, message)| message).unwrap_or(detail)
}

fn missing_wayland(detail: &str, env: &WaylandEnv) -> String {
    let lower = detail.to_ascii_lowercase();
    let why = if lower.contains("neither wayland_display") {
        "WAYLAND_DISPLAY, WAYLAND_SOCKET, and DISPLAY are all unset, so there is no display to draw on"
            .to_string()
    } else if lower.contains("wayland_socket was set") {
        "WAYLAND_SOCKET was set, but it is not a usable file descriptor".to_string()
    } else if lower.contains("wayland library") {
        "the Wayland client library could not be loaded".to_string()
    } else {
        match env.display.as_deref() {
            None => "WAYLAND_DISPLAY is not set, so nav cannot find the compositor".to_string(),
            Some(display) if display.is_empty() => {
                "WAYLAND_DISPLAY is empty, so nav cannot find the compositor".to_string()
            }
            Some(display) if Path::new(display).is_absolute() => {
                if env.socket_exists {
                    format!(
                        "WAYLAND_DISPLAY is \"{display}\", and that socket is there, but the connection failed"
                    )
                } else {
                    format!("WAYLAND_DISPLAY is \"{display}\", and that socket does not exist")
                }
            }
            Some(display) => match env.runtime_dir.as_deref() {
                None => format!(
                    "WAYLAND_DISPLAY is \"{display}\", but XDG_RUNTIME_DIR is not set, so the compositor socket cannot be found"
                ),
                Some(runtime) if runtime.is_empty() => format!(
                    "WAYLAND_DISPLAY is \"{display}\", but XDG_RUNTIME_DIR is empty, so the compositor socket cannot be found"
                ),
                Some(runtime) if !Path::new(runtime).is_absolute() => format!(
                    "WAYLAND_DISPLAY is \"{display}\", but XDG_RUNTIME_DIR is \"{runtime}\", which is not an absolute path"
                ),
                Some(runtime) => {
                    let path = Path::new(runtime).join(display);
                    if env.socket_exists {
                        format!(
                            "the compositor socket {} is there, but the connection failed",
                            path.display()
                        )
                    } else {
                        format!("the compositor socket {} does not exist", path.display())
                    }
                }
            },
        }
    };
    format!("no Wayland display. {why}. ({detail})")
}

/// Commands need bash. Browsing does not, so a missing bash is a warning.
fn warn_if_bash_missing() {
    if bash_is_executable(std::env::var_os("PATH").as_deref()) {
        return;
    }
    eprintln!("nav: bash was not found on PATH, so commands on the ring cannot run.");
}

fn bash_is_executable(path_var: Option<&OsStr>) -> bool {
    let Some(path_var) = path_var else {
        return false;
    };
    std::env::split_paths(path_var).any(|dir| {
        let candidate = dir.join("bash");
        std::fs::metadata(&candidate).is_ok_and(|meta| {
            use std::os::unix::fs::PermissionsExt;
            meta.is_file() && meta.permissions().mode() & 0o111 != 0
        })
    })
}

const USAGE: &str = "usage: nav [-d] [-s] [--floor grid|hex|checker] [directory]";

/// Text printed by `nav --help`. README.md includes this same text.
fn help_text() -> String {
    format!(
        "\
{USAGE}
       nav --tag <colour> <shape>
       nav -h
       nav --help

Open a 3D view of a directory. With no path, nav uses the working directory.
-d shows folders whose names start with a dot. Without it they stay hidden. H toggles them in the window.
-s turns on click sounds.
--floor picks the plaza floor. grid is a fine line grid and the default. hex is a hexagonal pattern. checker is a muted checkerboard.
-h and --help print this text.

Before a folder is drawn, nav reads <folder>/.nav if it is there.
The JSON keys are colour and shape. --tag writes that file in the working directory.
A Cargo.toml or package.json in the open folder puts its commands on the center ring as thicker segments. While a command is running, its segment glows and stands taller.
Click a segment to run that command in a login bash. A tile on the left shows its output. When it exits, Finished sits at the bottom of the tile.
With -s, a click plays a short tap. Opening a folder, going up, and clicking empty ground sound different.
Moving the pointer onto a folder plays a softer tap. A command plays a different chime.
colours: {colours}
shapes: {shapes}
colour may also be a hex value such as #c45a2a.
",
        colours = crate::mark::colour_names(),
        shapes = crate::mark::shape_names(),
    )
}

fn tag_current(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: nav --tag <colour> <shape>".into());
    }
    let dir = std::env::current_dir().map_err(|err| err.to_string())?;
    crate::mark::write_tag(&dir, &args[0], &args[1])
}

struct Options {
    sounds: bool,
    show_dots: bool,
    floor: Floor,
    path: Option<String>,
}

/// `-h` and `--help` print help and never reach here.
fn asks_for_help(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "-h" || arg == "--help")
}

/// `-d`, `-s`, and `--floor` may sit before or after the directory. Anything else that starts with `-` is an error.
fn options(args: &[String]) -> Result<Options, String> {
    let mut sounds = false;
    let mut show_dots = false;
    let mut floor = Floor::Grid;
    let mut path = None;
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        if arg == "-s" {
            sounds = true;
        } else if arg == "-d" {
            show_dots = true;
        } else if arg == "--floor" || arg.starts_with("--floor=") {
            let value = if let Some(value) = arg.strip_prefix("--floor=") {
                value.to_string()
            } else {
                index += 1;
                args.get(index).cloned().ok_or_else(|| {
                    format!("--floor needs a kind: {}\n{USAGE}", Floor::names())
                })?
            };
            floor = Floor::parse(&value).ok_or_else(|| {
                format!("unknown floor \"{value}\"\nfloors: {}\n{USAGE}", Floor::names())
            })?;
        } else if arg.starts_with('-') {
            return Err(format!("unknown option {arg}\n{USAGE}"));
        } else if path.is_some() {
            return Err(USAGE.into());
        } else {
            path = Some(arg.clone());
        }
        index += 1;
    }
    Ok(Options {
        sounds,
        show_dots,
        floor,
        path,
    })
}

fn start_path(path: Option<&str>) -> Result<PathBuf, String> {
    let raw = if let Some(path) = path {
        PathBuf::from(path)
    } else {
        std::env::current_dir().map_err(|err| err.to_string())?
    };
    let meta = std::fs::metadata(&raw).map_err(|err| format!("{}: {err}", raw.display()))?;
    if !meta.is_dir() {
        return Err(format!("{} is not a directory", raw.display()));
    }
    std::fs::canonicalize(&raw).map_err(|err| format!("{}: {err}", raw.display()))
}

struct Press {
    origin: (f32, f32),
    last: (f32, f32),
    moved: bool,
}

struct App {
    nav: Navigator,
    gpu: Option<Gpu>,
    cursor: (f32, f32),
    press: Option<Press>,
    started: Instant,
    title: String,
    clicks: Clicks,
    hovered: Option<String>,
    floor: Floor,
    /// Set when the window or the GPU cannot be opened. `run` returns it after the loop stops.
    startup_error: Option<String>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() || self.startup_error.is_some() {
            return;
        }
        let mut attributes = Window::default_attributes()
            .with_inner_size(LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(LogicalSize::new(640.0, 480.0))
            .with_title("nav");
        #[cfg(target_os = "linux")]
        {
            use winit::platform::wayland::WindowAttributesExtWayland;
            attributes = attributes.with_name("nav", "nav");
        }
        let window = match event_loop.create_window(attributes) {
            Ok(window) => Arc::new(window),
            Err(err) => {
                self.startup_error = Some(format!("could not open a window. ({err})"));
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Gpu::new(window, event_loop, self.floor)) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(err) => {
                self.startup_error = Some(err);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                    gpu.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x as f32, position.y as f32);
                self.drag();
            }
            WindowEvent::MouseInput { state, button, .. } => self.mouse(state, button),
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(offset) => (offset.y / 40.0) as f32,
                };
                self.nav.zoom(steps);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match event.logical_key {
                    Key::Named(NamedKey::Backspace) => {
                        self.nav.go_parent();
                    }
                    Key::Named(NamedKey::Escape) if !event.repeat => event_loop.exit(),
                    Key::Named(NamedKey::Home) => self.nav.reset_camera(),
                    Key::Character(ch) if !event.repeat && (ch == "h" || ch == "H") => self.nav.toggle_dots(),
                    Key::Character(ch) if !event.repeat && (ch == "r" || ch == "R") => self.nav.rescan(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.redraw(),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

impl App {
    fn drag(&mut self) {
        let Some(press) = self.press.as_mut() else {
            return;
        };
        let dx = self.cursor.0 - press.last.0;
        let dy = self.cursor.1 - press.last.1;
        let ox = self.cursor.0 - press.origin.0;
        let oy = self.cursor.1 - press.origin.1;
        if ox * ox + oy * oy > 36.0 {
            press.moved = true;
        }
        if press.moved {
            self.nav.orbit(-dx * 0.005, dy * 0.004);
        }
        press.last = self.cursor;
    }

    fn mouse(&mut self, state: ElementState, button: MouseButton) {
        if button == MouseButton::Back && state == ElementState::Pressed {
            if self.nav.go_parent() {
                self.clicks.play(Tap::Up, 0.4);
            }
            return;
        }
        if button != MouseButton::Left {
            return;
        }
        match state {
            ElementState::Pressed => {
                self.press = Some(Press {
                    origin: self.cursor,
                    last: self.cursor,
                    moved: false,
                });
            }
            ElementState::Released => {
                let moved = self.press.take().is_some_and(|press| press.moved);
                if moved {
                    return;
                }
                let pitch = self.nav.hover_pitch();
                match self.nav.activate() {
                    Click::Miss => self.clicks.play(Tap::Miss, pitch),
                    Click::Quiet => {}
                    Click::Opened { up } => {
                        self.clicks.play(if up { Tap::Up } else { Tap::Open }, pitch);
                    }
                    Click::Acted => self.clicks.play(Tap::Open, pitch),
                }
            }
        }
    }

    fn redraw(&mut self) {
        let Some(gpu) = self.gpu.as_ref() else {
            return;
        };
        let (width, height) = gpu.size();
        self.nav.poll();
        let ray = self.nav.ray(self.cursor.0, self.cursor.1, width, height);
        let frame = self
            .nav
            .build_frame(Some(self.cursor), ray, width, height, self.started.elapsed().as_secs_f32());
        let title = self.nav.window_title();
        let pointer = frame.pointer;
        let gpu = self.gpu.as_mut().expect("gpu");
        if title != self.title {
            gpu.window.set_title(&title);
            self.title = title;
        }
        gpu.window.set_cursor(if pointer {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        });
        if let Err(err) = gpu.render(&frame) {
            eprintln!("nav: {err}");
        }
        self.note_hover();
    }

    /// Play once when the pointer arrives on a folder or a command. Dragging the view stays quiet.
    fn note_hover(&mut self) {
        if self.press.is_some() {
            return;
        }
        let hovered = self.nav.hover_key();
        if hovered == self.hovered {
            return;
        }
        self.hovered = hovered;
        if self.hovered.is_some() {
            let tap = if self.nav.hover_is_command() {
                Tap::Command
            } else {
                Tap::Hover
            };
            self.clicks.play(tap, self.nav.hover_pitch());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::{asks_for_help, bash_is_executable, explain_event_loop, help_text, options, WaylandEnv};
    use crate::render::Floor;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| (*part).to_string()).collect()
    }

    #[test]
    fn sounds_stay_off_unless_s_is_set() {
        let plain = options(&args(&[])).unwrap();
        assert!(!plain.sounds);
        assert!(plain.path.is_none());

        let flagged = options(&args(&["-s"])).unwrap();
        assert!(flagged.sounds);
        assert!(flagged.path.is_none());

        let before = options(&args(&["-s", "/tmp"])).unwrap();
        assert!(before.sounds);
        assert_eq!(before.path.as_deref(), Some("/tmp"));

        let after = options(&args(&["/tmp", "-s"])).unwrap();
        assert!(after.sounds);
        assert_eq!(after.path.as_deref(), Some("/tmp"));

        assert!(options(&args(&["-s", "-s"])).unwrap().sounds);
        assert!(!options(&args(&["-s"])).unwrap().show_dots);
        assert!(options(&args(&["/tmp", "/other"])).is_err());
        assert!(options(&args(&["--sound"])).is_err());
    }

    #[test]
    fn d_shows_dot_directories_from_the_start() {
        assert!(!options(&args(&[])).unwrap().show_dots);

        let flagged = options(&args(&["-d"])).unwrap();
        assert!(flagged.show_dots);
        assert!(!flagged.sounds);
        assert!(flagged.path.is_none());

        let before = options(&args(&["-d", "/tmp"])).unwrap();
        assert!(before.show_dots);
        assert_eq!(before.path.as_deref(), Some("/tmp"));

        let after = options(&args(&["/tmp", "-d"])).unwrap();
        assert!(after.show_dots);
        assert_eq!(after.path.as_deref(), Some("/tmp"));

        let both = options(&args(&["-s", "/tmp", "-d"])).unwrap();
        assert!(both.sounds);
        assert!(both.show_dots);

        assert!(options(&args(&["-d", "-d"])).unwrap().show_dots);
        assert!(options(&args(&["-h"])).is_err());
        assert!(options(&args(&["--help"])).is_err());
    }

    #[test]
    fn h_and_help_print_the_same_text() {
        assert!(asks_for_help(&args(&["-h"])));
        assert!(asks_for_help(&args(&["--help"])));
        assert!(asks_for_help(&args(&["/tmp", "-h"])));
        assert!(asks_for_help(&args(&["-s", "--help"])));
        assert!(!asks_for_help(&args(&["-d"])));
        assert!(!asks_for_help(&args(&[])));
    }

    #[test]
    fn floor_defaults_to_the_grid_and_names_the_other_two() {
        assert_eq!(options(&args(&[])).unwrap().floor, Floor::Grid);

        let hex = options(&args(&["--floor", "hex", "/tmp"])).unwrap();
        assert_eq!(hex.floor, Floor::Hex);
        assert_eq!(hex.path.as_deref(), Some("/tmp"));

        let after = options(&args(&["/tmp", "--floor", "checker"])).unwrap();
        assert_eq!(after.floor, Floor::Checker);
        assert_eq!(after.path.as_deref(), Some("/tmp"));

        let equals = options(&args(&["--floor=grid", "-s"])).unwrap();
        assert_eq!(equals.floor, Floor::Grid);
        assert!(equals.sounds);

        assert_eq!(options(&args(&["--floor", "HEX"])).unwrap().floor, Floor::Hex);
        assert!(options(&args(&["--floor"])).is_err());
        assert!(options(&args(&["--floor", "tiles"])).is_err());
        assert!(options(&args(&["--floor=nope"])).is_err());
    }

    fn look(display: Option<&str>, runtime_dir: Option<&str>, socket_exists: bool) -> WaylandEnv {
        WaylandEnv {
            display: display.map(str::to_string),
            runtime_dir: runtime_dir.map(str::to_string),
            socket_exists,
        }
    }

    #[test]
    fn a_missing_wayland_display_names_the_variable() {
        let detail = "os error at event_loop.rs:89: Could not find wayland compositor";
        let message = explain_event_loop(detail, &look(None, Some("/run/user/1000"), false));
        assert!(message.starts_with("no Wayland display."));
        assert!(message.contains("WAYLAND_DISPLAY is not set"));
        assert!(message.contains("Could not find wayland compositor"));
        assert!(!message.contains("event_loop.rs"));
        assert!(!message.starts_with("nav:"));
    }

    #[test]
    fn no_display_variable_names_all_three() {
        let detail = "os error at linux/mod.rs:765: neither WAYLAND_DISPLAY nor WAYLAND_SOCKET nor DISPLAY is set.";
        let message = explain_event_loop(detail, &look(None, None, false));
        assert!(message.contains(
            "WAYLAND_DISPLAY, WAYLAND_SOCKET, and DISPLAY are all unset, so there is no display to draw on"
        ));
        assert!(!message.contains("not a usable file descriptor"));
        assert!(!message.contains("linux/mod.rs"));
    }

    #[test]
    fn a_missing_runtime_dir_is_named() {
        let message = explain_event_loop(
            "Could not find wayland compositor",
            &look(Some("wayland-1"), None, false),
        );
        assert!(message.contains("WAYLAND_DISPLAY is \"wayland-1\""));
        assert!(message.contains("XDG_RUNTIME_DIR is not set"));
    }

    #[test]
    fn a_relative_runtime_dir_is_named() {
        let message = explain_event_loop(
            "Could not find wayland compositor",
            &look(Some("wayland-1"), Some("run/user"), false),
        );
        assert!(message.contains("XDG_RUNTIME_DIR is \"run/user\""));
        assert!(message.contains("not an absolute path"));
    }

    #[test]
    fn a_missing_socket_names_its_path() {
        let message = explain_event_loop(
            "Could not find wayland compositor",
            &look(Some("wayland-1"), Some("/run/user/1000"), false),
        );
        assert!(message.contains("the compositor socket /run/user/1000/wayland-1 does not exist"));
    }

    #[test]
    fn a_socket_that_is_there_still_reports_the_failed_connection() {
        let message = explain_event_loop(
            "Could not find wayland compositor",
            &look(Some("wayland-1"), Some("/run/user/1000"), true),
        );
        assert!(message.contains(
            "the compositor socket /run/user/1000/wayland-1 is there, but the connection failed"
        ));
    }

    #[test]
    fn an_absolute_display_that_is_missing_names_that_path() {
        let message = explain_event_loop(
            "Could not find wayland compositor",
            &look(Some("/tmp/nowhere.sock"), Some("/run/user/1000"), false),
        );
        assert!(message.contains("WAYLAND_DISPLAY is \"/tmp/nowhere.sock\""));
        assert!(message.contains("that socket does not exist"));
    }

    #[test]
    fn a_bad_wayland_socket_and_a_missing_library_are_named() {
        let socket = explain_event_loop(
            "WAYLAND_SOCKET was set but contained garbage",
            &look(Some("wayland-1"), Some("/run/user/1000"), true),
        );
        assert!(socket.contains("WAYLAND_SOCKET was set, but it is not a usable file descriptor"));

        let library = explain_event_loop(
            "The wayland library could not be loaded",
            &look(Some("wayland-1"), Some("/run/user/1000"), true),
        );
        assert!(library.contains("the Wayland client library could not be loaded"));
    }

    #[test]
    fn an_unrelated_event_loop_error_stays_a_window_error() {
        let message = explain_event_loop("EventLoop can't be recreated", &look(None, None, false));
        assert_eq!(message, "the window could not start. (EventLoop can't be recreated)");
        assert!(!message.contains("WAYLAND_DISPLAY"));
    }

    #[test]
    fn bash_must_be_an_executable_file_on_path() {
        let dir = std::env::temp_dir().join(format!("nav-bash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = RemoveDir(dir.clone());

        assert!(!bash_is_executable(None));
        assert!(!bash_is_executable(Some(OsStr::new(""))));
        assert!(!bash_is_executable(Some(dir.as_os_str())));

        let bash = dir.join("bash");
        std::fs::write(&bash, b"#!/bin/sh\n").unwrap();
        assert!(!bash_is_executable(Some(dir.as_os_str())));

        let mut perms = std::fs::metadata(&bash).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&bash, perms).unwrap();
        assert!(bash_is_executable(Some(dir.as_os_str())));
    }

    struct RemoveDir(PathBuf);

    impl Drop for RemoveDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_readme_includes_the_help_text() {
        let readme = include_str!("../README.md");
        let help = help_text();
        assert!(
            readme.contains(help.trim_end()),
            "README.md is missing the text from nav --help"
        );
    }
}
