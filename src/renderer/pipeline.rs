// pipeline.rs — geometry render pipelines (Phase 2). Toolpath pipeline arrives in Phase 3.
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use image::RgbImage;
use std::sync::OnceLock;

use crate::renderer::lighting;

use crate::{
    cam::{
        operations::Operation,
        toolpath::{MoveKind, ToolMove},
    },
    workspace::{
        layer::Layer,
        model::Model,
        setup::{MachiningSetup, WorkAxis},
    },
};

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

static WOOD_DIFFUSE: OnceLock<RgbImage> = OnceLock::new();

fn wood_diffuse() -> &'static RgbImage {
    WOOD_DIFFUSE.get_or_init(|| {
        image::load_from_memory(include_bytes!(
            "../../assets/materials/fine_grained_wood_1k.jpg"
        ))
        .expect("bundled wood texture should decode")
        .to_rgb8()
    })
}

fn wood_color(u: f32, v: f32) -> [f32; 3] {
    let texture = wood_diffuse();
    let x = (u.rem_euclid(1.0) * texture.width() as f32) as u32;
    let y = ((1.0 - v).rem_euclid(1.0) * texture.height() as f32) as u32;
    let pixel = texture.get_pixel(x.min(texture.width() - 1), y.min(texture.height() - 1));
    // Display (sRGB) colour; `lighting::shade` does the linear-light maths.
    pixel.0.map(|channel| channel as f32 / 255.0)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StockMaterial {
    #[default]
    Wood,
    Aluminum,
    Steel,
}

pub struct SimulationProgress<'a> {
    pub moves: &'a [ToolMove],
    pub state: &'a crate::gcode::simulator::Simulator,
    /// Which operation's tool runs each part of `moves`.
    pub ranges: &'a [crate::gcode::simulator::SimulationRange],
}

impl StockMaterial {
    pub fn label(self) -> &'static str {
        match self {
            Self::Wood => "Wood",
            Self::Aluminum => "Aluminum",
            Self::Steel => "Steel",
        }
    }

    fn color(self) -> [f32; 3] {
        match self {
            Self::Wood => [0.66, 0.36, 0.16],
            Self::Aluminum => [0.58, 0.66, 0.72],
            Self::Steel => [0.38, 0.44, 0.51],
        }
    }
}

/// CPU-side snapshot of everything drawn by the geometry pass.
#[derive(Default)]
pub struct Scene {
    /// Changes whenever the contents change (used to trigger GPU re-upload)
    pub revision: u64,
    pub triangles: Vec<Vertex>,
    pub lines: Vec<Vertex>,
    pub toolpaths: Vec<Vertex>,
}

fn face_normal(points: [[f32; 3]; 3]) -> [f32; 3] {
    let u = [
        points[1][0] - points[0][0],
        points[1][1] - points[0][1],
        points[1][2] - points[0][2],
    ];
    let v = [
        points[2][0] - points[0][0],
        points[2][1] - points[0][1],
        points[2][2] - points[0][2],
    ];
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

/// A triangle lit per vertex: each corner has its own colour, normal and baked
/// `[ambient occlusion, shadow]` factors.
fn append_lit_triangle(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    colors: [[f32; 3]; 3],
    normals: [[f32; 3]; 3],
    factors: [[f32; 2]; 3],
    surface: lighting::Surface,
    orbit: [f32; 2],
) {
    let view = view_direction(orbit);
    for corner in 0..3 {
        let color = lighting::shade(
            colors[corner],
            normals[corner],
            view,
            surface,
            factors[corner][0],
            factors[corner][1],
        );
        scene.triangles.push(Vertex {
            pos: project_point(
                points[corner][0],
                points[corner][1],
                points[corner][2],
                true,
                orbit,
            ),
            color: [color[0], color[1], color[2], 1.0],
        });
    }
}

fn append_shaded_triangle(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    base: [f32; 3],
    orbit: [f32; 2],
) {
    append_shaded_triangle_with(scene, points, base, orbit, lighting::Surface::MATTE);
}

fn append_shaded_triangle_with(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    base: [f32; 3],
    orbit: [f32; 2],
    surface: lighting::Surface,
) {
    let normal = face_normal(points);
    append_lit_triangle(
        scene,
        points,
        [base; 3],
        [normal; 3],
        [[1.0, 1.0]; 3],
        surface,
        orbit,
    );
}

fn append_material_triangle(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    colors: [[f32; 3]; 3],
    orbit: [f32; 2],
) {
    let normal = face_normal(points);
    append_lit_triangle(
        scene,
        points,
        colors,
        [normal; 3],
        [[1.0, 1.0]; 3],
        lighting::Surface::WOOD,
        orbit,
    );
}

fn append_textured_face(scene: &mut Scene, quad: [[f32; 3]; 4], plane: usize, orbit: [f32; 2]) {
    const TEXTURE_WIDTH_MM: f32 = 600.0;
    let texture = wood_diffuse();
    let length = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let subdivisions = |world_length: f32| {
        ((world_length / TEXTURE_WIDTH_MM * texture.width() as f32) / 2.0)
            .ceil()
            .clamp(8.0, 256.0) as usize
    };
    let nx = subdivisions(length(quad[0], quad[1]));
    let ny = subdivisions(length(quad[0], quad[3]));
    let point = |u: f32, v: f32| {
        std::array::from_fn(|axis| {
            quad[0][axis] * (1.0 - u) * (1.0 - v)
                + quad[1][axis] * u * (1.0 - v)
                + quad[2][axis] * u * v
                + quad[3][axis] * (1.0 - u) * v
        })
    };
    let color = |point: [f32; 3]| {
        let (u, v) = match plane {
            0 => (point[1], point[2]),
            1 => (point[0], point[2]),
            _ => (point[0], point[1]),
        };
        wood_color(u / TEXTURE_WIDTH_MM, v / TEXTURE_WIDTH_MM)
    };
    for y in 0..ny {
        let v0 = y as f32 / ny as f32;
        let v1 = (y + 1) as f32 / ny as f32;
        for x in 0..nx {
            let u0 = x as f32 / nx as f32;
            let u1 = (x + 1) as f32 / nx as f32;
            let p00 = point(u0, v0);
            let p10 = point(u1, v0);
            let p11 = point(u1, v1);
            let p01 = point(u0, v1);
            append_material_triangle(
                scene,
                [p00, p10, p11],
                [color(p00), color(p10), color(p11)],
                orbit,
            );
            append_material_triangle(
                scene,
                [p00, p11, p01],
                [color(p00), color(p11), color(p01)],
                orbit,
            );
        }
    }
}

/// Split every triangle whose longest edge exceeds `limit` into four, up to
/// `depth` times.
fn split_large_triangles(triangles: &mut Vec<[[f32; 3]; 3]>, limit: f32, depth: usize) {
    for _ in 0..depth {
        let mut next = Vec::with_capacity(triangles.len());
        let mut changed = false;
        for t in triangles.iter() {
            let edge = |a: [f32; 3], b: [f32; 3]| {
                ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
            };
            let longest = edge(t[0], t[1]).max(edge(t[1], t[2])).max(edge(t[2], t[0]));
            if longest <= limit {
                next.push(*t);
                continue;
            }
            changed = true;
            let mid = |a: [f32; 3], b: [f32; 3]| {
                [
                    (a[0] + b[0]) * 0.5,
                    (a[1] + b[1]) * 0.5,
                    (a[2] + b[2]) * 0.5,
                ]
            };
            let (ab, bc, ca) = (mid(t[0], t[1]), mid(t[1], t[2]), mid(t[2], t[0]));
            next.extend([[t[0], ab, ca], [ab, t[1], bc], [ca, bc, t[2]], [ab, bc, ca]]);
        }
        *triangles = next;
        if !changed {
            break;
        }
    }
}

/// Matte plastic with a soft highlight.
const MODEL_SURFACE: lighting::Surface = lighting::Surface {
    specular: 0.28,
    shininess: 42.0,
};

/// A top-down height-field of the visible models (None when there are none).
fn model_height_field(models: &[Model]) -> Option<lighting::HeightField> {
    let mut low = [f32::INFINITY; 2];
    let mut high = [f32::NEG_INFINITY; 2];
    let mut any = false;
    for model in models.iter().filter(|model| model.visible) {
        let bounds = model.transformed_bounds();
        for axis in 0..2 {
            low[axis] = low[axis].min(bounds[0][axis]);
            high[axis] = high[axis].max(bounds[1][axis]);
        }
        any = true;
    }
    if !any || !low.iter().chain(high.iter()).all(|v| v.is_finite()) {
        return None;
    }
    let triangles = models
        .iter()
        .filter(|model| model.visible)
        .flat_map(|model| {
            // At most ~150k triangles are rasterised.
            let stride = model.triangles.len().div_ceil(150_000).max(1);
            model
                .triangles
                .iter()
                .step_by(stride)
                .filter_map(move |face| {
                    let a = model.transformed_vertex(face[0] as usize)?;
                    let b = model.transformed_vertex(face[1] as usize)?;
                    let c = model.transformed_vertex(face[2] as usize)?;
                    Some([a, b, c])
                })
        });
    Some(lighting::HeightField::from_triangles(
        triangles,
        (low, high),
        384,
    ))
}

/// Smooth vertex normals and baked occlusion/shadow factors for one model.
struct ModelLighting<'a> {
    field: Option<&'a lighting::HeightField>,
    normals: std::collections::HashMap<[i32; 3], Vec<[f32; 3]>>,
    occlusion_radius: f32,
    shadow_reach: f32,
}

