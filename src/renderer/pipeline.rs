// pipeline.rs — geometry render pipelines (Phase 2). Toolpath pipeline arrives in Phase 3.
use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu;
use egui_wgpu::wgpu::util::DeviceExt;
use image::RgbImage;
use std::sync::OnceLock;

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
    pixel.0.map(|channel| {
        let srgb = channel as f32 / 255.0;
        if srgb <= 0.04045 {
            srgb / 12.92
        } else {
            ((srgb + 0.055) / 1.055).powf(2.4)
        }
    })
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

fn append_shaded_triangle(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    base: [f32; 3],
    orbit: [f32; 2],
) {
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
    let magnitude = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
        .sqrt()
        .max(1e-8);
    let light = (normal[0] * 0.3 + normal[1] * 0.5 + normal[2] * 0.8) / magnitude;
    let shade = 0.28 + 0.72 * light.max(0.0);
    let color = [base[0] * shade, base[1] * shade, base[2] * shade, 1.0];
    for point in points {
        scene.triangles.push(Vertex {
            pos: project_point(point[0], point[1], point[2], true, orbit),
            color,
        });
    }
}

fn append_material_triangle(
    scene: &mut Scene,
    points: [[f32; 3]; 3],
    colors: [[f32; 3]; 3],
    orbit: [f32; 2],
) {
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
    let magnitude = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2])
        .sqrt()
        .max(1e-8);
    let light = (normal[0] * 0.3 + normal[1] * 0.5 + normal[2] * 0.8) / magnitude;
    let shade = 0.28 + 0.72 * light.max(0.0);
    for (point, color) in points.into_iter().zip(colors) {
        scene.triangles.push(Vertex {
            pos: project_point(point[0], point[1], point[2], true, orbit),
            color: [color[0] * shade, color[1] * shade, color[2] * shade, 1.0],
        });
    }
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
            append_stock_solid(&mut scene, setup, stock_material, orbit, render_stock);
        }
        for l in layers.iter().filter(|l| l.visible) {
            let color = l.color;
            let layer_z = if view_3d
                && matches!(
                    l.kind,
                    crate::workspace::layer::LayerKind::Text
                        | crate::workspace::layer::LayerKind::Drawing
                ) {
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
                let depth = points
                    .iter()
                    .map(|p| camera_point(*p, orbit)[2])
                    .sum::<f32>()
                    / 3.0;
                faces.push((depth, points, color));
            }
            // camera_point() produces larger depth for points closer to the
            // viewer. Paint the far faces first so nearer faces cover them.
            // Sort the sampled drag preview too; otherwise source mesh order
            // lets back faces flash over the front while the camera orbits.
            faces.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, points, color) in faces {
                append_shaded_triangle(&mut scene, points, color, orbit);
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
            if let Some(model) = models.iter().find(|model| model.visible) {
                let extent = (0..3)
                    .map(|axis| {
                        let bounds = model.transformed_bounds();
                        (bounds[1][axis] - bounds[0][axis]).abs()
                    })
                    .fold(1.0_f32, f32::max)
                    * 0.6;
                for (end, color) in [
                    ([extent, 0.0, 0.0], [0.95, 0.3, 0.3, 1.0]),
                    ([0.0, extent, 0.0], [0.35, 0.85, 0.4, 1.0]),
                    ([0.0, 0.0, extent], [0.35, 0.6, 1.0, 1.0]),
                ] {
                    scene.lines.push(Vertex {
                        pos: project_point(0.0, 0.0, 0.0, true, orbit),
                        color,
                    });
                    scene.lines.push(Vertex {
                        pos: project_point(end[0], end[1], end[2], true, orbit),
                        color,
                    });
                }
            }
        }
        for op in operations.iter().filter(|op| op.generated) {
            let colour = |kind| match kind {
                MoveKind::Rapid => [0.35, 0.55, 0.95, 0.55],
                MoveKind::Feed => [1.0, 0.45, 0.15, 1.0],
                MoveKind::Arc => [0.85, 0.35, 1.0, 1.0],
                MoveKind::Drill => [1.0, 0.85, 0.15, 1.0],
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
            depth(&a.0).total_cmp(&depth(&b.0))
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
        triangles.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, points) in triangles {
            append_shaded_triangle(scene, points, base, orbit);
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

    let preview_paths: Vec<(Vec<ToolMove>, f64, [f64; 2])> = if let Some(simulation) = simulation {
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
        let radius = operations
            .iter()
            .find(|operation| operation.generated)
            .map(|operation| operation.tool.diameter_mm * 0.5)
            .unwrap_or(0.5);
        vec![(moves, radius, [origin[0], origin[1]])]
    } else {
        operations
            .iter()
            .filter(|operation| operation.generated)
            .map(|operation| {
                (
                    operation.toolpath.moves.clone(),
                    operation.tool.diameter_mm * 0.5,
                    [0.0, 0.0],
                )
            })
            .collect()
    };

    for (moves, radius, start_xy) in preview_paths {
        let mut current = [start_xy[0], start_xy[1], top];
        for movement in &moves {
            match movement {
                ToolMove::Rapid { to, z } => current = [to[0], to[1], *z],
                ToolMove::Feed { to, z, .. } => {
                    apply_cut_segment(
                        &mut heights,
                        nx,
                        ny,
                        origin,
                        dx,
                        dy,
                        floor,
                        [current[0], current[1], current[2]],
                        [to[0], to[1], *z],
                        radius,
                    );
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
                        apply_cut_segment(
                            &mut heights,
                            nx,
                            ny,
                            origin,
                            dx,
                            dy,
                            floor,
                            previous,
                            next,
                            radius,
                        );
                        previous = next;
                    }
                    current = [to[0], to[1], *z];
                }
                ToolMove::Drill { at, depth, .. } => {
                    apply_cut_segment(
                        &mut heights,
                        nx,
                        ny,
                        origin,
                        dx,
                        dy,
                        floor,
                        [at[0], at[1], top],
                        [at[0], at[1], *depth],
                        radius,
                    );
                    current = [at[0], at[1], *depth];
                }
            }
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
    for iy in 0..ny {
        for ix in 0..nx {
            let a = point(ix, iy);
            let b = point(ix + 1, iy);
            let c = point(ix + 1, iy + 1);
            let d = point(ix, iy + 1);
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
                append_material_triangle(
                    scene,
                    [a, b, c],
                    [texture_color(a), texture_color(b), texture_color(c)],
                    orbit,
                );
                append_material_triangle(
                    scene,
                    [a, c, d],
                    [texture_color(a), texture_color(c), texture_color(d)],
                    orbit,
                );
            } else {
                append_shaded_triangle(scene, [a, b, c], cut_base, orbit);
                append_shaded_triangle(scene, [a, c, d], cut_base, orbit);
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

fn apply_cut_segment(
    heights: &mut [f64],
    nx: usize,
    ny: usize,
    origin: [f64; 3],
    dx: f64,
    dy: f64,
    floor: f64,
    start: [f64; 3],
    end: [f64; 3],
    cutter_radius: f64,
) {
    // Sample the actual cutter footprint. Expanding by half a grid cell made
    // each cut visibly oversized and square at the preview's old resolution.
    let radius = cutter_radius;
    let min_x = (((start[0].min(end[0]) - radius - origin[0]) / dx).floor() as isize)
        .clamp(0, nx as isize) as usize;
    let max_x = (((start[0].max(end[0]) + radius - origin[0]) / dx).ceil() as isize)
        .clamp(0, nx as isize) as usize;
    let min_y = (((start[1].min(end[1]) - radius - origin[1]) / dy).floor() as isize)
        .clamp(0, ny as isize) as usize;
    let max_y = (((start[1].max(end[1]) + radius - origin[1]) / dy).ceil() as isize)
        .clamp(0, ny as isize) as usize;
    let ab = [end[0] - start[0], end[1] - start[1]];
    let length_sq = ab[0] * ab[0] + ab[1] * ab[1];
    let radius_sq = radius * radius;
    for iy in min_y..=max_y {
        for ix in min_x..=max_x {
            let x = origin[0] + ix as f64 * dx;
            let y = origin[1] + iy as f64 * dy;
            let t = if length_sq > f64::EPSILON {
                (((x - start[0]) * ab[0] + (y - start[1]) * ab[1]) / length_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let px = start[0] + ab[0] * t;
            let py = start[1] + ab[1] * t;
            let dist_sq = (x - px) * (x - px) + (y - py) * (y - py);
            if dist_sq <= radius_sq {
                let cut_z = (start[2] + (end[2] - start[2]) * t).max(floor);
                let height = &mut heights[iy * (nx + 1) + ix];
                *height = height.min(cut_z);
            }
        }
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
