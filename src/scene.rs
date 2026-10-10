//! The plaza of folders, and the ray that picks one.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::commands::Command;
use crate::geom::{SEGMENT_INNER, SEGMENT_OUTER};
use crate::jobs::{JobView, Jobs};
use crate::mark::{NavMark, Shape};
use crate::scan::{ScanEvent, Scanner};

const MAX_SHOWN: usize = 240;
const MAX_GRANDS: usize = 4;
const HELP: &str = "Drag to look around. Scroll to zoom. Click a folder to open it. Click a command to run it. Backspace goes up. H toggles dot folders. Escape exits.";

/// Tiles sit under the path title, stacked down the left of the window.
const TILE_W: f32 = 300.0;
const TILE_H: f32 = 96.0;
const TILE_X: f32 = 22.0;
const TILE_Y0: f32 = 92.0;
const TILE_GAP: f32 = 8.0;
const FINISHED_TAG_W: f32 = 84.0;
const FINISHED_TAG_H: f32 = 22.0;
/// Extra frame height so the Finished tag sits under the output.
const FINISHED_EXTRA: f32 = 32.0;

/// The "you are here" ring. The mesh is one unit tall, so the lip of the
/// opening is at `HERE_RING_Y + HERE_RING_HEIGHT`.
pub(crate) const HERE_RING_Y: f32 = 0.02;
pub(crate) const HERE_RING_HEIGHT: f32 = 0.045;
/// Instance scale of the unit ring. The pulse in the renderer adds a little on top.
pub(crate) const HERE_RING_SCALE: f32 = 1.62;
/// A command segment is this many times as tall as the ring.
const SEGMENT_THICK: f32 = 3.6;
/// Share of the circle each command keeps. The rest stays the thin ring.
const SEGMENT_FILL: f32 = 0.7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Parent,
    Child,
    Grandchild,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScanStatus {
    Idle,
    Scanning,
    Done,
    Failed,
}

struct ChildEntry {
    name: String,
    sort_key: String,
    path: PathBuf,
    symlink: bool,
    seen: u64,
}

struct DirRecord {
    children: Vec<ChildEntry>,
    status: ScanStatus,
    gen: u64,
    born: Option<Instant>,
    error: Option<String>,
}

impl DirRecord {
    fn new() -> Self {
        Self {
            children: Vec::new(),
            status: ScanStatus::Idle,
            gen: 0,
            born: None,
            error: None,
        }
    }
}

struct Camera {
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.62,
            pitch: 0.78,
            distance: 16.0,
        }
    }
}

impl Camera {
    fn eye(&self) -> Vec3 {
        let horizontal = self.pitch.cos() * self.distance;
        Vec3::new(
            horizontal * self.yaw.sin(),
            self.pitch.sin() * self.distance,
            horizontal * self.yaw.cos(),
        )
    }

    fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye(), Vec3::ZERO, Vec3::Y);
        let proj = Mat4::perspective_rh(50.0_f32.to_radians(), aspect.max(0.05), 0.08, 280.0);
        proj * view
    }
}

struct Placed {
    kind: Kind,
    path: PathBuf,
    label: String,
    center: Vec3,
    half: Vec3,
    model: Mat4,
    color: [f32; 4],
    emissive: f32,
    foot: f32,
    shape: Shape,
    pickable: bool,
    /// Center angle and sweep, in radians. A sweep of 0 is an ordinary stone.
    arc: [f32; 2],
    /// Index into the open folder's command list. Set only for a ring segment.
    command: Option<usize>,
    /// This segment's command is running in the open folder.
    running: bool,
}

struct Arrangement {
    nodes: Vec<Placed>,
    outer: f32,
    shown: usize,
    total: usize,
}

pub struct NodeDraw {
    pub model: Mat4,
    pub color: [f32; 4],
    pub emissive: f32,
    pub foot: f32,
    pub shape: Shape,
    pub arc: [f32; 2],
}

pub struct ScreenLabel {
    pub text: String,
    pub center_x: f32,
    pub top: f32,
    pub size: f32,
    pub color: [u8; 3],
}

/// One command, drawn as a rectangle on the left.
pub struct TileView {
    pub id: u64,
    pub name: String,
    pub lines: Vec<String>,
    pub running: bool,
    /// Set once the command has exited. The tag rect is then at the bottom of `rect`.
    pub finished: bool,
    pub rect: [f32; 4],
    pub arrow: [f32; 4],
    pub close: [f32; 4],
    /// Screen rect of the Finished tag. Zero when the command has not exited.
    pub tag: [f32; 4],
    pub arrow_hot: bool,
    pub close_hot: bool,
}

pub struct Frame {
    pub nodes: Vec<NodeDraw>,
    pub ground_radius: f32,
    pub ring_emissive: f32,
    pub time: f32,
    pub labels: Vec<ScreenLabel>,
    pub tiles: Vec<TileView>,
    pub title: String,
    pub meta: String,
    pub help: String,
    pub pointer: bool,
    pub view_proj: Mat4,
    pub camera_pos: Vec3,
}

/// What a click on the current aim should do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Click {
    /// The pointer was over empty ground.
    Miss,
    /// The pointer was on a tile, but not on a control.
    Quiet,
    /// A folder opened. `up` is the parent stone.
    Opened { up: bool },
    /// A command started, or its tile was closed.
    Acted,
}

#[derive(Clone, Debug)]
enum Aim {
    Folder(PathBuf),
    Command { index: usize, name: String },
    Back(u64),
    Kill(u64),
    Tile,
}

pub struct Navigator {
    current: PathBuf,
    dirs: HashMap<PathBuf, DirRecord>,
    marks: HashMap<PathBuf, NavMark>,
    commands: HashMap<PathBuf, Vec<Command>>,
    scanner: Scanner,
    camera: Camera,
    user_zoomed: bool,
    aim: Option<Aim>,
    jobs: Jobs,
    show_dots: bool,
}

impl Navigator {
    /// `show_dots` is the starting visibility of folders whose names start with a dot. H still toggles it.
    pub fn open(path: PathBuf, show_dots: bool) -> Self {
        let mut nav = Self {
            current: path.clone(),
            dirs: HashMap::new(),
            marks: HashMap::new(),
            commands: HashMap::new(),
            scanner: Scanner::spawn(),
            camera: Camera::default(),
            user_zoomed: false,
            aim: None,
            jobs: Jobs::new(),
            show_dots,
        };
        nav.request_scan(&path);
        nav
    }

    pub fn poll(&mut self) {
        let events = self.scanner.poll();
        for event in events {
            self.apply(event);
        }
        self.jobs.poll();
    }

    pub fn navigate(&mut self, path: PathBuf) {
        if path == self.current || !path.is_dir() {
            return;
        }
        self.current = path;
        self.user_zoomed = false;
        self.aim = None;
        let current = self.current.clone();
        self.request_scan(&current);
        self.schedule_children();
    }

    /// Move to the parent folder. Returns false at the root, or when the parent cannot be opened.
    pub fn go_parent(&mut self) -> bool {
        let Some(parent) = self.current.parent() else {
            return false;
        };
        if parent.as_os_str().is_empty() {
            return false;
        }
        let before = self.current.clone();
        self.navigate(parent.to_path_buf());
        self.current != before
    }

    pub fn hovered_path(&self) -> Option<&Path> {
        match &self.aim {
            Some(Aim::Folder(path)) => Some(path),
            _ => None,
        }
    }

    /// Folder or command under the pointer, so a hover tap plays once per arrival.
    pub fn hover_key(&self) -> Option<String> {
        match &self.aim {
            Some(Aim::Folder(path)) => Some(format!("dir:{}", path.display())),
            Some(Aim::Command { name, .. }) => Some(format!("cmd:{name}")),
            _ => None,
        }
    }

    /// The pointer is on a command segment, so the hover chime is not the folder tap.
    pub fn hover_is_command(&self) -> bool {
        matches!(self.aim, Some(Aim::Command { .. }))
    }

    /// A stable 0 to 1 tone for the stone or command under the pointer.
    pub fn hover_pitch(&self) -> f32 {
        let key = match &self.aim {
            Some(Aim::Folder(path)) => path.to_string_lossy().into_owned(),
            Some(Aim::Command { name, .. }) => name.clone(),
            _ => return 0.35,
        };
        (fnv(&key) % 1000) as f32 / 999.0
    }

    /// Show or hide folders whose names start with a dot. The scan itself keeps them.
    pub fn toggle_dots(&mut self) {
        self.show_dots = !self.show_dots;
    }