impl<'a> ModelLighting<'a> {
    /// Faces meeting at an angle sharper than this stay creased.
    const CREASE_COS: f32 = 0.766; // 40 degrees

    fn new(model: &Model, field: Option<&'a lighting::HeightField>) -> Self {
        let mut normals: std::collections::HashMap<[i32; 3], Vec<[f32; 3]>> =
            std::collections::HashMap::new();
        for face in &model.triangles {
            let points: Option<Vec<[f32; 3]>> = face
                .iter()
                .map(|&index| model.transformed_vertex(index as usize))
                .collect();
            let Some(points) = points else { continue };
            let triangle = [points[0], points[1], points[2]];
            let normal = face_normal(triangle);
            for point in triangle {
                normals.entry(Self::key(point)).or_default().push(normal);
            }
        }
        let bounds = model.transformed_bounds();
        let extent = (0..3)
            .map(|axis| bounds[1][axis] - bounds[0][axis])
            .fold(1.0_f32, f32::max);
        Self {
            field,
            normals,
            occlusion_radius: (extent * 0.05).clamp(1.5, 8.0),
            shadow_reach: (extent * 0.6).clamp(6.0, 80.0),
        }
    }

    fn key(point: [f32; 3]) -> [i32; 3] {
        point.map(|value| (value * 50.0).round() as i32)
    }

    fn unit(v: [f32; 3]) -> [f32; 3] {
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-12);
        [v[0] / length, v[1] / length, v[2] / length]
    }

    /// Normals of a face's corners, averaged with the neighbouring faces that
    /// are not creased away.
    fn vertex_normals(&self, points: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
        let own = Self::unit(face_normal(points));
        points.map(|point| {
            let mut sum = [0.0_f32; 3];
            if let Some(neighbours) = self.normals.get(&Self::key(point)) {
                for neighbour in neighbours {
                    let unit = Self::unit(*neighbour);
                    let same_side = own[0] * unit[0] + own[1] * unit[1] + own[2] * unit[2];
                    if same_side >= Self::CREASE_COS {
                        // Larger faces count for more.
                        let weight =
                            (neighbour[0].powi(2) + neighbour[1].powi(2) + neighbour[2].powi(2))
                                .sqrt()
                                .max(1e-9);
                        for axis in 0..3 {
                            sum[axis] += unit[axis] * weight.sqrt();
                        }
                    }
                }
            }
            if sum == [0.0; 3] {
                own
            } else {
                Self::unit(sum)
            }
        })
    }

    /// `[ambient occlusion, shadow]` at a point of the model surface.
    fn factors(&self, point: [f32; 3]) -> [f32; 2] {
        let Some(field) = self.field else {
            return [1.0, 1.0];
        };
        [
            field.ambient_occlusion(point, self.occlusion_radius),
            field.shadow(point, lighting::key_light(), self.shadow_reach),
        ]
    }
}

