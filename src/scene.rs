//! The plaza of folders, and the ray that picks one.

use std::collections::HashMap;
use std::f32::consts::{FRAC_PI_2, TAU};
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{Mat4, Quat, Vec3, Vec4};

use crate::commands::Command;
use crate::geom::{RING_INNER, RING_OUTER};
use crate::mark::{NavMark, Shape};
use crate::scan::{ScanEvent, Scanner};

const MAX_SHOWN: usize = 240;
const MAX_GRANDS: usize = 4;
const HELP: &str = "Drag to look around. Scroll to zoom. Click a folder to open it. Backspace goes up. H toggles dot folders. Escape exits.";

/// The "you are here" ring. The mesh is one unit tall, so the lip of the
/// opening is at `HERE_RING_Y + HERE_RING_HEIGHT`.
pub(crate) const HERE_RING_Y: f32 = 0.02;
pub(crate) const HERE_RING_HEIGHT: f32 = 0.045;
/// Instance scale of the unit ring. The pulse in the renderer adds a little on top.
pub(crate) const HERE_RING_SCALE: f32 = 1.62;

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
}

pub struct ScreenLabel {
    pub text: String,
    pub center_x: f32,
    pub top: f32,
    pub size: f32,
    pub color: [u8; 3],
}

pub struct Frame {
    pub nodes: Vec<NodeDraw>,
    pub ground_radius: f32,
    pub ring_emissive: f32,
    pub time: f32,
    pub labels: Vec<ScreenLabel>,
    pub title: String,
    pub meta: String,
    pub help: String,
    pub pointer: bool,
    pub view_proj: Mat4,
    pub camera_pos: Vec3,
}

pub struct Navigator {
    current: PathBuf,
    dirs: HashMap<PathBuf, DirRecord>,
    marks: HashMap<PathBuf, NavMark>,
    commands: HashMap<PathBuf, Vec<Command>>,
    scanner: Scanner,
    camera: Camera,
    user_zoomed: bool,
    hover: Option<PathBuf>,
    show_dots: bool,
}

impl Navigator {
    pub fn open(path: PathBuf) -> Self {
        let mut nav = Self {
            current: path.clone(),
            dirs: HashMap::new(),
            marks: HashMap::new(),
            commands: HashMap::new(),
            scanner: Scanner::spawn(),
            camera: Camera::default(),
            user_zoomed: false,
            hover: None,
            show_dots: true,
        };
        nav.request_scan(&path);
        nav
    }

    pub fn poll(&mut self) {
        let events = self.scanner.poll();
        for event in events {
            self.apply(event);
        }
    }

    pub fn navigate(&mut self, path: PathBuf) {
        if path == self.current || !path.is_dir() {
            return;
        }
        self.current = path;
        self.user_zoomed = false;
        self.hover = None;
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
        self.hover.as_deref()
    }

    pub fn hovered_is_parent(&self) -> bool {
        let Some(path) = &self.hover else {
            return false;
        };
        self.current.parent() == Some(path.as_path())
    }

    /// A stable 0 to 1 tone for the stone under the pointer.
    pub fn hover_pitch(&self) -> f32 {
        let Some(path) = &self.hover else {
            return 0.35;
        };
        (fnv(&path.to_string_lossy()) % 1000) as f32 / 999.0
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

    pub fn activate(&mut self) -> bool {
        let Some(path) = self.hover.clone() else {
            return false;
        };
        if path == self.current {
            return false;
        }
        let before = self.current.clone();
        self.navigate(path);
        self.current != before
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
        self.hover = ray.and_then(|(origin, dir)| {
            pick(&arranged.nodes, origin, dir).map(|node| node.path.clone())
        });
        if let Some(path) = self.hover.clone() {
            for node in &mut arranged.nodes {
                if node.pickable && node.path == path {
                    node.color = [0.98, 0.86, 0.55, 1.0];
                    node.emissive = 0.55;
                }
            }
        }
        let labels = place_labels(&arranged.nodes, &self.current, view_proj, width, height, &self.hover);
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
                })
                .collect(),
            ground_radius: arranged.outer + 8.0,
            ring_emissive: 0.3 + amp * (time * rate).sin(),
            time,
            labels,
            title,
            meta,
            help: HELP.to_string(),
            pointer: self.hover.is_some(),
            view_proj,
            camera_pos,
        }
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
                self.attach_commands(&mut arranged);
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
        self.attach_commands(&mut arranged);
        arranged
    }

    fn attach_commands(&mut self, arranged: &mut Arrangement) {
        let current = self.current.clone();
        let commands = self.commands_of(&current);
        place_commands(&mut arranged.nodes, &commands);
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
        if let Some(path) = &self.hover {
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
    }
}

