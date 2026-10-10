//! `nav` opens the working directory as a 3D plaza of folders.

mod commands;
mod geom;
mod mark;
mod render;
mod scan;
mod scene;
mod sound;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window};

use crate::render::Gpu;
use crate::scene::Navigator;
use crate::sound::{Clicks, Tap};

fn main() {
    if let Err(err) = run() {
        eprintln!("nav: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() == 1 && (args[0] == "-h" || args[0] == "--help") {
        print_help();
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--tag") {
        return tag_current(&args[1..]);
    }
    let path = start_path(&args)?;
    let event_loop = winit::event_loop::EventLoop::new().map_err(|err| err.to_string())?;
    let mut app = App {
        nav: Navigator::open(path),
        gpu: None,
        cursor: (0.0, 0.0),
        press: None,
        started: Instant::now(),
        title: String::new(),
        clicks: Clicks::start(),
        hovered: None,
    };
    event_loop.run_app(&mut app).map_err(|err| err.to_string())
}

fn print_help() {
    println!("usage: nav [directory]");
    println!("       nav --tag <colour> <shape>");
    println!();
    println!("Open a 3D view of a directory. With no path, nav uses the working directory.");
    println!();
    println!("Before a folder is drawn, nav reads <folder>/.nav if it is there.");
    println!("The JSON keys are colour and shape. --tag writes that file in the working directory.");
    println!("A Cargo.toml or package.json in the open folder puts its commands on the center ring.");
    println!("A click plays a short tap. Opening a folder, going up, and clicking empty ground sound different.");
    println!("Moving the pointer onto a folder plays a softer tap.");
    println!("colours: {}", crate::mark::colour_names());
    println!("shapes: {}", crate::mark::shape_names());
    println!("colour may also be a hex value such as #c45a2a.");
}

fn tag_current(args: &[String]) -> Result<(), String> {
    if args.len() != 2 {
        return Err("usage: nav --tag <colour> <shape>".into());
    }
    let dir = std::env::current_dir().map_err(|err| err.to_string())?;
    crate::mark::write_tag(&dir, &args[0], &args[1])
}

fn start_path(args: &[String]) -> Result<PathBuf, String> {
    let raw = if args.is_empty() {
        std::env::current_dir().map_err(|err| err.to_string())?
    } else if args.len() == 1 {
        PathBuf::from(&args[0])
    } else {
        return Err("usage: nav [directory]".into());
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
    hovered: Option<PathBuf>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
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
                eprintln!("nav: could not open a window: {err}");
                event_loop.exit();
                return;
            }
        };
        match pollster::block_on(Gpu::new(window, event_loop)) {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(err) => {
                eprintln!("nav: {err}");
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
                let going_up = self.nav.hovered_is_parent();
                if !self.nav.activate() {
                    self.clicks.play(Tap::Miss, pitch);
                    return;
                }
                self.clicks.play(if going_up { Tap::Up } else { Tap::Open }, pitch);
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
            .build_frame(ray, width, height, self.started.elapsed().as_secs_f32());
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

    /// Play once when the pointer arrives on a folder. Dragging the view stays quiet.
    fn note_hover(&mut self) {
        if self.press.is_some() {
            return;
        }
        let hovered = self.nav.hovered_path().map(|path| path.to_path_buf());
        if hovered == self.hovered {
            return;
        }
        self.hovered = hovered;
        if self.hovered.is_some() {
            self.clicks.play(Tap::Hover, self.nav.hover_pitch());
        }
    }
}