impl Scene {
    pub fn build(
        layers: &[Layer],
        models: &[Model],
        operations: &[Operation],
        revision: u64,
        view_3d: bool,
        orbit: [f32; 2],
        selected_model: Option<usize>,
        selected_face_ids: &[u64],
        orbit_preview: bool,
        setup: &MachiningSetup,
        show_stock_removal: bool,
        render_stock: bool,
        stock_material: StockMaterial,
        simulation: Option<SimulationProgress<'_>>,
    ) -> Self {
        let mut scene = Scene {
            revision,
            ..Default::default()
        };
        if view_3d && render_stock {
            append_ground_shadow(&mut scene, setup, orbit);
            append_stock_solid(&mut scene, setup, stock_material, orbit, render_stock);
        } else if !view_3d {
            append_stock_top_surface(&mut scene, setup, stock_material);
        }
        for l in layers.iter().filter(|l| l.visible) {
            let color = l.color;
            let layer_z = if view_3d {
                (match setup.axis {
                    WorkAxis::ZUp => setup.stock_origin_mm[2] + setup.stock_size_mm[2],
                    WorkAxis::ZDown => setup.stock_origin_mm[2],
                }) as f32
            } else {
                0.0
            };
            scene
                .triangles
                .extend(l.geometry.triangles.iter().map(|p| Vertex {
                    pos: project_point(p[0], p[1], layer_z, view_3d, orbit),
                    color,
                }));
            scene.lines.extend(l.geometry.lines.iter().map(|p| Vertex {
                pos: project_point(p[0], p[1], layer_z, view_3d, orbit),
                color,
            }));
        }
        // One top-down height-field of every visible model, for occlusion and
        // shadows (skipped while the view is being dragged).
        let lighting_field = (view_3d && !orbit_preview)
            .then(|| model_height_field(models))
            .flatten();
        for (model_index, model) in models
            .iter()
            .enumerate()
            .filter(|(_, model)| view_3d && model.visible)
        {
            // During orbit drags, sample a bounded number of triangles and skip
            // the expensive painter sort. The complete sorted mesh is restored
            // on the first frame after the drag ends.
            let stride = if orbit_preview {
                (model.triangles.len() + 11_999) / 12_000
            } else {
                1
            }
            .max(1);
            let base = if selected_model == Some(model_index) {
                [0.95, 0.65, 0.28]
            } else {
                [0.55, 0.68, 0.82]
            };
            let mut triangle_face_ids = vec![None; model.triangles.len()];
            for body in &model.bodies {
                for offset in 0..body.triangle_count {
                    let triangle_index = body.first_triangle + offset;
                    if triangle_index < triangle_face_ids.len() {
                        triangle_face_ids[triangle_index] = Some(
                            body.triangle_face_ids
                                .get(offset)
                                .copied()
                                .unwrap_or(body.source_item_id),
                        );
                    }
                }
            }
            let model_extent = {
                let bounds = model.transformed_bounds();
                (0..3)
                    .map(|axis| bounds[1][axis] - bounds[0][axis])
                    .fold(0.0_f32, f32::max)
            };
            let mut faces = Vec::with_capacity(if orbit_preview {
                (model.triangles.len() + stride - 1) / stride
            } else {
                model.triangles.len()
            });
            for (triangle_index, face) in model.triangles.iter().enumerate().step_by(stride) {
                let points = face.map(|index| {
                    model
                        .transformed_vertex(index as usize)
                        .unwrap_or(model.vertices[index as usize])
                });
                if !is_front_facing(points, orbit) {
                    continue;
                }
                let selected = triangle_face_ids[triangle_index]
                    .is_some_and(|face_id| selected_face_ids.contains(&face_id));
                let color = if selected { [0.15, 0.95, 0.95] } else { base };
                // Big flat faces are split so they sort correctly against what stands
                // on them, and so occlusion and shadows can vary across them.
                let mut pieces = vec![points];
                if !orbit_preview && model_extent > 0.0 {
                    split_large_triangles(&mut pieces, model_extent * 0.1, 3);
                }
                for piece in pieces {
                    let depth = piece
                        .iter()
                        .map(|p| camera_point(*p, orbit)[2])
                        .sum::<f32>()
                        / 3.0;
                    faces.push((depth, piece, color));
                }
            }
            // camera_point() produces larger depth for points closer to the
            // viewer. Paint the far faces first so nearer faces cover them.
            // Sort the sampled drag preview too; otherwise source mesh order
            // lets back faces flash over the front while the camera orbits.
            faces.sort_by(|a, b| b.0.total_cmp(&a.0));
            if orbit_preview || faces.is_empty() {
                // Cheap flat shading while the view is being dragged.
                for (_, points, color) in faces {
                    append_shaded_triangle_with(&mut scene, points, color, orbit, MODEL_SURFACE);
                }
            } else {
                let lit = ModelLighting::new(model, lighting_field.as_ref());
                for (_, points, color) in faces {
                    let normals = lit.vertex_normals(points);
                    let factors = points.map(|p| lit.factors(p));
                    append_lit_triangle(
                        &mut scene,
                        points,
                        [color; 3],
                        normals,
                        factors,
                        MODEL_SURFACE,
                        orbit,
                    );
                }
            }
        }
        // Show the configured stock envelope in 3D so model placement and
        // generated toolpaths can be inspected against the actual setup.
        if view_3d {
            let min = setup.stock_origin_mm;
            let max = [
                min[0] + setup.stock_size_mm[0],
                min[1] + setup.stock_size_mm[1],
                match setup.axis {
                    WorkAxis::ZUp => min[2] + setup.stock_size_mm[2],
                    WorkAxis::ZDown => min[2] - setup.stock_size_mm[2],
                },
            ];
            let z_min = min[2].min(max[2]) as f32;
            let z_max = min[2].max(max[2]) as f32;
            let x_min = min[0] as f32;
            let x_max = max[0] as f32;
            let y_min = min[1] as f32;
            let y_max = max[1] as f32;
            let corners = [
                [x_min, y_min, z_min],
                [x_max, y_min, z_min],
                [x_max, y_max, z_min],
                [x_min, y_max, z_min],
                [x_min, y_min, z_max],
                [x_max, y_min, z_max],
                [x_max, y_max, z_max],
                [x_min, y_max, z_max],
            ];
            for (a, b) in [
                (0, 1),
                (1, 2),
                (2, 3),
                (3, 0),
                (4, 5),
                (5, 6),
                (6, 7),
                (7, 4),
                (0, 4),
                (1, 5),
                (2, 6),
                (3, 7),
            ] {
                let color = [0.45, 0.62, 0.78, 0.72];
                for corner in [corners[a], corners[b]] {
                    scene.lines.push(Vertex {
                        pos: project_point(corner[0], corner[1], corner[2], true, orbit),
                        color,
                    });
                }
            }
        }
        if view_3d && show_stock_removal && !orbit_preview {
            append_stock_removal_preview(
                &mut scene,
                operations,
                setup,
                orbit,
                stock_material,
                simulation,
            );
        }
        if view_3d {
            // The axis triad always sits at the centre of the stock's top face,
            // so it follows the stock when it is resized, moved or fitted to a model.
            let anchor = stock_top_center(setup);
            let extent = models
                .iter()
                .find(|model| model.visible)
                .map(|model| {
                    let bounds = model.transformed_bounds();
                    (0..3)
                        .map(|axis| (bounds[1][axis] - bounds[0][axis]).abs())
                        .fold(1.0_f32, f32::max)
                })
                .unwrap_or_else(|| {
                    setup
                        .stock_size_mm
                        .iter()
                        .fold(1.0_f32, |largest, size| largest.max(*size as f32))
                })
                * 0.6;
            for (direction, color) in [
                ([1.0_f32, 0.0, 0.0], [0.95, 0.3, 0.3, 1.0]),
                ([0.0, 1.0, 0.0], [0.35, 0.85, 0.4, 1.0]),
                ([0.0, 0.0, 1.0], [0.35, 0.6, 1.0, 1.0]),
            ] {
                scene.lines.push(Vertex {
                    pos: project_point(anchor[0], anchor[1], anchor[2], true, orbit),
                    color,
                });
                scene.lines.push(Vertex {
                    pos: project_point(
                        anchor[0] + direction[0] * extent,
                        anchor[1] + direction[1] * extent,
                        anchor[2] + direction[2] * extent,
                        true,
                        orbit,
                    ),
                    color,
                });
            }
        }
        for (operation_index, op) in operations
            .iter()
            .enumerate()
            .filter(|(_, op)| op.generated && !op.excluded_from_toolpath && op.visible)
        {
            // One colour per operation (as in PowerMill / Fusion 360), so every
            // operation can be told apart; rapids are a faint trace of it.
            let [red, green, blue] = crate::cam::operations::operation_color(operation_index);
            let colour = |kind| match kind {
                MoveKind::Rapid => [red, green, blue, 0.35],
                MoveKind::Feed | MoveKind::Arc | MoveKind::Drill => [red, green, blue, 1.0],
            };
            let mut previous: Option<[f32; 3]> = None;
            for mv in &op.toolpath.moves {
                let (end, z) = match mv {
                    ToolMove::Rapid { to, z }
                    | ToolMove::Feed { to, z, .. }
                    | ToolMove::Arc { to, z, .. } => (*to, *z),
                    ToolMove::Drill { at, depth, .. } => (*at, *depth),
                };
                let p = [end[0] as f32, end[1] as f32, z as f32];
                if let Some(start) = previous {
                    scene.toolpaths.push(Vertex {
                        pos: project_point(start[0], start[1], start[2], view_3d, orbit),
                        color: colour(mv.kind()),
                    });
                    scene.toolpaths.push(Vertex {
                        pos: project_point(p[0], p[1], p[2], view_3d, orbit),
                        color: colour(mv.kind()),
                    });
                }
                previous = Some(p);
            }
        }
        scene
    }

    pub fn bounds(&self) -> Option<([f32; 2], [f32; 2])> {
        self.triangles
            .iter()
            .chain(&self.lines)
            .chain(&self.toolpaths)
            .fold(None, |bounds, vertex| {
                Some(match bounds {
                    None => (vertex.pos, vertex.pos),
                    Some((min, max)) => (
                        [min[0].min(vertex.pos[0]), min[1].min(vertex.pos[1])],
                        [max[0].max(vertex.pos[0]), max[1].max(vertex.pos[1])],
                    ),
                })
            })
    }
}

/// Centre of the stock's top face in world coordinates.
fn stock_top_center(setup: &MachiningSetup) -> [f32; 3] {
    let origin = setup.stock_origin_mm;
    let size = setup.stock_size_mm;
    let top = match setup.axis {
        WorkAxis::ZUp => origin[2] + size[2],
        WorkAxis::ZDown => origin[2],
    };
    [
        (origin[0] + size[0] * 0.5) as f32,
        (origin[1] + size[1] * 0.5) as f32,
        top as f32,
    ]
}

fn append_stock_top_surface(scene: &mut Scene, setup: &MachiningSetup, material: StockMaterial) {
    let min = setup.stock_origin_mm;
    let max_x = min[0] + setup.stock_size_mm[0];
    let max_y = min[1] + setup.stock_size_mm[1];
    let corners = [
        [min[0] as f32, min[1] as f32],
        [max_x as f32, min[1] as f32],
        [max_x as f32, max_y as f32],
        [min[0] as f32, max_y as f32],
    ];
    let base = material.color();
    let fill = [base[0] * 0.72, base[1] * 0.72, base[2] * 0.72, 1.0];
    for triangle in [[0, 1, 2], [0, 2, 3]] {
        scene.triangles.extend(triangle.map(|index| Vertex {
            pos: corners[index],
            color: fill,
        }));
    }
    let edge = [0.82, 0.84, 0.9, 1.0];
    for (start, end) in [(0, 1), (1, 2), (2, 3), (3, 0)] {
        scene.lines.extend([
            Vertex {
                pos: corners[start],
                color: edge,
            },
            Vertex {
                pos: corners[end],
                color: edge,
            },
        ]);
    }
}

