//! Triangle meshes for the plaza. Winding is counter-clockwise from the outside.

use std::f32::consts::TAU;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u16>,
}

impl Mesh {
    fn push_tri(&mut self, a: [f32; 3], b: [f32; 3], c: [f32; 3], normal: [f32; 3]) {
        let base = self.vertices.len() as u16;
        for position in [a, b, c] {
            self.vertices.push(Vertex { position, normal });
        }
        self.indices.extend_from_slice(&[base, base + 1, base + 2]);
    }
}

fn cross_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ux = b[0] - a[0];
    let uy = b[1] - a[1];
    let uz = b[2] - a[2];
    let vx = c[0] - a[0];
    let vy = c[1] - a[1];
    let vz = c[2] - a[2];
    let nx = uy * vz - uz * vy;
    let ny = uz * vx - ux * vz;
    let nz = ux * vy - uy * vx;
    let len = (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
    [nx / len, ny / len, nz / len]
}

/// A tapered hexagonal stone. The base radius is 0.5 and the height is 1.
pub fn prism() -> Mesh {
    column(6, 0.34)
}

pub fn cylinder() -> Mesh {
    column(20, 0.47)
}

pub fn block() -> Mesh {
    column(4, 0.46)
}

pub fn cone() -> Mesh {
    column(20, 0.0)
}

pub fn pyramid() -> Mesh {
    column(4, 0.0)
}

pub fn obelisk() -> Mesh {
    column(4, 0.12)
}

/// Wide in the middle, pointed at the top. Base radius is still about 0.5.
pub fn crystal() -> Mesh {
    let foot = ring_points(6, 0.18, 0.0);
    let belt = ring_points(6, 0.5, 0.38);
    let apex = ring_points(6, 0.0, 1.0);
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    push_band(&mut mesh, &foot, &belt);
    push_band(&mut mesh, &belt, &apex);
    push_cap(&mut mesh, &foot, [0.0, -1.0, 0.0], true);
    mesh
}

/// Straight or tapered column. `top_radius` of 0 comes to a point. Base radius is 0.5 and height is 1.
fn column(sides: usize, top_radius: f32) -> Mesh {
    let bottom = ring_points(sides, 0.5, 0.0);
    let top = ring_points(sides, top_radius, 1.0);
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    push_band(&mut mesh, &bottom, &top);
    push_cap(&mut mesh, &bottom, [0.0, -1.0, 0.0], true);
    if top_radius > 0.001 {
        push_cap(&mut mesh, &top, [0.0, 1.0, 0.0], false);
    }
    mesh
}

fn ring_points(sides: usize, radius: f32, y: f32) -> Vec<[f32; 3]> {
    let sides = sides.max(3);
    let mut points = Vec::with_capacity(sides);
    for i in 0..sides {
        let angle = i as f32 / sides as f32 * TAU;
        points.push([radius * angle.cos(), y, radius * angle.sin()]);
    }
    points
}

fn push_band(mesh: &mut Mesh, lower: &[[f32; 3]], upper: &[[f32; 3]]) {
    let sides = lower.len();
    for i in 0..sides {
        let j = (i + 1) % sides;
        let collapsed = (upper[i][0] - upper[j][0]).abs() < 1e-5
            && (upper[i][2] - upper[j][2]).abs() < 1e-5;
        if collapsed {
            let normal = cross_normal(lower[i], upper[i], lower[j]);
            mesh.push_tri(lower[i], upper[i], lower[j], normal);
        } else {
            // Counter-clockwise when seen from outside the stone.
            let normal = cross_normal(lower[i], upper[j], lower[j]);
            mesh.push_tri(lower[i], upper[j], lower[j], normal);
            mesh.push_tri(lower[i], upper[i], upper[j], normal);
        }
    }
}

fn push_cap(mesh: &mut Mesh, ring: &[[f32; 3]], normal: [f32; 3], downward: bool) {
    let center = [0.0, ring[0][1], 0.0];
    for i in 0..ring.len() {
        let j = (i + 1) % ring.len();
        if downward {
            mesh.push_tri(center, ring[j], ring[i], normal);
        } else {
            mesh.push_tri(center, ring[i], ring[j], normal);
        }
    }
}

pub fn ground() -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let normal = [0.0, 1.0, 0.0];
    mesh.push_tri([-1.0, 0.0, -1.0], [1.0, 0.0, -1.0], [1.0, 0.0, 1.0], normal);
    mesh.push_tri([-1.0, 0.0, -1.0], [1.0, 0.0, 1.0], [-1.0, 0.0, 1.0], normal);
    mesh
}

pub fn disc(segments: usize) -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let normal = [0.0, 1.0, 0.0];
    let center = [0.0, 0.02, 0.0];
    let mut rim = Vec::with_capacity(segments);
    for i in 0..segments {
        let angle = i as f32 / segments as f32 * TAU;
        rim.push([angle.cos(), 0.02, angle.sin()]);
    }
    for i in 0..segments {
        let j = (i + 1) % segments;
        mesh.push_tri(center, rim[i], rim[j], normal);
    }
    mesh
}

/// Unit-ring radii. The plaza scales this mesh by `HERE_RING_SCALE`.
pub const RING_INNER: f32 = 0.72;
pub const RING_OUTER: f32 = 1.0;