    pub fn rescan(&mut self) {
        self.marks.clear();
        self.commands.clear();
        let current = self.current.clone();
        let kids: Vec<(PathBuf, bool)> = self
            .dirs
            .get(&current)
            .map(|rec| rec.children.iter().map(|child| (child.path.clone(), child.symlink)).collect())
            .unwrap_or_default();
        for (path, symlink) in kids {
            if symlink {
                continue;
            }
            if let Some(rec) = self.dirs.get_mut(&path) {
                // Bump the generation so a listing already in flight is ignored.
                rec.gen = rec.gen.wrapping_add(1);
                rec.status = ScanStatus::Idle;
            }
        }
        self.force_scan(&current);
        self.schedule_children();
    }

    pub fn activate(&mut self) -> Click {
        match self.aim.clone() {
            Some(Aim::Folder(path)) => {
                let up = self.current.parent() == Some(path.as_path());
                let before = self.current.clone();
                self.navigate(path);
                if self.current != before {
                    Click::Opened { up }
                } else {
                    Click::Miss
                }
            }
            Some(Aim::Command { index, .. }) => {
                if self.launch(index) {
                    Click::Acted
                } else {
                    Click::Quiet
                }
            }
            Some(Aim::Back(id)) => {
                let Some(dir) = self.jobs.dir_of(id) else {
                    return Click::Quiet;
                };
                let before = self.current.clone();
                self.navigate(dir);
                if self.current != before {
                    Click::Opened { up: false }
                } else {
                    Click::Quiet
                }
            }
            Some(Aim::Kill(id)) => {
                self.jobs.close(id);
                Click::Acted
            }
            Some(Aim::Tile) => Click::Quiet,
            None => Click::Miss,
        }
    }

    fn launch(&mut self, index: usize) -> bool {
        let current = self.current.clone();
        let commands = self.commands_of(&current);
        let Some(command) = commands.get(index) else {
            return false;
        };
        self.jobs.start(&command.name, &current, &command.line).is_some()
    }

    pub fn orbit(&mut self, dyaw: f32, dpitch: f32) {
        self.camera.yaw += dyaw;
        self.camera.pitch = (self.camera.pitch + dpitch).clamp(0.22, 1.28);
    }

    pub fn zoom(&mut self, steps: f32) {
        let factor = (1.0 - steps * 0.09).clamp(0.55, 1.55);
        self.camera.distance = (self.camera.distance * factor).clamp(3.5, 90.0);
        self.user_zoomed = true;
    }

    pub fn reset_camera(&mut self) {
        let distance = self.camera.distance;
        self.camera = Camera::default();
        self.camera.distance = distance;
        self.user_zoomed = false;
    }

    pub fn window_title(&self) -> String {
        format!("nav — {}", ellipsize_middle(&self.current.display().to_string(), 80))
    }

    pub fn build_frame(
        &mut self,
        cursor: Option<(f32, f32)>,
        ray: Option<(Vec3, Vec3)>,
        width: f32,
        height: f32,
        time: f32,
    ) -> Frame {
        let mut arranged = self.layout(time);
        self.fit_camera(arranged.outer);
        let aspect = if height > 1.0 { width / height } else { 1.0 };
        let view_proj = self.camera.view_proj(aspect);
        let camera_pos = self.camera.eye();
        let mut tiles = self.tile_views(width);
        let tile_aim = cursor.and_then(|(x, y)| aim_tile(x, y, &tiles));
        self.aim = if tile_aim.is_some() {
            tile_aim
        } else {
            ray.and_then(|(origin, dir)| aim_node(&arranged.nodes, origin, dir))
        };
        for tile in &mut tiles {
            tile.arrow_hot = matches!(&self.aim, Some(Aim::Back(id)) if *id == tile.id);
            tile.close_hot = matches!(&self.aim, Some(Aim::Kill(id)) if *id == tile.id);
        }
        match &self.aim {
            Some(Aim::Folder(path)) => {
                for node in &mut arranged.nodes {
                    if node.command.is_none() && node.pickable && node.path == *path {
                        node.color = [0.98, 0.86, 0.55, 1.0];
                        node.emissive = 0.55;
                    }
                }
            }
            Some(Aim::Command { index, .. }) => {
                if let Some(node) = arranged
                    .nodes
                    .iter_mut()
                    .filter(|node| node.command.is_some())
                    .nth(*index)
                {
                    node.color = [1.0, 0.90, 0.62, 1.0];
                    node.emissive = 0.62;
                }
            }
            _ => {}
        }
        for node in &mut arranged.nodes {
            if !node.running {
                continue;
            }
            let hovered = matches!(
                &self.aim,
                Some(Aim::Command { index, .. }) if node.command == Some(*index)
            );
            style_running(node, time, hovered);
        }
        let folder_hover = match &self.aim {
            Some(Aim::Folder(path)) => Some(path.clone()),
            _ => None,
        };
        let labels = place_labels(&arranged.nodes, &self.current, view_proj, width, height, &folder_hover);
        let title_cols = ((width - 64.0) / 14.0).floor() as usize;
        let title = ellipsize_middle(&self.current.display().to_string(), title_cols.max(12));
        let meta = self.meta_line(arranged.shown, arranged.total, width);
        let busy = self.busy();
        let amp = if busy { 0.2 } else { 0.07 };
        let rate = if busy { 3.1 } else { 1.3 };
        Frame {
            nodes: arranged
                .nodes
                .into_iter()
                .map(|node| NodeDraw {
                    model: node.model,
                    color: node.color,
                    emissive: node.emissive,
                    foot: node.foot,
                    shape: node.shape,
                    arc: node.arc,
                })
                .collect(),
            ground_radius: arranged.outer + 8.0,
            ring_emissive: 0.3 + amp * (time * rate).sin(),
            time,
            labels,
            tiles,
            title,
            meta,
            help: HELP.to_string(),
            pointer: matches!(
                self.aim,
                Some(Aim::Folder(_) | Aim::Command { .. } | Aim::Back(_) | Aim::Kill(_))
            ),
            view_proj,
            camera_pos,
        }
    }

    fn tile_views(&self, width: f32) -> Vec<TileView> {
        let width = TILE_W.min((width - TILE_X - 16.0).max(180.0));
        let mut top = TILE_Y0;
        let mut tiles = Vec::new();
        for job in self.jobs.views() {
            let tile = tile_view(top, width, job, &self.current);
            top += tile.rect[3] + TILE_GAP;
            tiles.push(tile);
        }
        tiles
    }

    pub fn ray(&self, x: f32, y: f32, width: f32, height: f32) -> Option<(Vec3, Vec3)> {
        if width < 1.0 || height < 1.0 {
            return None;
        }
        let view_proj = self.camera.view_proj(width / height);
        let inverse = view_proj.inverse();
        let nx = (x / width) * 2.0 - 1.0;
        let ny = 1.0 - (y / height) * 2.0;
        let far = inverse * Vec4::new(nx, ny, 1.0, 1.0);
        if far.w.abs() < 1e-5 {
            return None;
        }
        let far = far.truncate() / far.w;
        let eye = self.camera.eye();
        let dir = far - eye;
        if dir.length_squared() < 1e-8 {
            return None;
        }
        Some((eye, dir.normalize()))
    }

    fn apply(&mut self, event: ScanEvent) {
        match event {
            ScanEvent::Child {
                parent,
                name,
                path,
                symlink,
                gen,
            } => {
                let accept = self.dirs.get(&parent).is_some_and(|rec| rec.gen == gen);
                if !accept {
                    return;
                }
                {
                    let rec = self.dirs.get_mut(&parent).expect("parent record");
                    if let Some(child) = rec.children.iter_mut().find(|child| child.path == path) {
                        child.seen = gen;
                        child.symlink = symlink;
                    } else {
                        rec.children.push(ChildEntry {
                            sort_key: name.to_lowercase(),
                            name,
                            path: path.clone(),
                            symlink,
                            seen: gen,
                        });
                    }
                }
                self.ensure(&path);
                if parent == self.current && !symlink {
                    self.request_scan(&path);
                }
            }
            ScanEvent::Finished { path, gen } => {
                let Some(rec) = self.dirs.get_mut(&path) else {
                    return;
                };
                if rec.gen != gen {
                    return;
                }
                rec.children.retain(|child| child.seen == gen);
                rec.status = ScanStatus::Done;
                rec.error = None;
            }
            ScanEvent::Failed { path, gen, message } => {
                let Some(rec) = self.dirs.get_mut(&path) else {
                    return;
                };
                if rec.gen != gen {
                    return;
                }
                rec.status = ScanStatus::Failed;
                rec.error = Some(message);
            }
        }
    }