/// A soft contact shadow of the stock on the ground plane below it, thrown
/// away from the key light, so the part reads as standing on something.
fn append_ground_shadow(scene: &mut Scene, setup: &MachiningSetup, orbit: [f32; 2]) {
    let size = setup.stock_size_mm.map(|v| v as f32);
    let bottom = match setup.axis {
        WorkAxis::ZUp => setup.stock_origin_mm[2] as f32,
        WorkAxis::ZDown => (setup.stock_origin_mm[2] - setup.stock_size_mm[2]) as f32,
    };
    let z = bottom - 0.05;
    let light = lighting::key_light();
    let horizontal = light[0].hypot(light[1]).max(1e-3);
    // The shadow of a block of height h on the ground is displaced by h / tan(elevation).
    let throw = size[2] * horizontal / light[2].max(0.2) * 0.55;
    let offset = [
        -light[0] / horizontal * throw,
        -light[1] / horizontal * throw,
    ];
    let low = [
        setup.stock_origin_mm[0] as f32 + offset[0],
        setup.stock_origin_mm[1] as f32 + offset[1],
    ];
    let high = [low[0] + size[0], low[1] + size[1]];
    let blur = (size[0].max(size[1]) * 0.12).max(6.0);
    let reach = blur * 1.8;
    const N: usize = 24;
    let min = [low[0] - reach, low[1] - reach];
    let span = [size[0] + 2.0 * reach, size[1] + 2.0 * reach];
    let alpha_at = |x: f32, y: f32| {
        let dx = (low[0] - x).max(0.0).max(x - high[0]);
        let dy = (low[1] - y).max(0.0).max(y - high[1]);
        let distance = dx.hypot(dy);
        let t = (distance / reach).clamp(0.0, 1.0);
        let smooth = t * t * (3.0 - 2.0 * t);
        0.5 * (1.0 - smooth)
    };
    let vertex = |i: usize, j: usize| {
        let x = min[0] + span[0] * i as f32 / N as f32;
        let y = min[1] + span[1] * j as f32 / N as f32;
        Vertex {
            pos: project_point(x, y, z, true, orbit),
            color: [0.0, 0.0, 0.0, alpha_at(x, y)],
        }
    };
    for j in 0..N {
        for i in 0..N {
            let (a, b, c, d) = (
                vertex(i, j),
                vertex(i + 1, j),
                vertex(i + 1, j + 1),
                vertex(i, j + 1),
            );
            scene.triangles.extend([a, b, c, a, c, d]);
        }
    }
}

fn append_stock_solid(
    scene: &mut Scene,
    setup: &MachiningSetup,
    material: StockMaterial,
    orbit: [f32; 2],
    textured: bool,
) {
    let min = setup.stock_origin_mm;
    let max = [
        min[0] + setup.stock_size_mm[0],
        min[1] + setup.stock_size_mm[1],
        match setup.axis {
            WorkAxis::ZUp => min[2] + setup.stock_size_mm[2],
            WorkAxis::ZDown => min[2] - setup.stock_size_mm[2],
        },
    ];
    let p = [
        [min[0] as f32, min[1] as f32, min[2] as f32],
        [max[0] as f32, min[1] as f32, min[2] as f32],
        [max[0] as f32, max[1] as f32, min[2] as f32],
        [min[0] as f32, max[1] as f32, min[2] as f32],
        [min[0] as f32, min[1] as f32, max[2] as f32],
        [max[0] as f32, min[1] as f32, max[2] as f32],
        [max[0] as f32, max[1] as f32, max[2] as f32],
        [min[0] as f32, max[1] as f32, max[2] as f32],
    ];
    let faces = [
        [0, 2, 1],
        [0, 3, 2], // bottom
        [4, 5, 6],
        [4, 6, 7], // top
        [0, 1, 5],
        [0, 5, 4], // front
        [1, 2, 6],
        [1, 6, 5], // right
        [2, 3, 7],
        [2, 7, 6], // back
        [3, 0, 4],
        [3, 4, 7], // left
    ];
    let base = if textured {
        material.color()
    } else {
        [0.58, 0.62, 0.68]
    };
    let quads = [
        ([p[0], p[3], p[2], p[1]], 2), // bottom
        ([p[4], p[5], p[6], p[7]], 2), // top
        ([p[0], p[1], p[5], p[4]], 1), // front
        ([p[1], p[2], p[6], p[5]], 0), // right
        ([p[2], p[3], p[7], p[6]], 1), // back
        ([p[3], p[0], p[4], p[7]], 0), // left
    ];
    if textured && material == StockMaterial::Wood {
        let mut ordered = quads.to_vec();
        ordered.sort_by(|a, b| {
            let depth = |quad: &[[f32; 3]; 4]| {
                quad.iter()
                    .map(|point| camera_point(*point, orbit)[2])
                    .sum::<f32>()
                    / 4.0
            };
            depth(&b.0).total_cmp(&depth(&a.0))
        });
        for (quad, plane) in ordered {
            append_textured_face(scene, quad, plane, orbit);
        }
    } else {
        let mut triangles: Vec<_> = faces
            .into_iter()
            .map(|face| {
                let points = [p[face[0]], p[face[1]], p[face[2]]];
                let depth = points
                    .iter()
                    .map(|point| camera_point(*point, orbit)[2])
                    .sum::<f32>()
                    / 3.0;
                (depth, points)
            })
            .collect();
        triangles.sort_by(|a, b| b.0.total_cmp(&a.0));
        let surface = if textured && material != StockMaterial::Wood {
            lighting::Surface::METAL
        } else {
            lighting::Surface::MATTE
        };
        for (_, points) in triangles {
            append_shaded_triangle_with(scene, points, base, orbit, surface);
        }
        // Fine parallel highlights suggest brushed metal without adding visual clutter.
        let top_z = match setup.axis {
            WorkAxis::ZUp => max[2] as f32,
            WorkAxis::ZDown => min[2] as f32,
        };
        let z = top_z
            + if setup.axis == WorkAxis::ZUp {
                0.0005
            } else {
                -0.0005
            };
        for band in 0..20 {
            let fraction = (band as f32 + 0.5) / 20.0;
            let y = p[0][1] + (p[3][1] - p[0][1]) * fraction;
            scene.lines.extend([
                Vertex {
                    pos: project_point(p[0][0], y, z, true, orbit),
                    color: [0.9, 0.95, 1.0, 0.12],
                },
                Vertex {
                    pos: project_point(p[1][0], y, z, true, orbit),
                    color: [0.9, 0.95, 1.0, 0.12],
                },
            ]);
        }
    }
}