/// A round stone. Radius 0.5 and height 1, so the usual instance scale stays a sphere.
pub fn sphere(stacks: usize, slices: usize) -> Mesh {
    let stacks = stacks.max(2);
    let slices = slices.max(3);
    let mut rings = Vec::with_capacity(stacks - 1);
    for stack in 1..stacks {
        let phi = stack as f32 / stacks as f32 * std::f32::consts::PI;
        let y = 0.5 + 0.5 * phi.cos();
        let radius = 0.5 * phi.sin();
        rings.push(ring_points(slices, radius, y));
    }
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let top = [0.0, 1.0, 0.0];
    let first = &rings[0];
    for i in 0..slices {
        let j = (i + 1) % slices;
        let normal = cross_normal(first[i], top, first[j]);
        mesh.push_tri(first[i], top, first[j], normal);
    }
    for band in rings.windows(2) {
        push_band(&mut mesh, &band[1], &band[0]);
    }
    let bottom = [0.0, 0.0, 0.0];
    let last = rings.last().expect("a sphere has a ring");
    for i in 0..slices {
        let j = (i + 1) % slices;
        let normal = cross_normal(bottom, last[i], last[j]);
        mesh.push_tri(bottom, last[i], last[j], normal);
    }
    mesh
}

/// Annulus used as the "you are here" ring. Outer radius is 1 and height is 1.
pub fn ring(segments: usize) -> Mesh {
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let inner = RING_INNER;
    let outer = RING_OUTER;
    for i in 0..segments {
        let a0 = i as f32 / segments as f32 * TAU;
        let a1 = (i + 1) as f32 / segments as f32 * TAU;
        let (c0, s0) = (a0.cos(), a0.sin());
        let (c1, s1) = (a1.cos(), a1.sin());
        let ob0 = [outer * c0, 0.0, outer * s0];
        let ob1 = [outer * c1, 0.0, outer * s1];
        let ot0 = [outer * c0, 1.0, outer * s0];
        let ot1 = [outer * c1, 1.0, outer * s1];
        let ib0 = [inner * c0, 0.0, inner * s0];
        let ib1 = [inner * c1, 0.0, inner * s1];
        let it0 = [inner * c0, 1.0, inner * s0];
        let it1 = [inner * c1, 1.0, inner * s1];
        let out_n = cross_normal(ob0, ob1, ot1);
        mesh.push_tri(ob0, ob1, ot1, out_n);
        mesh.push_tri(ob0, ot1, ot0, out_n);
        let in_n = cross_normal(ib1, ib0, it0);
        mesh.push_tri(ib1, ib0, it0, in_n);
        mesh.push_tri(ib1, it0, it1, in_n);
        let top_n = [0.0, 1.0, 0.0];
        mesh.push_tri(ot0, ot1, it1, top_n);
        mesh.push_tri(ot0, it1, it0, top_n);
        let bot_n = [0.0, -1.0, 0.0];
        mesh.push_tri(ob0, ib0, ib1, bot_n);
        mesh.push_tri(ob0, ib1, ob1, bot_n);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prism_sides_face_out_and_the_cap_faces_up() {
        let mesh = prism();
        assert!(!mesh.indices.is_empty());
        assert_eq!(mesh.indices.len() % 3, 0);
        let mut side_dots = 0.0_f32;
        let mut side_count = 0.0_f32;
        let mut cap_count = 0;
        for tri in mesh.indices.chunks(3) {
            let verts = [
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
            ];
            let normal = verts[0].normal;
            let len = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
            assert!((len - 1.0).abs() < 0.05, "normal {normal:?}");
            if normal[1] > 0.8 {
                cap_count += 1;
            }
            if normal[1].abs() < 0.6 {
                let px = (verts[0].position[0] + verts[1].position[0] + verts[2].position[0]) / 3.0;
                let pz = (verts[0].position[2] + verts[1].position[2] + verts[2].position[2]) / 3.0;
                side_dots += normal[0] * px + normal[2] * pz;
                side_count += 1.0;
            }
        }
        assert!(cap_count > 0);
        assert!(side_count > 0.0);
        assert!(side_dots / side_count > 0.0, "side faces should point outward");
    }

    #[test]
    fn every_stone_shape_faces_outward() {
        for (name, mesh) in [
            ("prism", prism()),
            ("cylinder", cylinder()),
            ("block", block()),
            ("cone", cone()),
            ("pyramid", pyramid()),
            ("crystal", crystal()),
            ("obelisk", obelisk()),
            ("sphere", sphere(8, 12)),
        ] {
            assert!(!mesh.indices.is_empty(), "{name}");
            let mut side_dots = 0.0_f32;
            let mut side_count = 0.0_f32;
            let mut upward = 0;
            let mut max_y = f32::MIN;
            for tri in mesh.indices.chunks(3) {
                let verts = [
                    mesh.vertices[tri[0] as usize],
                    mesh.vertices[tri[1] as usize],
                    mesh.vertices[tri[2] as usize],
                ];
                let normal = verts[0].normal;
                max_y = max_y.max(normal[1]);
                if normal[1] > 0.25 {
                    upward += 1;
                }
                if normal[1].abs() < 0.92 {
                    let px = (verts[0].position[0] + verts[1].position[0] + verts[2].position[0]) / 3.0;
                    let pz = (verts[0].position[2] + verts[1].position[2] + verts[2].position[2]) / 3.0;
                    if px * px + pz * pz > 1e-4 {
                        side_dots += normal[0] * px + normal[2] * pz;
                        side_count += 1.0;
                    }
                }
            }
            assert!(upward > 0, "{name} needs a face that points up, max y {max_y}");
            assert!(side_count > 0.0, "{name}");
            assert!(side_dots / side_count > 0.0, "{name} side faces should point outward");
        }
    }
}