    fn ensure(&mut self, path: &Path) {
        self.dirs.entry(path.to_path_buf()).or_insert_with(DirRecord::new);
    }

    fn request_scan(&mut self, path: &Path) {
        self.ensure(path);
        let gen = {
            let rec = self.dirs.get_mut(path).expect("record");
            if matches!(rec.status, ScanStatus::Scanning | ScanStatus::Done) {
                return;
            }
            rec.gen = rec.gen.wrapping_add(1);
            rec.status = ScanStatus::Scanning;
            rec.error = None;
            rec.gen
        };
        self.scanner.enqueue(path.to_path_buf(), gen);
    }

    fn force_scan(&mut self, path: &Path) {
        self.ensure(path);
        let gen = {
            let rec = self.dirs.get_mut(path).expect("record");
            rec.gen = rec.gen.wrapping_add(1);
            rec.status = ScanStatus::Scanning;
            rec.error = None;
            rec.gen
        };
        self.scanner.enqueue(path.to_path_buf(), gen);
    }

    /// Queue a listing for each real folder directly inside the current one.
    /// One extra level is enough to draw subfolders, and it stays bounded.
    fn schedule_children(&mut self) {
        let current = self.current.clone();
        let kids: Vec<(PathBuf, bool)> = self
            .dirs
            .get(&current)
            .map(|rec| rec.children.iter().map(|child| (child.path.clone(), child.symlink)).collect())
            .unwrap_or_default();
        for (path, symlink) in kids {
            if !symlink {
                self.request_scan(&path);
            }
        }
    }

    fn layout(&mut self, time: f32) -> Arrangement {
        let current = self.current.clone();
        let parent = current.parent().map(|path| path.to_path_buf());
        let parent_mark = parent.as_ref().map(|path| self.mark(path)).unwrap_or_default();
        let (order, total) = {
            let Some(rec) = self.dirs.get(&current) else {
                let mut arranged = arrange_full(parent.as_deref(), parent_mark, &[], time);
                self.attach_commands(&mut arranged, time);
                return arranged;
            };
            let mut order = visible_order(&rec.children, self.show_dots);
            let total = order.len();
            order.truncate(MAX_SHOWN);
            (order, total)
        };

        let mut children = Vec::with_capacity(order.len());
        for index in order {
            let entry = {
                let rec = self.dirs.get(&current).expect("current record");
                (
                    rec.children[index].name.clone(),
                    rec.children[index].path.clone(),
                    rec.children[index].symlink,
                )
            };
            let mark = self.mark(&entry.1);
            let snap = ChildSnap {
                name: entry.0,
                path: entry.1,
                symlink: entry.2,
                mark,
            };
            let (grands, child_count) = self.grand_snaps(&snap.path);
            let appear = self.appearance(&snap.path);
            let scanning = self
                .dirs
                .get(&snap.path)
                .is_some_and(|rec| rec.status == ScanStatus::Scanning);
            children.push(ChildSnapFull {
                snap,
                appear,
                scanning,
                child_count,
                grands,
            });
        }
        let shown = children.len();
        let mut arranged = arrange_full(parent.as_deref(), parent_mark, &children, time);
        arranged.total = total;
        arranged.shown = shown;
        self.attach_commands(&mut arranged, time);
        arranged
    }

    fn attach_commands(&mut self, arranged: &mut Arrangement, time: f32) {
        let current = self.current.clone();
        let commands = self.commands_of(&current);
        let running = self.jobs.running_names(&current);
        place_commands(&mut arranged.nodes, &commands, time, &running);
    }

    fn commands_of(&mut self, path: &Path) -> Vec<Command> {
        if let Some(found) = self.commands.get(path) {
            return found.clone();
        }
        let found = crate::commands::read_dir(path);
        self.commands.insert(path.to_path_buf(), found.clone());
        found
    }

    fn mark(&mut self, path: &Path) -> NavMark {
        if let Some(mark) = self.marks.get(path).copied() {
            return mark;
        }
        let mark = NavMark::read(path);
        self.marks.insert(path.to_path_buf(), mark);
        mark
    }

    fn grand_snaps(&mut self, path: &Path) -> (Vec<GrandSnap>, usize) {
        let Some(rec) = self.dirs.get(path) else {
            return (Vec::new(), 0);
        };
        let mut order = visible_order(&rec.children, self.show_dots);
        let total = order.len();
        order.truncate(MAX_GRANDS);
        let picked: Vec<(String, PathBuf)> = order
            .into_iter()
            .map(|index| (rec.children[index].name.clone(), rec.children[index].path.clone()))
            .collect();
        let mut grands = Vec::with_capacity(picked.len());
        for (name, grand_path) in picked {
            let appear = self.appearance(&grand_path);
            let mark = self.mark(&grand_path);
            grands.push(GrandSnap {
                name,
                path: grand_path,
                appear,
                mark,
            });
        }
        (grands, total)
    }

    fn appearance(&mut self, path: &Path) -> f32 {
        let Some(rec) = self.dirs.get_mut(path) else {
            return 1.0;
        };
        let born = rec.born.get_or_insert_with(Instant::now);
        let t = born.elapsed().as_secs_f32() / 0.42;
        ease(t).max(0.04)
    }

    fn busy(&self) -> bool {
        if self
            .dirs
            .get(&self.current)
            .is_some_and(|rec| rec.status == ScanStatus::Scanning)
        {
            return true;
        }
        let Some(rec) = self.dirs.get(&self.current) else {
            return false;
        };
        rec.children.iter().any(|child| {
            self.dirs
                .get(&child.path)
                .is_some_and(|rec| rec.status == ScanStatus::Scanning)
        })
    }

    fn meta_line(&self, shown: usize, total: usize, width: f32) -> String {
        let status = self.dirs.get(&self.current).map(|rec| rec.status);
        let error = self.dirs.get(&self.current).and_then(|rec| rec.error.clone());
        let mut line = if status == Some(ScanStatus::Failed) && total == 0 {
            format!("Could not read this folder ({})", error.unwrap_or_else(|| "unknown error".into()))
        } else {
            match total {
                0 if matches!(status, Some(ScanStatus::Scanning) | Some(ScanStatus::Idle) | None) => {
                    "Scanning…".to_string()
                }
                0 => "No folders here".to_string(),
                1 => "1 folder".to_string(),
                n if shown < n => format!("Showing {shown} of {n} folders"),
                n => format!("{n} folders"),
            }
        };
        if matches!(status, Some(ScanStatus::Scanning)) && total > 0 {
            line.push_str(" · scanning");
        } else if self.busy() && total > 0 && !line.contains("scanning") {
            line.push_str(" · scanning");
        }
        if !self.show_dots {
            line.push_str(" · dot folders hidden");
        }
        if let Some(path) = self.hovered_path() {
            let cols = ((width - 64.0) / 9.0).floor() as usize;
            let shown_path = ellipsize_middle(&path.display().to_string(), cols.max(16));
            line = format!("{shown_path} · {line}");
        }
        line
    }

    fn fit_camera(&mut self, outer: f32) {
        if self.user_zoomed {
            return;
        }
        let target = (outer * 1.18 + 7.0).clamp(12.0, 60.0);
        self.camera.distance += (target - self.camera.distance) * 0.14;
    }
}

struct ChildSnap {
    name: String,
    path: PathBuf,
    symlink: bool,
    mark: NavMark,
}

struct GrandSnap {
    name: String,
    path: PathBuf,
    appear: f32,
    mark: NavMark,
}

struct ChildSnapFull {
    snap: ChildSnap,
    appear: f32,
    scanning: bool,
    child_count: usize,
    grands: Vec<GrandSnap>,
}

fn visible_order(children: &[ChildEntry], show_dots: bool) -> Vec<usize> {
    let mut order: Vec<usize> = (0..children.len())
        .filter(|&index| show_dots || !children[index].name.starts_with('.'))
        .collect();
    order.sort_by(|&a, &b| compare_entries(&children[a], &children[b]));
    order
}

fn compare_entries(a: &ChildEntry, b: &ChildEntry) -> std::cmp::Ordering {
    a.name
        .starts_with('.')
        .cmp(&b.name.starts_with('.'))
        .then_with(|| a.sort_key.cmp(&b.sort_key))
        .then_with(|| a.name.cmp(&b.name))
}