/// Approximate stock removal with a 64 by 64 sampled height field. Each cutting
/// move sweeps a circular cutter footprint over the grid; vertical walls and
/// cutter engagement between grid samples are not modeled.
fn append_stock_removal_preview(
    scene: &mut Scene,
    operations: &[Operation],
    setup: &MachiningSetup,
    orbit: [f32; 2],
    material: StockMaterial,
    simulation: Option<SimulationProgress<'_>>,
) {
    // Keep animated updates lighter, while using a sub-pixel surface grid for
    // the settled preview so small cutter radii appear smooth on screen.
    let cells = if simulation
        .as_ref()
        .is_some_and(|simulation| simulation.state.running)
    {
        256
    } else {
        512
    };
    let nx = cells;
    let ny = cells;
    let origin = setup.stock_origin_mm;
    let dx = setup.stock_size_mm[0] / nx as f64;
    let dy = setup.stock_size_mm[1] / ny as f64;
    let top = match setup.axis {
        WorkAxis::ZUp => origin[2] + setup.stock_size_mm[2],
        WorkAxis::ZDown => origin[2],
    };
    let floor = match setup.axis {
        WorkAxis::ZUp => origin[2],
        WorkAxis::ZDown => origin[2] - setup.stock_size_mm[2],
    };
    let mut heights = vec![top; (nx + 1) * (ny + 1)];

    let preview_paths: Vec<(Vec<ToolMove>, crate::cam::tool::Tool, [f64; 2])> =
        if let Some(simulation) = simulation {
            let index = simulation.state.move_index.min(simulation.moves.len());
            let mut moves = simulation.moves[..index].to_vec();
            if index < simulation.moves.len() && simulation.state.fraction > 0.0 {
                let at = simulation.state.position.unwrap_or([0.0, 0.0]);
                moves.push(partial_simulation_move(
                    &simulation.moves[index],
                    at,
                    simulation.state.z,
                ));
            }
            let origin = setup.work_origin_mm;
            moves = moves
                .into_iter()
                .map(|movement| move_to_world(movement, origin))
                .collect();
            // Split the executed moves between the operations that made them, so
            // each part is cut with its own tool.
            let fallback_tool = operations
                .iter()
                .find(|operation| operation.generated)
                .map(|operation| operation.tool.clone())
                .unwrap_or_default();
            if simulation.ranges.is_empty() {
                vec![(moves, fallback_tool, [origin[0], origin[1]])]
            } else {
                let mut groups = Vec::new();
                let mut start = 0;
                for range in simulation.ranges {
                    let end = range.end.min(moves.len());
                    if start < end {
                        groups.push((
                            moves[start..end].to_vec(),
                            range.tool.clone(),
                            [origin[0], origin[1]],
                        ));
                    }
                    start = start.max(end);
                }
                groups
            }
        } else {
            operations
                .iter()
                .filter(|operation| {
                    operation.generated && !operation.excluded_from_toolpath && operation.visible
                })
                .map(|operation| {
                    (
                        operation.toolpath.moves.clone(),
                        operation.tool.clone(),
                        [0.0, 0.0],
                    )
                })
                .collect()
        };

    for (moves, cutter, start_xy) in preview_paths {
        let radius = cutter.diameter_mm * 0.5;
        let mut current = [start_xy[0], start_xy[1], top];
        // Every cut of this path is collected and swept in one batch, on the GPU
        // when there is one.
        let mut cuts: Vec<crate::accel::Cut> = Vec::with_capacity(moves.len());
        let mut cut = |from: [f64; 3], to: [f64; 3]| {
            cuts.push([from.map(|v| v as f32), to.map(|v| v as f32)]);
        };
        for movement in &moves {
            match movement {
                ToolMove::Rapid { to, z } => current = [to[0], to[1], *z],
                ToolMove::Feed { to, z, .. } => {
                    cut(current, [to[0], to[1], *z]);
                    current = [to[0], to[1], *z];
                }
                ToolMove::Arc {
                    to,
                    center_offset,
                    z,
                    clockwise,
                    ..
                } => {
                    let center = [current[0] + center_offset[0], current[1] + center_offset[1]];
                    let start_angle = (current[1] - center[1]).atan2(current[0] - center[0]);
                    let end_angle = (to[1] - center[1]).atan2(to[0] - center[0]);
                    let sweep = arc_sweep(start_angle, end_angle, *clockwise);
                    let radius_arc = (current[0] - center[0]).hypot(current[1] - center[1]);
                    let samples = ((sweep.abs() / 5_f64.to_radians()).ceil() as usize).max(1);
                    let mut previous = current;
                    for sample in 1..=samples {
                        let t = sample as f64 / samples as f64;
                        let angle = start_angle + sweep * t;
                        let next = [
                            center[0] + radius_arc * angle.cos(),
                            center[1] + radius_arc * angle.sin(),
                            current[2] + (z - current[2]) * t,
                        ];
                        cut(previous, next);
                        previous = next;
                    }
                    current = [to[0], to[1], *z];
                }
                ToolMove::Drill { at, depth, .. } => {
                    cut([at[0], at[1], top], [at[0], at[1], *depth]);
                    current = [at[0], at[1], *depth];
                }
            }
        }
        let spec = crate::accel::GridSpec {
            origin: [origin[0], origin[1]],
            cell: [dx, dy],
            columns: nx + 1,
            rows: ny + 1,
            floor: floor as f32,
        };
        let mut swept: Vec<f32> = heights.iter().map(|height| *height as f32).collect();
        crate::accel::cut_capsules(
            &spec,
            &mut swept,
            &cuts,
            crate::accel::CutTool::from_tool(&cutter),
        );
        for (height, value) in heights.iter_mut().zip(swept) {
            *height = value as f64;
        }
    }

    let point = |ix: usize, iy: usize| {
        [
            (origin[0] + ix as f64 * dx) as f32,
            (origin[1] + iy as f64 * dy) as f32,
            heights[iy * (nx + 1) + ix] as f32,
        ]
    };
    let base = material.color();
    let surface = match material {
        StockMaterial::Wood => lighting::Surface::WOOD,
        _ => lighting::Surface::METAL,
    };
    // Occlusion and shadow baked from the cut surface itself, so pockets get
    // darker corners and walls throw soft shadows. Half resolution at the
    // settled 512 grid keeps rebuilds quick.
    let node_heights: Vec<f32> = heights.iter().map(|h| *h as f32).collect();
    let field = lighting::HeightField::from_grid(
        [origin[0] as f32, origin[1] as f32],
        [dx as f32, dy as f32],
        nx + 1,
        ny + 1,
        node_heights.clone(),
    );
    let step = if nx > 256 { 2 } else { 1 };
    let size_mm = setup.stock_size_mm[0].max(setup.stock_size_mm[1]) as f32;
    let occlusion_radius = (size_mm * 0.03).clamp(2.0, 8.0);
    let shadow_reach = (size_mm * 0.25).clamp(8.0, 60.0);
    let coarse_cols = nx / step + 2;
    let mut baked = vec![[1.0_f32, 1.0]; coarse_cols * (ny / step + 2)];
    for cy in 0..=(ny / step) {
        for cx in 0..=(nx / step) {
            let (ix, iy) = ((cx * step).min(nx), (cy * step).min(ny));
            let p = [
                (origin[0] + ix as f64 * dx) as f32,
                (origin[1] + iy as f64 * dy) as f32,
                node_heights[iy * (nx + 1) + ix],
            ];
            baked[cy * coarse_cols + cx] = [
                field.ambient_occlusion(p, occlusion_radius),
                field.shadow(p, lighting::key_light(), shadow_reach),
            ];
        }
    }
    let factors = |ix: usize, iy: usize| baked[(iy / step) * coarse_cols + ix / step];
    let normal_at = |ix: usize, iy: usize| {
        let at = |x: usize, y: usize| node_heights[y.min(ny) * (nx + 1) + x.min(nx)];
        let (x0, x1) = (ix.saturating_sub(1), (ix + 1).min(nx));
        let (y0, y1) = (iy.saturating_sub(1), (iy + 1).min(ny));
        let gx = (at(x1, iy) - at(x0, iy)) / ((x1 - x0).max(1) as f32 * dx as f32);
        let gy = (at(ix, y1) - at(ix, y0)) / ((y1 - y0).max(1) as f32 * dy as f32);
        [-gx, -gy, 1.0]
    };
    for iy in 0..ny {
        for ix in 0..nx {
            let a = point(ix, iy);
            let b = point(ix + 1, iy);
            let c = point(ix + 1, iy + 1);
            let d = point(ix, iy + 1);
            let corner_normals = [
                normal_at(ix, iy),
                normal_at(ix + 1, iy),
                normal_at(ix + 1, iy + 1),
                normal_at(ix, iy + 1),
            ];
            let corner_factors = [
                factors(ix, iy),
                factors(ix + 1, iy),
                factors(ix + 1, iy + 1),
                factors(ix, iy + 1),
            ];
            let removed =
                (top - (a[2] as f64 + b[2] as f64 + c[2] as f64 + d[2] as f64) * 0.25).max(0.0);
            let cut_tint = match material {
                StockMaterial::Wood => [0.9, 0.61, 0.31],
                StockMaterial::Aluminum => [0.79, 0.87, 0.91],
                StockMaterial::Steel => [0.61, 0.69, 0.76],
            };
            let blend =
                (removed as f32 / (setup.stock_size_mm[2] as f32 * 0.08).max(0.1)).clamp(0.0, 0.32);
            let cut_base = std::array::from_fn(|channel| {
                base[channel] * (1.0 - blend) + cut_tint[channel] * blend
            });
            if material == StockMaterial::Wood {
                let texture_color = |point: [f32; 3]| {
                    let albedo = wood_color(point[0] / 600.0, point[1] / 600.0);
                    std::array::from_fn(|channel| {
                        albedo[channel] * (1.0 - blend) + cut_tint[channel] * blend
                    })
                };
                append_lit_triangle(
                    scene,
                    [a, b, c],
                    [texture_color(a), texture_color(b), texture_color(c)],
                    [corner_normals[0], corner_normals[1], corner_normals[2]],
                    [corner_factors[0], corner_factors[1], corner_factors[2]],
                    surface,
                    orbit,
                );
                append_lit_triangle(
                    scene,
                    [a, c, d],
                    [texture_color(a), texture_color(c), texture_color(d)],
                    [corner_normals[0], corner_normals[2], corner_normals[3]],
                    [corner_factors[0], corner_factors[2], corner_factors[3]],
                    surface,
                    orbit,
                );
            } else {
                append_lit_triangle(
                    scene,
                    [a, b, c],
                    [cut_base; 3],
                    [corner_normals[0], corner_normals[1], corner_normals[2]],
                    [corner_factors[0], corner_factors[1], corner_factors[2]],
                    surface,
                    orbit,
                );
                append_lit_triangle(
                    scene,
                    [a, c, d],
                    [cut_base; 3],
                    [corner_normals[0], corner_normals[2], corner_normals[3]],
                    [corner_factors[0], corner_factors[2], corner_factors[3]],
                    surface,
                    orbit,
                );
            }
        }
    }
}