fn place_commands(nodes: &mut Vec<Placed>, commands: &[Command]) {
    let count = commands.len();
    if count == 0 {
        return;
    }
    let mid = HERE_RING_SCALE * (RING_INNER + RING_OUTER) * 0.5;
    let gap = TAU * mid / count as f32;
    let diameter = (gap * 0.55).clamp(0.12, 0.28);
    let lip = HERE_RING_Y + HERE_RING_HEIGHT;
    for (index, command) in commands.iter().enumerate() {
        let angle = -FRAC_PI_2 + index as f32 * TAU / count as f32;
        let mut node = stone(
            Kind::Command,
            PathBuf::new(),
            short_name(&command.name, 20),
            Vec3::new(angle.cos() * mid, lip, angle.sin() * mid),
            diameter * 0.5,
            diameter,
            [0.95, 0.72, 0.28, 1.0],
            0.22,
            0.0,
            Shape::Blob,
        );
        node.pickable = false;
        nodes.push(node);
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
        let min = node.center - node.half;
        let max = node.center + node.half;
        let Some(t) = ray_aabb(origin, dir, min, max) else {
            continue;
        };
        if best.is_none_or(|(_, best_t)| t < best_t) {
            best = Some((node, t));
        }
    }
    best.map(|(node, _)| node)
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

        let mut nav = Navigator::open(root.clone());
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

        nav.hover = Some(root.join("atlas"));
        assert!(nav.activate());
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

        let mut nav = Navigator::open(root.clone());
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
    fn manifest_commands_sit_on_the_center_ring_and_are_not_clickable() {
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

        let mut nav = Navigator::open(root.clone());
        let arranged = nav.layout(0.0);
        let commands: Vec<_> = arranged.nodes.iter().filter(|node| node.kind == Kind::Command).collect();
        let labels: Vec<_> = commands.iter().map(|node| node.label.as_str()).collect();
        assert_eq!(labels, ["serve", "demo", "dev", "test"]);
        let mid = HERE_RING_SCALE * (RING_INNER + RING_OUTER) * 0.5;
        let lip = HERE_RING_Y + HERE_RING_HEIGHT;
        for node in &commands {
            let radial = (node.center.x * node.center.x + node.center.z * node.center.z).sqrt();
            assert!((radial - mid).abs() < 0.02, "radial {radial}");
            assert!((node.center.y - node.half.y - lip).abs() < 0.001);
            assert!(!node.pickable);
            assert!(node.path.as_os_str().is_empty());
            assert_eq!(node.shape, Shape::Blob);
            assert!((node.half.x - node.half.y).abs() < 0.001, "a blob stays round");
        }
        let hit = pick(
            &arranged.nodes,
            Vec3::new(commands[0].center.x, 4.0, commands[0].center.z),
            Vec3::new(0.0, -1.0, 0.0),
        );
        assert!(hit.is_none());

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
    fn h_hides_dot_directories_and_shows_them_again() {
        let tmp = Tmp::new();
        let root = tmp.0.join("plaza");
        fs::create_dir_all(root.join("plain/kept")).unwrap();
        fs::create_dir_all(root.join("plain/.secret")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        std::os::unix::fs::symlink("plain", root.join(".link")).unwrap();

        let mut nav = Navigator::open(root.clone());
        let start = Instant::now();
        loop {
            nav.poll();
            let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
            if labels.iter().any(|label| label == "plain")
                && labels.iter().any(|label| label == ".hidden")
                && labels.iter().any(|label| label == ".link")
                && labels.iter().any(|label| label == ".secret")
                && labels.iter().any(|label| label == "kept")
            {
                break;
            }
            if start.elapsed() > Duration::from_secs(5) {
                panic!("dot folders should be visible at first: {labels:?}");
            }
            std::thread::sleep(Duration::from_millis(15));
        }

        nav.toggle_dots();
        let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
        assert!(labels.iter().any(|label| label == "plain"));
        assert!(labels.iter().any(|label| label == "kept"));
        assert!(!labels.iter().any(|label| label == ".hidden" || label == ".secret" || label == ".link"));
        assert!(nav.build_frame(None, 800.0, 600.0, 0.0).meta.contains("dot folders hidden"));

        nav.toggle_dots();
        let labels: Vec<_> = nav.layout(0.0).nodes.iter().map(|node| node.label.clone()).collect();
        assert!(labels.iter().any(|label| label == ".hidden"));
        assert!(labels.iter().any(|label| label == ".secret"));
        assert!(labels.iter().any(|label| label == ".link"));
        assert!(!nav.build_frame(None, 800.0, 600.0, 0.0).meta.contains("dot folders hidden"));
    }
}