fn arrange_full(
    parent: Option<&Path>,
    parent_mark: NavMark,
    children: &[ChildSnapFull],
    time: f32,
) -> Arrangement {
    let mut nodes = Vec::new();
    let plan = ring_plan(children.len());
    let mut cursor = 0;
    let mut outer = 2.6_f32;
    for (ring_index, &(radius, count)) in plan.iter().enumerate() {
        if count == 0 {
            continue;
        }
        let spacing = TAU * radius / count as f32;
        let full_r = (spacing * 0.27).clamp(0.18, 0.46);
        outer = outer.max(radius + full_r);
        for i in 0..count {
            let child = &children[cursor + i];
            let angle = -FRAC_PI_2 + i as f32 * TAU / count as f32;
            let x = angle.cos() * radius;
            let z = angle.sin() * radius;
            let appear = child.appear;
            let stone_r = full_r * appear;
            let extra = (child.child_count as f32).sqrt().min(4.0) * 0.28;
            let height = (1.12 + extra) * appear;
            let color = child
                .snap
                .mark
                .color
                .unwrap_or_else(|| child_color(&child.snap, ring_index));
            nodes.push(stone(
                Kind::Child,
                child.snap.path.clone(),
                short_name(&child.snap.name, 28),
                Vec3::new(x, 0.0, z),
                stone_r,
                height,
                color,
                glow(child.scanning, time, 0.05),
                spin_of(&child.snap.name),
                child.snap.mark.shape,
            ));
            place_grands(&mut nodes, child, Vec3::new(x, 0.0, z), full_r, spacing, time);
        }
        cursor += count;
    }

    if let Some(path) = parent {
        let parent_r = 0.78;
        let parent_h = 1.9;
        let z = -(outer + 3.1);
        outer = outer.max(z.abs() + parent_r);
        nodes.push(stone(
            Kind::Parent,
            path.to_path_buf(),
            format!("↑ {}", display_name(path)),
            Vec3::new(0.0, 0.0, z),
            parent_r,
            parent_h,
            parent_mark.color.unwrap_or([0.86, 0.56, 0.26, 1.0]),
            0.12,
            0.25,
            parent_mark.shape,
        ));
    }

    Arrangement {
        nodes,
        outer,
        shown: children.len(),
        total: children.len(),
    }
}

fn place_grands(
    nodes: &mut Vec<Placed>,
    child: &ChildSnapFull,
    origin: Vec3,
    full_r: f32,
    spacing: f32,
    time: f32,
) {
    if child.grands.is_empty() || child.snap.symlink {
        return;
    }
    // Sit beside the parent stone. Straight out along the ring hides the
    // back row behind the stone the camera is looking over.
    let radial = full_r + 0.28;
    let step = 1.7;
    let outward = origin.normalize_or_zero();
    if outward.length_squared() < 0.5 {
        return;
    }
    let side = Vec3::new(-outward.z, 0.0, outward.x);
    let n = child.grands.len();
    let half_span = if n <= 1 { step * 0.55 } else { (n - 1) as f32 * 0.5 * step };
    let reach = (radial * radial + half_span * half_span).sqrt();
    if reach >= spacing * 0.45 {
        return;
    }
    for (k, grand) in child.grands.iter().enumerate() {
        let along = if n == 1 {
            step * 0.55
        } else {
            (k as f32 - (n - 1) as f32 * 0.5) * step
        };
        let pos = origin + outward * radial + side * along;
        let appear = grand.appear;
        let radius = 0.2 * appear;
        let height = 0.72 * appear;
        let color = grand
            .mark
            .color
            .unwrap_or_else(|| jitter(&grand.name, [0.18, 0.42, 0.52]));
        nodes.push(stone(
            Kind::Grandchild,
            grand.path.clone(),
            short_name(&grand.name, 22),
            pos,
            radius,
            height,
            color,
            glow(false, time, 0.03),
            spin_of(&grand.name),
            grand.mark.shape,
        ));
    }
}

fn stone(
    kind: Kind,
    path: PathBuf,
    label: String,
    base: Vec3,
    radius: f32,
    height: f32,
    color: [f32; 4],
    emissive: f32,
    spin: f32,
    shape: Shape,
) -> Placed {
    let model = Mat4::from_scale_rotation_translation(
        Vec3::new(radius * 2.0, height.max(0.02), radius * 2.0),
        Quat::from_rotation_y(spin),
        base,
    );
    Placed {
        kind,
        path,
        label,
        center: Vec3::new(base.x, base.y + height * 0.5, base.z),
        half: Vec3::new(radius, height * 0.5, radius),
        model,
        color,
        emissive,
        foot: radius,
        shape,
        pickable: radius > 0.03 && height > 0.03,
        arc: [0.0, 0.0],
        command: None,
        running: false,
    }
}

fn place_commands(nodes: &mut Vec<Placed>, commands: &[Command], time: f32, running: &[String]) {
    let count = commands.len();
    if count == 0 {
        return;
    }
    let pulse = HERE_RING_SCALE + (time.sin() * 0.035);
    let sweep = TAU / count as f32 * SEGMENT_FILL;
    let mid = pulse * (SEGMENT_INNER + SEGMENT_OUTER) * 0.5;
    for (index, command) in commands.iter().enumerate() {
        let live = running.iter().any(|name| name == &command.name);
        let breathe = if live { 1.0 + (time * 3.2).sin() * 0.045 } else { 1.0 };
        let height = HERE_RING_HEIGHT * SEGMENT_THICK * if live { 1.22 * breathe } else { 1.0 };
        let base_y = if live { HERE_RING_Y + 0.012 } else { HERE_RING_Y };
        let angle = -FRAC_PI_2 + index as f32 * TAU / count as f32;
        let mut node = Placed {
            kind: Kind::Command,
            path: PathBuf::new(),
            label: short_name(&command.name, 20),
            center: Vec3::new(angle.cos() * mid, base_y + height * 0.5, angle.sin() * mid),
            half: Vec3::new(0.2, height * 0.5, 0.2),
            model: Mat4::from_scale_rotation_translation(
                Vec3::new(pulse, height, pulse),
                Quat::IDENTITY,
                Vec3::new(0.0, base_y, 0.0),
            ),
            color: [0.95, 0.72, 0.28, 1.0],
            emissive: 0.22,
            foot: 0.0,
            shape: Shape::Blob,
            pickable: true,
            arc: [angle, sweep],
            command: Some(index),
            running: live,
        };
        if live {
            style_running(&mut node, time, false);
        }
        nodes.push(node);
    }
}

/// A running segment stays brighter than its neighbours and keeps a slow pulse.
fn style_running(node: &mut Placed, time: f32, hovered: bool) {
    let pulse = (time * 3.2).sin();
    if hovered {
        node.color = [1.0, 0.94, 0.68, 1.0];
        node.emissive = 0.84 + pulse * 0.1;
    } else {
        node.color = [1.0, 0.84, 0.34, 1.0];
        node.emissive = 0.62 + pulse * 0.22;
    }
}

fn child_color(child: &ChildSnap, _ring: usize) -> [f32; 4] {
    if child.symlink {
        jitter(&child.name, [0.56, 0.42, 0.74])
    } else if child.name.starts_with('.') {
        jitter(&child.name, [0.32, 0.38, 0.50])
    } else {
        jitter(&child.name, [0.20, 0.58, 0.64])
    }
}

fn glow(scanning: bool, time: f32, base: f32) -> f32 {
    if scanning {
        base + 0.16 + (time * 3.6).sin() * 0.1
    } else {
        base
    }
}

fn ring_plan(n: usize) -> Vec<(f32, usize)> {
    let mut left = n;
    let mut ring = 0;
    let mut plan = Vec::new();
    while left > 0 && ring < 18 {
        let cap = 9 + ring * 5;
        let count = left.min(cap);
        let radius = 4.4 + ring as f32 * 2.6;
        plan.push((radius, count));
        left -= count;
        ring += 1;
    }
    plan
}