fn partial_simulation_move(movement: &ToolMove, at: [f64; 2], z: f64) -> ToolMove {
    match movement {
        ToolMove::Rapid { .. } => ToolMove::Rapid { to: at, z },
        ToolMove::Feed { feed_mm_min, .. } => ToolMove::Feed {
            to: at,
            z,
            feed_mm_min: *feed_mm_min,
        },
        ToolMove::Arc {
            center_offset,
            clockwise,
            feed_mm_min,
            ..
        } => ToolMove::Arc {
            to: at,
            center_offset: *center_offset,
            z,
            clockwise: *clockwise,
            feed_mm_min: *feed_mm_min,
        },
        ToolMove::Drill {
            retract,
            plunge_mm_min,
            ..
        } => ToolMove::Drill {
            at,
            depth: z,
            retract: *retract,
            plunge_mm_min: *plunge_mm_min,
        },
    }
}

fn move_to_world(movement: ToolMove, origin: [f64; 3]) -> ToolMove {
    match movement {
        ToolMove::Rapid { to, z } => ToolMove::Rapid {
            to: [to[0] + origin[0], to[1] + origin[1]],
            z: z + origin[2],
        },
        ToolMove::Feed { to, z, feed_mm_min } => ToolMove::Feed {
            to: [to[0] + origin[0], to[1] + origin[1]],
            z: z + origin[2],
            feed_mm_min,
        },
        ToolMove::Arc {
            to,
            center_offset,
            z,
            clockwise,
            feed_mm_min,
        } => ToolMove::Arc {
            to: [to[0] + origin[0], to[1] + origin[1]],
            center_offset,
            z: z + origin[2],
            clockwise,
            feed_mm_min,
        },
        ToolMove::Drill {
            at,
            depth,
            retract,
            plunge_mm_min,
        } => ToolMove::Drill {
            at: [at[0] + origin[0], at[1] + origin[1]],
            depth: depth + origin[2],
            retract: retract + origin[2],
            plunge_mm_min,
        },
    }
}

fn arc_sweep(start: f64, end: f64, clockwise: bool) -> f64 {
    let mut sweep = end - start;
    if clockwise {
        while sweep >= 0.0 {
            sweep -= std::f64::consts::TAU;
        }
    } else {
        while sweep <= 0.0 {
            sweep += std::f64::consts::TAU;
        }
    }
    sweep
}

pub fn projected_model_bounds(model: &Model, orbit: [f32; 2]) -> ([f32; 2], [f32; 2]) {
    let mut min = [f32::INFINITY; 2];
    let mut max = [f32::NEG_INFINITY; 2];
    let bounds = model.transformed_bounds();
    for x in [bounds[0][0], bounds[1][0]] {
        for y in [bounds[0][1], bounds[1][1]] {
            for z in [bounds[0][2], bounds[1][2]] {
                let point = project_point(x, y, z, true, orbit);
                for axis in 0..2 {
                    min[axis] = min[axis].min(point[axis]);
                    max[axis] = max[axis].max(point[axis]);
                }
            }
        }
    }
    (min, max)
}

fn project_point(x: f32, y: f32, z: f32, view_3d: bool, orbit: [f32; 2]) -> [f32; 2] {
    if view_3d {
        let camera = camera_point([x, y, z], orbit);
        [camera[0], camera[1]]
    } else {
        [x, y]
    }
}

/// Project a 3D world position into the same orthographic coordinates used by
/// the viewport scene, for egui overlays such as the simulated cutter marker.
pub fn project_view_point(point: [f32; 3], orbit: [f32; 2]) -> [f32; 2] {
    project_point(point[0], point[1], point[2], true, orbit)
}

pub fn view_depth(point: [f32; 3], orbit: [f32; 2]) -> f32 {
    camera_point(point, orbit)[2]
}

pub fn view_triangle_is_front_facing(points: [[f32; 3]; 3], orbit: [f32; 2]) -> bool {
    is_front_facing(points, orbit)
}

fn camera_point(point: [f32; 3], orbit: [f32; 2]) -> [f32; 3] {
    let yaw = std::f32::consts::FRAC_PI_4 + orbit[0];
    let pitch = (35.264_f32.to_radians() + orbit[1]).clamp(-1.52, 1.52);
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let x = point[0] * cy - point[1] * sy;
    let y = point[0] * sy + point[1] * cy;
    [x, y * sp + point[2] * cp, y * cp - point[2] * sp]
}

/// Unit vector, in world coordinates, from the scene toward the viewer.
///
/// `camera_point`'s Z grows *away* from the viewer (a face is front-facing when
/// its camera-space normal Z is negative), so the direction toward the viewer
/// is the negative gradient of that Z.
pub fn view_direction(orbit: [f32; 2]) -> [f32; 3] {
    let yaw = std::f32::consts::FRAC_PI_4 + orbit[0];
    let pitch = (35.264_f32.to_radians() + orbit[1]).clamp(-1.52, 1.52);
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    [-sy * cp, -cy * cp, sp]
}

fn is_front_facing(points: [[f32; 3]; 3], orbit: [f32; 2]) -> bool {
    let u = [
        points[1][0] - points[0][0],
        points[1][1] - points[0][1],
        points[1][2] - points[0][2],
    ];
    let v = [
        points[2][0] - points[0][0],
        points[2][1] - points[0][1],
        points[2][2] - points[0][2],
    ];
    let normal = [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ];
    let yaw = std::f32::consts::FRAC_PI_4 + orbit[0];
    let pitch = (35.264_f32.to_radians() + orbit[1]).clamp(-1.52, 1.52);
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let camera_normal_z = normal[0] * sy * cp + normal[1] * cy * cp - normal[2] * sp;
    let magnitude_squared = normal
        .iter()
        .map(|component| component * component)
        .sum::<f32>();
    magnitude_squared <= 1e-16 || camera_normal_z < 0.0
}

pub struct GeometryRenderer {
    tri_pipeline: wgpu::RenderPipeline,
    line_pipeline: wgpu::RenderPipeline,
    uniform_buffer: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    tri_buffer: Option<wgpu::Buffer>,
    tri_count: u32,
    line_buffer: Option<wgpu::Buffer>,
    line_count: u32,
    toolpath_buffer: Option<wgpu::Buffer>,
    toolpath_count: u32,
    revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isometric_projection_includes_tool_depth() {
        let surface = project_point(4.0, 2.0, 0.0, true, [0.0, 0.0]);
        let cut = project_point(4.0, 2.0, -3.0, true, [0.0, 0.0]);
        assert_eq!(surface[0], cut[0]);
        // Negative CNC Z is below the work plane and projects downward in the view.
        assert!(cut[1] < surface[1]);
        assert_eq!(project_point(4.0, 2.0, -3.0, false, [0.0, 0.0]), [4.0, 2.0]);
    }

    /// Software-rasterise a scene (painter's order, alpha blended) to a PNG, so
    /// the look of the 3D view can be inspected without a window.
    fn rasterise(scene: &Scene, path: &str) {
        let (width, height) = (1000u32, 760u32);
        let mut low = [f32::INFINITY; 2];
        let mut high = [f32::NEG_INFINITY; 2];
        for vertex in &scene.triangles {
            for axis in 0..2 {
                low[axis] = low[axis].min(vertex.pos[axis]);
                high[axis] = high[axis].max(vertex.pos[axis]);
            }
        }
        let scale = ((width as f32 - 40.0) / (high[0] - low[0]))
            .min((height as f32 - 40.0) / (high[1] - low[1]));
        let to_pixel = |pos: [f32; 2]| {
            [
                20.0 + (pos[0] - low[0]) * scale,
                height as f32 - 20.0 - (pos[1] - low[1]) * scale,
            ]
        };
        let mut image = image::RgbImage::from_pixel(width, height, image::Rgb([34, 36, 44]));
        for triangle in scene.triangles.chunks_exact(3) {
            let points = [
                to_pixel(triangle[0].pos),
                to_pixel(triangle[1].pos),
                to_pixel(triangle[2].pos),
            ];
            let min_x = points
                .iter()
                .map(|p| p[0])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as u32;
            let max_x = points
                .iter()
                .map(|p| p[0])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(width as f32 - 1.0) as u32;
            let min_y = points
                .iter()
                .map(|p| p[1])
                .fold(f32::INFINITY, f32::min)
                .floor()
                .max(0.0) as u32;
            let max_y = points
                .iter()
                .map(|p| p[1])
                .fold(f32::NEG_INFINITY, f32::max)
                .ceil()
                .min(height as f32 - 1.0) as u32;
            let denominator = (points[1][1] - points[2][1]) * (points[0][0] - points[2][0])
                + (points[2][0] - points[1][0]) * (points[0][1] - points[2][1]);
            if denominator.abs() < 1e-9 {
                continue;
            }
            for y in min_y..=max_y {
                for x in min_x..=max_x {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let a = ((points[1][1] - points[2][1]) * (px - points[2][0])
                        + (points[2][0] - points[1][0]) * (py - points[2][1]))
                        / denominator;
                    let b = ((points[2][1] - points[0][1]) * (px - points[2][0])
                        + (points[0][0] - points[2][0]) * (py - points[2][1]))
                        / denominator;
                    let c = 1.0 - a - b;
                    if a < -0.001 || b < -0.001 || c < -0.001 {
                        continue;
                    }
                    let mix = |channel: usize| {
                        a * triangle[0].color[channel]
                            + b * triangle[1].color[channel]
                            + c * triangle[2].color[channel]
                    };
                    let alpha = mix(3).clamp(0.0, 1.0);
                    let pixel = image.get_pixel_mut(x, y);
                    for channel in 0..3 {
                        let source = mix(channel).clamp(0.0, 1.0) * 255.0;
                        pixel.0[channel] =
                            (pixel.0[channel] as f32 * (1.0 - alpha) + source * alpha) as u8;
                    }
                }
            }
        }
        image.save(path).expect("write png");
    }

    /// `TEKKCAM_RENDER_OUT=<dir> cargo test --lib render_debug_images -- --ignored`
    #[test]
    #[ignore]
    fn render_debug_images() {
        use crate::cam::{adaptive, operations::Operation};
        use crate::workspace::model::{MeshBody, StepLengthUnit};
        let out = std::env::var("TEKKCAM_RENDER_OUT").unwrap_or_else(|_| ".".into());
        // Two stacked blocks (a stepped part) so there is relief to light.
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        for (lo, hi, top) in [(14.0_f32, 46.0, 6.0), (24.0, 36.0, 12.0)] {
            let base = vertices.len() as u32;
            vertices.extend([
                [lo, lo, 0.0],
                [hi, lo, 0.0],
                [hi, hi, 0.0],
                [lo, hi, 0.0],
                [lo, lo, top],
                [hi, lo, top],
                [hi, hi, top],
                [lo, hi, top],
            ]);
            for t in [
                [0, 2, 1],
                [0, 3, 2],
                [4, 5, 6],
                [4, 6, 7],
                [0, 1, 5],
                [0, 5, 4],
                [1, 2, 6],
                [1, 6, 5],
                [2, 3, 7],
                [2, 7, 6],
                [3, 0, 4],
                [3, 4, 7],
            ] {
                triangles.push([base + t[0], base + t[1], base + t[2]]);
            }
        }
        let count = triangles.len();
        let model = Model {
            name: "stepped".into(),
            source_path: "fixture".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: Default::default(),
            vertices,
            triangles,
            bodies: vec![MeshBody {
                name: "stepped".into(),
                source_item_id: 1,
                first_triangle: 0,
                triangle_count: count,
                triangle_face_ids: (1..=count as u64).collect(),
                transform: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
                bounds: [[14., 14., 0.], [46., 46., 12.]],
            }],
            bounds: [[14., 14., 0.], [46., 46., 12.]],
            import_notes: vec![],
        };
        let setup = MachiningSetup {
            stock_size_mm: [60.0, 60.0, 14.0],
            path_tolerance_mm: 0.5,
            ..MachiningSetup::default()
        };
        crate::renderer::lighting::set_target_is_srgb(false);
        let build_at =
            |operations: &[Operation], removal: bool, material: StockMaterial, orbit: [f32; 2]| {
                Scene::build(
                    &[],
                    std::slice::from_ref(&model),
                    operations,
                    0,
                    true,
                    orbit,
                    Some(0),
                    &[],
                    false,
                    &setup,
                    removal,
                    true,
                    material,
                    None,
                )
            };
        let build = |operations: &[Operation], removal: bool, material: StockMaterial| {
            build_at(operations, removal, material, [-0.35, 0.05])
        };
        rasterise(
            &build(&[], false, StockMaterial::Aluminum),
            &format!("{out}/model.png"),
        );

        let tool = crate::cam::tool::Tool {
            diameter_mm: 4.0,
            flute_length_mm: 15.0,
            stickout_mm: 30.0,
            ..Default::default()
        };
        let params = adaptive::AdaptiveParams::conservative(&setup, &tool, 0.0);
        let path = adaptive::generate(&model, &setup, &tool, &params).expect("roughing");
        let mut operation = Operation::roughing_3d();
        operation.tool = tool;
        operation.generated = true;
        operation.toolpath = path;
        rasterise(
            &build(&[operation.clone()], true, StockMaterial::Wood),
            &format!("{out}/wood_removal.png"),
        );
        rasterise(
            &build(&[operation.clone()], true, StockMaterial::Aluminum),
            &format!("{out}/aluminum_removal.png"),
        );
        rasterise(
            &build_at(
                &[operation.clone()],
                true,
                StockMaterial::Aluminum,
                [1.6, 0.05],
            ),
            &format!("{out}/aluminum_removal_back.png"),
        );
        rasterise(
            &build_at(&[operation], true, StockMaterial::Aluminum, [-0.35, 0.7]),
            &format!("{out}/aluminum_removal_high.png"),
        );
    }