fn place_labels(
    nodes: &[Placed],
    current: &Path,
    view_proj: Mat4,
    width: f32,
    height: f32,
    hover: &Option<PathBuf>,
) -> Vec<ScreenLabel> {
    let mut pending = Vec::new();
    let here = Vec3::new(0.0, HERE_RING_Y + HERE_RING_HEIGHT, 0.0);
    if let Some((x, y, depth)) = project(here, view_proj, width, height) {
        let size = 22.0;
        pending.push(PendingLabel {
            text: display_name(current),
            center_x: x,
            top: y - label_body_center(size),
            size,
            color: [244, 248, 246],
            depth,
            priority: 0,
        });
    }
    for node in nodes {
        let Some((x, y, depth)) = project(
            Vec3::new(node.center.x, node.center.y + node.half.y + 0.08, node.center.z),
            view_proj,
            width,
            height,
        ) else {
            continue;
        };
        let hovered = hover.as_ref().is_some_and(|path| path == &node.path);
        let (size, color, priority) = match node.kind {
            Kind::Parent => (18.0, [255, 214, 170], 1),
            Kind::Child if hovered => (17.0, [255, 244, 214], 1),
            Kind::Child => (16.0, [236, 244, 242], 2),
            Kind::Grandchild if hovered => (14.0, [255, 244, 214], 1),
            Kind::Grandchild => (13.0, [186, 208, 210], 3),
            Kind::Command if node.running => (15.0, [255, 236, 196], 1),
            Kind::Command => (15.0, [255, 236, 210], 1),
        };
        pending.push(PendingLabel {
            text: node.label.clone(),
            center_x: x,
            top: y - size - 2.0,
            size,
            color,
            depth,
            priority,
        });
    }
    pending.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| a.depth.total_cmp(&b.depth))
    });
    let mut kept: Vec<PendingLabel> = Vec::new();
    for label in pending {
        if kept.iter().any(|other| labels_overlap(&label, other)) {
            continue;
        }
        kept.push(label);
    }
    kept.into_iter()
        .map(|label| ScreenLabel {
            text: label.text,
            center_x: label.center_x,
            top: label.top,
            size: label.size,
            color: label.color,
        })
        .collect()
}

struct PendingLabel {
    text: String,
    center_x: f32,
    top: f32,
    size: f32,
    color: [u8; 3],
    depth: f32,
    priority: u8,
}

/// Pixels from the top of a label to the middle of its letters.
/// The sans font, at a line height of 1.25em, puts that middle at 0.7em.
fn label_body_center(size: f32) -> f32 {
    size * 0.70
}

fn labels_overlap(a: &PendingLabel, b: &PendingLabel) -> bool {
    let aw = a.text.chars().count() as f32 * a.size * 0.52;
    let bw = b.text.chars().count() as f32 * b.size * 0.52;
    let ax = a.center_x - aw * 0.5;
    let bx = b.center_x - bw * 0.5;
    let pad = 6.0;
    ax < bx + bw + pad && ax + aw + pad > bx && a.top < b.top + b.size + pad && a.top + a.size + pad > b.top
}

fn project(point: Vec3, view_proj: Mat4, width: f32, height: f32) -> Option<(f32, f32, f32)> {
    let clip = view_proj * point.extend(1.0);
    if clip.w <= 0.05 {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    if ndc.x.abs() > 1.08 || ndc.y.abs() > 1.08 {
        return None;
    }
    let x = (ndc.x + 1.0) * 0.5 * width;
    let y = (1.0 - ndc.y) * 0.5 * height;
    Some((x, y, ndc.z))
}

fn pick<'a>(nodes: &'a [Placed], origin: Vec3, dir: Vec3) -> Option<&'a Placed> {
    let mut best: Option<(&Placed, f32)> = None;
    for node in nodes {
        if !node.pickable {
            continue;
        }
        let hit = if node.command.is_some() {
            ray_sector(origin, dir, node)
        } else {
            ray_aabb(origin, dir, node.center - node.half, node.center + node.half)
        };
        let Some(t) = hit else {
            continue;
        };
        if best.is_none_or(|(_, best_t)| t < best_t) {
            best = Some((node, t));
        }
    }
    best.map(|(node, _)| node)
}

fn aim_node(nodes: &[Placed], origin: Vec3, dir: Vec3) -> Option<Aim> {
    let node = pick(nodes, origin, dir)?;
    if let Some(index) = node.command {
        Some(Aim::Command {
            index,
            name: node.label.clone(),
        })
    } else {
        Some(Aim::Folder(node.path.clone()))
    }
}

/// The segment is an annular slice. A downward ray hits its top; a shallower one can hit the wall.
fn ray_sector(origin: Vec3, dir: Vec3, node: &Placed) -> Option<f32> {
    let y0 = node.center.y - node.half.y;
    let y1 = node.center.y + node.half.y;
    if dir.y.abs() > 1e-6 {
        let mut best: Option<f32> = None;
        for y in [y0, y1] {
            let t = (y - origin.y) / dir.y;
            if t < 0.0 {
                continue;
            }
            if point_in_sector(origin + dir * t, node) && best.is_none_or(|best_t| t < best_t) {
                best = Some(t);
            }
        }
        if let Some(t) = best {
            return Some(t);
        }
    }
    let (mut t, t1) = y_span(origin.y, dir.y, y0, y1)?;
    for _ in 0..80 {
        if t > t1 {
            break;
        }
        if point_in_sector(origin + dir * t, node) {
            return Some(t);
        }
        t += 0.03;
    }
    None
}

fn y_span(origin_y: f32, dir_y: f32, y0: f32, y1: f32) -> Option<(f32, f32)> {
    if dir_y.abs() < 1e-6 {
        if (y0..=y1).contains(&origin_y) {
            Some((0.0, 40.0))
        } else {
            None
        }
    } else {
        let mut t0 = (y0 - origin_y) / dir_y;
        let mut t1 = (y1 - origin_y) / dir_y;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        if t1 < 0.0 {
            None
        } else {
            Some((t0.max(0.0), t1))
        }
    }
}

fn point_in_sector(point: Vec3, node: &Placed) -> bool {
    let mid = (node.center.x * node.center.x + node.center.z * node.center.z).sqrt();
    let unit = (SEGMENT_INNER + SEGMENT_OUTER) * 0.5;
    if mid < 1e-4 {
        return false;
    }
    let pulse = mid / unit;
    let radial = (point.x * point.x + point.z * point.z).sqrt();
    if radial + 0.002 < pulse * SEGMENT_INNER || radial - 0.002 > pulse * SEGMENT_OUTER {
        return false;
    }
    let angle = point.z.atan2(point.x);
    angle_delta(node.arc[0], angle).abs() <= node.arc[1] * 0.5 + 0.02
}

fn angle_delta(from: f32, to: f32) -> f32 {
    let mut delta = (to - from + PI) % TAU;
    if delta < 0.0 {
        delta += TAU;
    }
    delta - PI
}

fn tile_view(top: f32, width: f32, job: JobView, here: &Path) -> TileView {
    let height = if job.finished { TILE_H + FINISHED_EXTRA } else { TILE_H };
    let rect = [TILE_X, top, width, height];
    let arrow = [rect[0] + 10.0, rect[1] + 8.0, 26.0, 26.0];
    let close = [rect[0] + rect[2] - 36.0, rect[1] + 8.0, 26.0, 26.0];
    let name_left = arrow[0] + arrow[2] + 8.0;
    let name_width = (close[0] - 8.0 - name_left).max(24.0);
    let line_width = (rect[2] - 24.0).max(24.0);
    let title = if job.dir == here {
        job.name
    } else {
        format!("{} · {}", job.name, display_name(&job.dir))
    };
    let tag = if job.finished {
        [
            rect[0] + 12.0,
            rect[1] + rect[3] - 8.0 - FINISHED_TAG_H,
            FINISHED_TAG_W,
            FINISHED_TAG_H,
        ]
    } else {
        [0.0; 4]
    };
    TileView {
        id: job.id,
        name: fit_text(&title, 15.0, name_width),
        lines: job.lines.into_iter().map(|line| fit_text(&line, 13.0, line_width)).collect(),
        running: job.running,
        finished: job.finished,
        rect,
        arrow,
        close,
        tag,
        arrow_hot: false,
        close_hot: false,
    }
}

fn aim_tile(x: f32, y: f32, tiles: &[TileView]) -> Option<Aim> {
    for tile in tiles.iter().rev() {
        if inside(x, y, tile.close) {
            return Some(Aim::Kill(tile.id));
        }
        if inside(x, y, tile.arrow) {
            return Some(Aim::Back(tile.id));
        }
        if inside(x, y, tile.rect) {
            return Some(Aim::Tile);
        }
    }
    None
}

fn inside(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x < rect[0] + rect[2] && y < rect[1] + rect[3]
}

fn fit_text(text: &str, size: f32, width: f32) -> String {
    let max = ((width / (size * 0.52)).floor() as usize).max(1);
    short_name(text, max)
}

fn ray_aabb(origin: Vec3, dir: Vec3, min: Vec3, max: Vec3) -> Option<f32> {
    let mut tmin = 0.0_f32;
    let mut tmax = f32::MAX;
    for axis in 0..3 {
        let start = origin[axis];
        let direction = dir[axis];
        let near = min[axis];
        let far = max[axis];
        if direction.abs() < 1e-8 {
            if start < near || start > far {
                return None;
            }
            continue;
        }
        let inv = 1.0 / direction;
        let mut t0 = (near - start) * inv;
        let mut t1 = (far - start) * inv;
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        tmin = tmin.max(t0);
        tmax = tmax.min(t1);
        if tmin > tmax {
            return None;
        }
    }
    if tmax < 0.0 {
        None
    } else if tmin >= 0.0 {
        Some(tmin)
    } else {
        Some(tmax)
    }
}

fn display_name(path: &Path) -> String {
    match path.file_name() {
        Some(name) => {
            let text = name.to_string_lossy();
            if text.is_empty() {
                path.display().to_string()
            } else {
                text.into_owned()
            }
        }
        None => {
            if path.as_os_str().is_empty() {
                ".".to_string()
            } else {
                "/".to_string()
            }
        }
    }
}

fn short_name(name: &str, max: usize) -> String {
    if name.chars().count() <= max {
        return name.to_string();
    }
    let head: String = name.chars().take(max.saturating_sub(1)).collect();
    format!("{head}…")
}

fn ellipsize_middle(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 4 {
        return text.to_string();
    }
    let keep = max_chars - 1;
    let head = keep / 2;
    let tail = keep - head;
    let left: String = chars[..head].iter().collect();
    let right: String = chars[chars.len() - tail..].iter().collect();
    format!("{left}…{right}")
}

fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn fnv(text: &str) -> u32 {
    let mut hash = 2_166_136_261_u32;
    for byte in text.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash
}

fn jitter(name: &str, rgb: [f32; 3]) -> [f32; 4] {
    let shift = (fnv(name) % 1000) as f32 / 1000.0 - 0.5;
    [
        (rgb[0] + shift * 0.1).clamp(0.0, 1.0),
        (rgb[1] + shift * 0.05).clamp(0.0, 1.0),
        (rgb[2] - shift * 0.04).clamp(0.0, 1.0),
        1.0,
    ]
}