    #[test]
    fn view_direction_points_at_the_viewer_and_agrees_with_front_facing() {
        for orbit in [[0.0, 0.0], [-0.35, 0.05], [1.6, 0.4], [3.0, -0.2]] {
            let toward = view_direction(orbit);
            let length = toward.iter().map(|c| c * c).sum::<f32>().sqrt();
            assert!((length - 1.0).abs() < 1e-4);
            // Nearer points have a smaller camera depth.
            let near = camera_point([toward[0], toward[1], toward[2]], orbit)[2];
            let far = camera_point([-toward[0], -toward[1], -toward[2]], orbit)[2];
            assert!(near < far);
            // A triangle whose normal faces the viewer is front-facing.
            let up = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
            let facing = face_normal(up)[2] * toward[2] > 0.0;
            assert_eq!(is_front_facing(up, orbit), facing);
        }
    }

    #[test]
    fn large_triangles_are_split_without_changing_their_area() {
        let big = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0]];
        let mut pieces = vec![big];
        split_large_triangles(&mut pieces, 3.0, 4);
        assert!(pieces.len() > 4);
        let area = |t: &[[f32; 3]; 3]| {
            let n = face_normal(*t);
            0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
        };
        let total: f32 = pieces.iter().map(area).sum();
        assert!((total - area(&big)).abs() < 1e-3);
        // Small triangles are left alone.
        let mut small = vec![[[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]];
        split_large_triangles(&mut small, 3.0, 4);
        assert_eq!(small.len(), 1);
    }

    #[test]
    fn every_operation_toolpath_gets_its_own_colour() {
        use crate::cam::operations::{operation_color, Operation};
        let mut operations = Vec::new();
        for shift in [0.0, 10.0] {
            let mut operation = Operation::contour();
            operation.generated = true;
            operation.toolpath.push_rapid([shift, 0.0], 5.0);
            operation.toolpath.push_feed([shift, 0.0], -1.0, 100.0);
            operation
                .toolpath
                .push_feed([shift + 5.0, 0.0], -1.0, 100.0);
            operation.toolpath.push_rapid([shift + 5.0, 0.0], 5.0);
            operations.push(operation);
        }
        let scene = Scene::build(
            &[],
            &[],
            &operations,
            0,
            false,
            [0.0, 0.0],
            None,
            &[],
            false,
            &MachiningSetup::default(),
            false,
            false,
            StockMaterial::default(),
            None,
        );
        // Opaque (feed) vertices carry the operation colour; rapids are faint.
        let feed_colors: Vec<[f32; 3]> = scene
            .toolpaths
            .iter()
            .filter(|vertex| vertex.color[3] > 0.9)
            .map(|vertex| [vertex.color[0], vertex.color[1], vertex.color[2]])
            .collect();
        assert!(feed_colors.contains(&operation_color(0)));
        assert!(feed_colors.contains(&operation_color(1)));
        assert_ne!(operation_color(0), operation_color(1));
        assert!(scene.toolpaths.iter().any(|vertex| vertex.color[3] < 0.5));
    }

    #[test]
    fn hidden_operations_are_not_drawn_but_keep_their_colour_slot() {
        use crate::cam::operations::{operation_color, Operation};
        let mut operations = Vec::new();
        for shift in [0.0, 10.0] {
            let mut operation = Operation::contour();
            operation.generated = true;
            operation.toolpath.push_rapid([shift, 0.0], 5.0);
            operation.toolpath.push_feed([shift, 0.0], -1.0, 100.0);
            operation
                .toolpath
                .push_feed([shift + 5.0, 0.0], -1.0, 100.0);
            operations.push(operation);
        }
        operations[0].visible = false;
        let scene = Scene::build(
            &[],
            &[],
            &operations,
            0,
            false,
            [0.0, 0.0],
            None,
            &[],
            false,
            &MachiningSetup::default(),
            false,
            false,
            StockMaterial::default(),
            None,
        );
        let colors: Vec<[f32; 3]> = scene
            .toolpaths
            .iter()
            .map(|vertex| [vertex.color[0], vertex.color[1], vertex.color[2]])
            .collect();
        assert!(!colors.is_empty());
        // Only the second operation is drawn, still in its own (second) colour.
        assert!(colors.iter().all(|color| *color == operation_color(1)));
    }

    #[test]
    fn axis_triad_stays_centered_on_the_stock_top() {
        let orbit = [0.3, 0.1];
        let mut setup = MachiningSetup::default();
        for (origin, size) in [
            ([0.0, 0.0, 0.0], [100.0, 100.0, 20.0]),
            ([-40.0, 25.0, -5.0], [300.0, 40.0, 12.0]),
        ] {
            setup.stock_origin_mm = origin;
            setup.stock_size_mm = size;
            let scene = Scene::build(
                &[],
                &[],
                &[],
                0,
                true,
                orbit,
                None,
                &[],
                false,
                &setup,
                false,
                false,
                StockMaterial::default(),
                None,
            );
            let top = match setup.axis {
                WorkAxis::ZUp => origin[2] + size[2],
                WorkAxis::ZDown => origin[2],
            };
            let expected = project_point(
                (origin[0] + size[0] * 0.5) as f32,
                (origin[1] + size[1] * 0.5) as f32,
                top as f32,
                true,
                orbit,
            );
            // Every axis line starts at the stock-top centre.
            for color in [
                [0.95, 0.3, 0.3, 1.0],
                [0.35, 0.85, 0.4, 1.0],
                [0.35, 0.6, 1.0, 1.0],
            ] {
                let start = scene
                    .lines
                    .iter()
                    .find(|vertex| vertex.color == color)
                    .expect("axis line present")
                    .pos;
                assert!((start[0] - expected[0]).abs() < 1e-3);
                assert!((start[1] - expected[1]).abs() < 1e-3);
            }
        }
    }
}

impl GeometryRenderer {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("geometry_uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("geometry_bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("geometry_bg"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform_buffer.as_entire_binding(),
            }],
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geometry_shader"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../../assets/shaders/geometry.wgsl").into(),
            ),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("geometry_pipeline_layout"),
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });

        let attrs = wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
        let make = |topology: wgpu::PrimitiveTopology, label: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target_format,
                        blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };

        Self {
            tri_pipeline: make(
                wgpu::PrimitiveTopology::TriangleList,
                "geometry_tri_pipeline",
            ),
            line_pipeline: make(wgpu::PrimitiveTopology::LineList, "geometry_line_pipeline"),
            uniform_buffer,
            bind_group,
            tri_buffer: None,
            tri_count: 0,
            line_buffer: None,
            line_count: 0,
            toolpath_buffer: None,
            toolpath_count: 0,
            revision: u64::MAX,
        }
    }

    pub fn update_camera(&self, queue: &wgpu::Queue, view_proj: &glam::Mat4) {
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::cast_slice(&view_proj.to_cols_array()),
        );
    }

    /// Re-upload vertex buffers if the scene changed.
    pub fn sync(&mut self, device: &wgpu::Device, scene: &Scene) {
        if scene.revision == self.revision {
            return;
        }
        self.revision = scene.revision;
        let upload = |verts: &[Vertex], label: &str| {
            if verts.is_empty() {
                None
            } else {
                Some(
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents: bytemuck::cast_slice(verts),
                        usage: wgpu::BufferUsages::VERTEX,
                    }),
                )
            }
        };
        self.tri_buffer = upload(&scene.triangles, "geometry_tris");
        self.tri_count = scene.triangles.len() as u32;
        self.line_buffer = upload(&scene.lines, "geometry_lines");
        self.line_count = scene.lines.len() as u32;
        self.toolpath_buffer = upload(&scene.toolpaths, "toolpath_lines");
        self.toolpath_count = scene.toolpaths.len() as u32;
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'static>) {
        if let Some(buf) = &self.tri_buffer {
            pass.set_pipeline(&self.tri_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..self.tri_count, 0..1);
        }
        if let Some(buf) = &self.line_buffer {
            pass.set_pipeline(&self.line_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..self.line_count, 0..1);
        }
        if let Some(buf) = &self.toolpath_buffer {
            pass.set_pipeline(&self.line_pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.set_vertex_buffer(0, buf.slice(..));
            pass.draw(0..self.toolpath_count, 0..1);
        }
    }
}