fn spin_of(name: &str) -> f32 {
    (fnv(name) % 1000) as f32 / 1000.0 * 0.7
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mark::{self, NavMark, Shape};
    use std::fs;
    use std::path::Path;
    use std::time::{Duration, Instant};

    #[test]
    fn current_folder_name_sits_in_the_ring() {
        let view_proj = Camera::default().view_proj(16.0 / 10.0);
        let labels = place_labels(
            &[],
            Path::new("/tmp/navigator"),
            view_proj,
            1600.0,
            1000.0,
            &None,
        );
        let label = labels
            .iter()
            .find(|label| label.text == "navigator")
            .expect("current folder label");
        let (x, y, _) = project(
            Vec3::new(0.0, HERE_RING_Y + HERE_RING_HEIGHT, 0.0),
            view_proj,
            1600.0,
            1000.0,
        )
        .expect("ring center on screen");
        assert!((label.center_x - x).abs() < 0.01);
        assert!((label.top + label_body_center(label.size) - y).abs() < 0.01);
    }

    #[test]
    fn ellipsize_keeps_both_ends() {
        assert_eq!(ellipsize_middle("abcdef", 10), "abcdef");
        let long = ellipsize_middle("abcdefghijklmnopqrstuvwxyz", 11);
        assert_eq!(long.chars().count(), 11);
        assert!(long.contains('…'));
        assert!(long.starts_with('a'));
        assert!(long.ends_with('z'));
    }

    #[test]
    fn a_finished_tile_keeps_its_tag_at_the_bottom() {
        let here = PathBuf::from("/tmp/plaza");
        let live = tile_view(
            TILE_Y0,
            TILE_W,
            JobView {
                id: 1,
                name: "ping".into(),
                dir: here.clone(),
                lines: vec!["hello".into()],
                running: true,
                finished: false,
            },
            &here,
        );
        assert!(!live.finished);
        assert_eq!(live.tag, [0.0; 4]);
        assert!((live.rect[3] - TILE_H).abs() < 0.1);

        let done = tile_view(
            TILE_Y0,
            TILE_W,
            JobView {
                id: 1,
                name: "ping".into(),
                dir: here.clone(),
                lines: vec!["hello".into()],
                running: false,
                finished: true,
            },
            &here,
        );
        assert!(done.finished);
        assert!(done.rect[3] > TILE_H + 20.0);
        assert!(done.tag[1] >= done.rect[1] + TILE_H - 1.0);
        assert!(done.tag[1] + done.tag[3] <= done.rect[1] + done.rect[3] + 0.1);
        assert!(done.tag[0] >= done.rect[0]);
        assert!(done.tag[0] + done.tag[2] <= done.rect[0] + done.rect[2]);

        let next = tile_view(
            done.rect[1] + done.rect[3] + TILE_GAP,
            TILE_W,
            JobView {
                id: 2,
                name: "slow".into(),
                dir: here,
                lines: vec!["running".into()],
                running: true,
                finished: false,
            },
            Path::new("/tmp/plaza"),
        );
        assert!(!next.finished);
        assert!(next.rect[1] + 0.1 >= done.rect[1] + done.rect[3] + TILE_GAP);
    }

    #[test]
    fn ray_hits_the_stone_in_front_and_misses_empty_ground() {
        let path = PathBuf::from("/alpha");
        let node = stone(
            Kind::Child,
            path.clone(),
            "alpha".into(),
            Vec3::ZERO,
            0.5,
            1.0,
            [1.0, 1.0, 1.0, 1.0],
            0.0,
            0.0,
            Shape::Prism,
        );
        let nodes = [node];
        let hit = pick(&nodes, Vec3::new(0.1, 4.0, 0.1), Vec3::new(0.0, -1.0, 0.0));
        assert_eq!(hit.unwrap().path, path);
        let miss = pick(&nodes, Vec3::new(3.0, 4.0, 0.0), Vec3::new(0.0, -1.0, 0.0));
        assert!(miss.is_none());
    }

    #[test]
    fn arrange_puts_the_parent_behind_and_the_subfolder_beside_its_parent() {
        let parent = PathBuf::from("/home");
        let alpha = PathBuf::from("/home/work/alpha");
        let one = PathBuf::from("/home/work/alpha/one");
        let beta = PathBuf::from("/home/work/beta");
        let children = vec![
            ChildSnapFull {
                snap: ChildSnap {
                    name: "alpha".into(),
                    path: alpha.clone(),
                    symlink: false,
                    mark: NavMark {
                        color: Some([0.8, 0.2, 0.2, 1.0]),
                        shape: Shape::Cone,
                    },
                },
                appear: 1.0,
                scanning: false,
                child_count: 1,
                grands: vec![GrandSnap {
                    name: "one".into(),
                    path: one.clone(),
                    appear: 1.0,
                    mark: NavMark {
                        color: None,
                        shape: Shape::Crystal,
                    },
                }],
            },
            ChildSnapFull {
                snap: ChildSnap {
                    name: "beta".into(),
                    path: beta.clone(),
                    symlink: false,
                    mark: NavMark::default(),
                },
                appear: 1.0,
                scanning: false,
                child_count: 0,
                grands: Vec::new(),
            },
        ];
        let arranged = arrange_full(
            Some(&parent),
            NavMark {
                color: Some([0.2, 0.4, 0.8, 1.0]),
                shape: Shape::Box,
            },
            &children,
            0.0,
        );
        let parent_node = arranged.nodes.iter().find(|node| node.kind == Kind::Parent).unwrap();
        assert_eq!(parent_node.label, "↑ home");
        assert!(parent_node.center.z < -1.0);
        assert_eq!(parent_node.shape, Shape::Box);
        assert_eq!(parent_node.color, [0.2, 0.4, 0.8, 1.0]);
        let kids: Vec<_> = arranged.nodes.iter().filter(|node| node.kind == Kind::Child).collect();
        assert_eq!(kids.len(), 2);
        assert!(kids[0].center.distance(kids[1].center) > 1.0);
        let grand = arranged.nodes.iter().find(|node| node.kind == Kind::Grandchild).unwrap();
        assert_eq!(grand.path, one);
        assert_eq!(grand.shape, Shape::Crystal);
        let alpha_node = kids.iter().find(|node| node.path == alpha).unwrap();
        assert_eq!(alpha_node.shape, Shape::Cone);
        assert_eq!(alpha_node.color, [0.8, 0.2, 0.2, 1.0]);
        let beta_node = kids.iter().find(|node| node.path == beta).unwrap();
        assert_eq!(beta_node.shape, Shape::Prism);
        assert!(grand.center.distance(alpha_node.center) < 2.0);
        assert!(arranged.nodes.iter().all(|node| node.label != "notes.txt"));
    }

    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!("nav-scene-{}-{nanos}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn opening_a_folder_shows_subfolders_as_they_are_found_and_a_click_moves() {
        let tmp = Tmp::new();
        let base = tmp.0.join("coast");
        let root = base.join("harbor");
        fs::create_dir_all(root.join("atlas/europe")).unwrap();
        fs::create_dir_all(root.join("atlas/africa")).unwrap();
        fs::write(root.join("atlas/notes.txt"), b"nope").unwrap();
        fs::create_dir_all(root.join("beacon")).unwrap();
        fs::write(root.join("chart.txt"), b"nope").unwrap();
        std::os::unix::fs::symlink("atlas", root.join("link_atlas")).unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        let start = Instant::now();
        let mut saw_grand = false;
        while start.elapsed() < Duration::from_secs(5) {
            nav.poll();
            let arranged = nav.layout(0.0);
            let names: Vec<_> = arranged.nodes.iter().map(|node| (node.kind, node.label.clone())).collect();
            let labels: Vec<_> = names.iter().map(|(_, label)| label.as_str()).collect();
            assert!(!labels.contains(&"notes.txt"));
            assert!(!labels.contains(&"chart.txt"));
            if labels.contains(&"europe") && labels.contains(&"africa") && labels.contains(&"beacon") {
                assert!(names.iter().any(|(kind, label)| *kind == Kind::Grandchild && label == "europe"));
                assert!(names.iter().any(|(kind, label)| *kind == Kind::Parent && label == "↑ coast"));
                saw_grand = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(saw_grand, "subfolders should show up before the walk is done");
        assert!(!nav.dirs.contains_key(&base), "the parent is drawn, not scanned");
        let link = nav
            .dirs
            .get(&root)
            .unwrap()
            .children
            .iter()
            .find(|child| child.name == "link_atlas")
            .unwrap();
        assert!(link.symlink);
        assert_eq!(nav.dirs.get(&root.join("link_atlas")).unwrap().status, ScanStatus::Idle);

        nav.aim = Some(Aim::Folder(root.join("atlas")));
        assert!(matches!(nav.activate(), Click::Opened { up: false }));
        assert_eq!(nav.current, root.join("atlas"));

        let start = Instant::now();
        loop {
            nav.poll();
            let arranged = nav.layout(0.0);
            let kids: Vec<_> = arranged
                .nodes
                .iter()
                .filter(|node| node.kind == Kind::Child)
                .map(|node| node.label.clone())
                .collect();
            if kids.iter().any(|label| label == "europe") && kids.iter().any(|label| label == "africa") {
                assert!(!kids.iter().any(|label| label == "notes.txt"));
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("atlas should open onto its folders, saw {kids:?}");
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        nav.go_parent();
        assert_eq!(nav.current, root);
    }

    #[test]
    fn a_nav_file_sets_the_stone_before_it_is_drawn() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        let lantern = root.join("lantern");
        fs::create_dir_all(&lantern).unwrap();
        fs::create_dir_all(root.join("plain")).unwrap();
        fs::write(lantern.join(".nav"), "{\n  \"colour\": \"red\",\n  \"shape\": \"cone\"\n}\n").unwrap();
        fs::write(
            tmp.0.join(".nav"),
            "{\n  \"colour\": \"navy\",\n  \"shape\": \"obelisk\"\n}\n",
        )
        .unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        let start = Instant::now();
        let red = mark::parse_colour("red").unwrap();
        loop {
            nav.poll();
            let arranged = nav.layout(0.0);
            let tagged = arranged.nodes.iter().find(|node| node.label == "lantern");
            let plain = arranged.nodes.iter().find(|node| node.label == "plain");
            if let (Some(tagged), Some(plain)) = (tagged, plain) {
                assert_eq!(tagged.shape, Shape::Cone);
                assert_eq!(tagged.color, red);
                assert_eq!(plain.shape, Shape::Prism);
                assert_ne!(plain.color, red);
                let parent = arranged.nodes.iter().find(|node| node.kind == Kind::Parent).unwrap();
                assert_eq!(parent.shape, Shape::Obelisk);
                assert_eq!(parent.color, mark::parse_colour("navy").unwrap());
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("tagged folders should appear");
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        fs::write(lantern.join(".nav"), "{\n  \"colour\": \"gold\",\n  \"shape\": \"box\"\n}\n").unwrap();
        nav.rescan();
        let arranged = nav.layout(0.0);
        let tagged = arranged.nodes.iter().find(|node| node.label == "lantern").unwrap();
        assert_eq!(tagged.shape, Shape::Box);
        assert_eq!(tagged.color, mark::parse_colour("gold").unwrap());
    }

    #[test]
    fn manifest_commands_are_pickable_slices_of_the_center_ring() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"plaza\"\n\n[[bin]]\nname = \"serve\"\n\n[[example]]\nname = \"demo\"\n",
        )
        .unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"dev":"vite","test":"vitest"}}"#,
        )
        .unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        let arranged = nav.layout(0.0);
        let commands: Vec<_> = arranged.nodes.iter().filter(|node| node.kind == Kind::Command).collect();
        let labels: Vec<_> = commands.iter().map(|node| node.label.as_str()).collect();
        assert_eq!(labels, ["serve", "demo", "dev", "test"]);
        let mid = HERE_RING_SCALE * (SEGMENT_INNER + SEGMENT_OUTER) * 0.5;
        let height = HERE_RING_HEIGHT * 3.6;
        let sweep = TAU / 4.0 * 0.7;
        for node in &commands {
            let radial = (node.center.x * node.center.x + node.center.z * node.center.z).sqrt();
            assert!((radial - mid).abs() < 0.02, "radial {radial}");
            assert!((node.center.y - node.half.y - HERE_RING_Y).abs() < 0.001);
            assert!((node.half.y * 2.0 - height).abs() < 0.001);
            assert!(height > HERE_RING_HEIGHT);
            assert!((node.arc[1] - sweep).abs() < 0.001);
            assert!(node.pickable);
            assert_eq!(node.command, Some(commands.iter().position(|other| other.label == node.label).unwrap()));
            assert!(node.path.as_os_str().is_empty());
            assert_eq!(node.shape, Shape::Blob);
            assert_eq!(node.foot, 0.0);
        }
        let hit = pick(
            &arranged.nodes,
            Vec3::new(commands[0].center.x, 4.0, commands[0].center.z),
            Vec3::new(0.0, -1.0, 0.0),
        );
        assert_eq!(hit.map(|node| node.label.as_str()), Some("serve"));
        let gap = -FRAC_PI_2 + TAU / 8.0;
        let missed = pick(
            &arranged.nodes,
            Vec3::new(gap.cos() * mid, 4.0, gap.sin() * mid),
            Vec3::new(0.0, -1.0, 0.0),
        );
        assert!(missed.is_none(), "the pale ring between segments is not a command");

        fs::write(root.join("Cargo.toml"), "[package]\nname = \"plaza\"\n").unwrap();
        fs::write(root.join("package.json"), r#"{"scripts":{"build":"tsc"}}"#).unwrap();
        nav.rescan();
        let arranged = nav.layout(0.0);
        let labels: Vec<_> = arranged
            .nodes
            .iter()
            .filter(|node| node.kind == Kind::Command)
            .map(|node| node.label.clone())
            .collect();
        assert_eq!(labels, ["plaza", "build"]);
    }

    #[test]
    fn a_running_command_marks_only_its_own_segment() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        let side = root.join("side");
        fs::create_dir_all(&side).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"ping":"sleep 30","pong":"sleep 30"}}"#,
        )
        .unwrap();
        fs::write(side.join("package.json"), r#"{"scripts":{"ping":"sleep 30"}}"#).unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        let arranged = nav.layout(0.0);
        let ping = arranged.nodes.iter().find(|node| node.label == "ping").unwrap();
        let ray = Some((Vec3::new(ping.center.x, 5.0, ping.center.z), Vec3::new(0.0, -1.0, 0.0)));
        nav.build_frame(None, ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Acted));

        let arranged = nav.layout(0.0);
        let ping = arranged.nodes.iter().find(|node| node.label == "ping").unwrap();
        let pong = arranged.nodes.iter().find(|node| node.label == "pong").unwrap();
        assert!(ping.running);
        assert!(!pong.running);
        assert!(ping.half.y > pong.half.y + 0.004);
        assert!(ping.emissive > pong.emissive + 0.2);

        let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
        let mut glows: Vec<f32> = frame
            .nodes
            .iter()
            .filter(|node| node.shape == Shape::Blob)
            .map(|node| node.emissive)
            .collect();
        glows.sort_by(|a, b| a.total_cmp(b));
        assert!(glows.len() >= 2);
        assert!(glows[glows.len() - 1] > glows[0] + 0.2);

        nav.navigate(side.clone());
        let arranged = nav.layout(0.0);
        let ping = arranged.nodes.iter().find(|node| node.label == "ping").unwrap();
        assert!(!ping.running, "a command in another folder is a different task");

        nav.navigate(root);
        let arranged = nav.layout(0.0);
        assert!(arranged.nodes.iter().find(|node| node.label == "ping").unwrap().running);
        assert!(!arranged.nodes.iter().find(|node| node.label == "pong").unwrap().running);
    }

    #[test]
    fn clicking_a_command_runs_it_and_the_tile_can_return_and_close() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        let side = root.join("side");
        fs::create_dir_all(&side).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"ping":"echo hello-from-nav","pong":"echo again"}}"#,
        )
        .unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        let arranged = nav.layout(0.0);
        let ping = arranged.nodes.iter().find(|node| node.label == "ping").unwrap();
        let pong = arranged.nodes.iter().find(|node| node.label == "pong").unwrap();
        let ping_ray = Some((Vec3::new(ping.center.x, 5.0, ping.center.z), Vec3::new(0.0, -1.0, 0.0)));
        let pong_ray = Some((Vec3::new(pong.center.x, 5.0, pong.center.z), Vec3::new(0.0, -1.0, 0.0)));

        nav.build_frame(None, ping_ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Acted));
        assert_eq!(nav.current, root);

        let start = Instant::now();
        loop {
            nav.poll();
            let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
            let ready = frame.tiles.iter().any(|tile| {
                tile.name == "ping" && tile.lines.iter().any(|line| line.contains("hello-from-nav"))
            });
            if ready {
                break;
            }
            if start.elapsed() > Duration::from_secs(2) {
                let lines: Vec<_> = frame.tiles.iter().map(|tile| (tile.name.clone(), tile.lines.clone())).collect();
                panic!("ping should print: {lines:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
        let body = (frame.tiles[0].rect[0] + 48.0, frame.tiles[0].rect[1] + 56.0);
        nav.build_frame(Some(body), ping_ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Quiet));
        assert_eq!(nav.build_frame(None, None, 900.0, 700.0, 0.0).tiles.len(), 1);

        nav.build_frame(None, pong_ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Acted));
        let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
        assert_eq!(frame.tiles.len(), 2);
        assert_eq!(frame.tiles[0].name, "ping");
        assert_eq!(frame.tiles[1].name, "pong");
        assert!(frame.tiles[1].rect[1] > frame.tiles[0].rect[1] + frame.tiles[0].rect[3] - 1.0);
        assert!((frame.tiles[0].rect[0] - TILE_X).abs() < 0.1);

        nav.navigate(side);
        assert_ne!(nav.current, root);
        let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
        assert_eq!(frame.tiles[0].name, "ping · plaza");
        assert_eq!(frame.tiles[1].name, "pong · plaza");
        let arrow = frame.tiles[0].arrow;
        nav.build_frame(Some((arrow[0] + 4.0, arrow[1] + 4.0)), None, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Opened { up: false }));
        assert_eq!(nav.current, root);
        assert_eq!(nav.build_frame(None, None, 900.0, 700.0, 0.0).tiles[0].name, "ping");

        for _ in 0..4 {
            let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
            let Some(tile) = frame.tiles.first() else { break };
            let close = tile.close;
            nav.build_frame(Some((close[0] + 4.0, close[1] + 4.0)), None, 900.0, 700.0, 0.0);
            nav.activate();
        }
        assert!(nav.build_frame(None, None, 900.0, 700.0, 0.0).tiles.is_empty());
    }

    #[test]
    fn an_exited_command_shows_finished_on_its_tile() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("package.json"),
            r#"{"scripts":{"ping":"echo hello-from-nav","slow":"sleep 30"}}"#,
        )
        .unwrap();

        let mut nav = Navigator::open(root, false);
        let arranged = nav.layout(0.0);
        let ping = arranged.nodes.iter().find(|node| node.label == "ping").unwrap();
        let slow = arranged.nodes.iter().find(|node| node.label == "slow").unwrap();
        let ping_ray = Some((Vec3::new(ping.center.x, 5.0, ping.center.z), Vec3::new(0.0, -1.0, 0.0)));
        let slow_ray = Some((Vec3::new(slow.center.x, 5.0, slow.center.z), Vec3::new(0.0, -1.0, 0.0)));

        nav.build_frame(None, ping_ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Acted));
        nav.build_frame(None, slow_ray, 900.0, 700.0, 0.0);
        assert!(matches!(nav.activate(), Click::Acted));

        let start = Instant::now();
        let frame = loop {
            nav.poll();
            let frame = nav.build_frame(None, None, 900.0, 700.0, 0.0);
            if frame.tiles.iter().any(|tile| tile.name == "ping" && tile.finished) {
                break frame;
            }
            if start.elapsed() > Duration::from_secs(2) {
                let seen: Vec<_> = frame
                    .tiles
                    .iter()
                    .map(|tile| (tile.name.clone(), tile.finished, tile.lines.clone()))
                    .collect();
                panic!("ping should finish: {seen:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let ping = frame.tiles.iter().find(|tile| tile.name == "ping").unwrap();
        let slow = frame.tiles.iter().find(|tile| tile.name == "slow").unwrap();
        assert!(ping.tag[2] > 0.0);
        assert!(ping.tag[1] > ping.rect[1] + 70.0);
        assert!(ping.tag[1] + ping.tag[3] <= ping.rect[1] + ping.rect[3] + 0.1);
        assert!(!slow.finished);
        assert_eq!(slow.tag, [0.0; 4]);
        assert!(slow.rect[1] >= ping.rect[1] + ping.rect[3]);
    }

    #[test]
    fn h_hides_dot_directories_and_shows_them_again() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        fs::create_dir_all(root.join("plain/kept")).unwrap();
        fs::create_dir_all(root.join("plain/.secret")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        std::os::unix::fs::symlink("plain", root.join(".link")).unwrap();

        let mut nav = Navigator::open(root.clone(), false);
        assert!(!nav.show_dots);
        let start = Instant::now();
        loop {
            nav.poll();
            let root_done = nav
                .dirs
                .get(&root)
                .is_some_and(|rec| rec.status == ScanStatus::Done && rec.children.iter().any(|child| child.name == ".hidden"));
            let plain_done = nav.dirs.get(&root.join("plain")).is_some_and(|rec| {
                rec.status == ScanStatus::Done && rec.children.iter().any(|child| child.name == ".secret")
            });
            if root_done && plain_done {
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("the scan should list dot folders even while they are hidden");
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
        assert!(labels.iter().any(|label| label == "plain"));
        assert!(labels.iter().any(|label| label == "kept"));
        assert!(!labels.iter().any(|label| label == ".hidden" || label == ".secret" || label == ".link"));
        assert!(nav.build_frame(None, None, 800.0, 600.0, 0.0).meta.contains("dot folders hidden"));

        nav.toggle_dots();
        let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
        assert!(labels.iter().any(|label| label == ".hidden"));
        assert!(labels.iter().any(|label| label == ".secret"));
        assert!(labels.iter().any(|label| label == ".link"));
        assert!(!nav.build_frame(None, None, 800.0, 600.0, 0.0).meta.contains("dot folders hidden"));

        nav.toggle_dots();
        let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
        assert!(!labels.iter().any(|label| label == ".hidden" || label == ".secret" || label == ".link"));
        assert!(nav.build_frame(None, None, 800.0, 600.0, 0.0).meta.contains("dot folders hidden"));
    }
}
