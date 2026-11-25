//! Small isometric preview of a STEP model under a candidate orientation, used
//! by the "Fit stock to model" dialog so the user can see how the part will sit
//! (and which face points up toward the tool) before the stock is fitted.

use crate::workspace::model::{Model, ModelTransform};

/// Most triangles drawn; larger models are sampled evenly for the preview.
const MAX_TRIANGLES: usize = 9_000;

/// Stock box shown around the model: `(origin, size)` in millimetres.
pub type PreviewStock = ([f64; 3], [f64; 3]);

/// Rotate a point into the preview camera. `x`/`y` are screen axes (y up);
/// `z` grows toward the viewer.
fn camera_point(point: [f32; 3], orbit: [f32; 2]) -> [f32; 3] {
    let yaw = std::f32::consts::FRAC_PI_4 + orbit[0];
    let pitch = (35.264_f32.to_radians() + orbit[1]).clamp(-1.52, 1.52);
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let x = point[0] * cy - point[1] * sy;
    let y = point[0] * sy + point[1] * cy;
    [x, y * sp + point[2] * cp, y * cp - point[2] * sp]
}

fn corners(origin: [f64; 3], size: [f64; 3]) -> [[f32; 3]; 8] {
    std::array::from_fn(|index| {
        [
            (origin[0] + if index & 1 != 0 { size[0] } else { 0.0 }) as f32,
            (origin[1] + if index & 2 != 0 { size[1] } else { 0.0 }) as f32,
            (origin[2] + if index & 4 != 0 { size[2] } else { 0.0 }) as f32,
        ]
    })
}

/// Draw the preview and let the user drag it to look around. `orbit` holds the
/// view's yaw/pitch offsets and is updated by dragging.
pub fn show(
    ui: &mut egui::Ui,
    model: &Model,
    transform: ModelTransform,
    stock: Option<PreviewStock>,
    orbit: &mut [f32; 2],
    dark: bool,
    height: f32,
) {
    let width = ui.available_width().clamp(260.0, 420.0);
    let (response, painter) =
        ui.allocate_painter(egui::vec2(width, height), egui::Sense::click_and_drag());
    if response.dragged() {
        let delta = response.drag_delta();
        orbit[0] += delta.x * 0.01;
        orbit[1] = (orbit[1] - delta.y * 0.01).clamp(-1.2, 1.2);
    }
    if response.double_clicked() {
        *orbit = [0.0, 0.0];
    }
    let rect = response.rect;
    painter.rect_filled(
        rect,
        egui::CornerRadius::same(4),
        if dark {
            egui::Color32::from_rgb(30, 31, 38)
        } else {
            egui::Color32::from_rgb(236, 238, 244)
        },
    );

    // Frame the part (and its stock) in the preview.
    let bounds = model.bounds_with_transform(transform);
    let model_origin = bounds[0].map(f64::from);
    let model_size: [f64; 3] =
        std::array::from_fn(|axis| (bounds[1][axis] - bounds[0][axis]) as f64);
    let (frame_origin, frame_size) = stock.unwrap_or((model_origin, model_size));
    let mut low = [f32::INFINITY; 2];
    let mut high = [f32::NEG_INFINITY; 2];
    for corner in corners(frame_origin, frame_size) {
        let projected = camera_point(corner, *orbit);
        for axis in 0..2 {
            low[axis] = low[axis].min(projected[axis]);
            high[axis] = high[axis].max(projected[axis]);
        }
    }
    let span = (high[0] - low[0]).max(high[1] - low[1]).max(1e-3);
    let scale = (rect.width().min(rect.height()) * 0.78) / span;
    let middle = [(low[0] + high[0]) * 0.5, (low[1] + high[1]) * 0.5];
    let to_screen = |point: [f32; 3]| {
        let projected = camera_point(point, *orbit);
        egui::pos2(
            rect.center().x + (projected[0] - middle[0]) * scale,
            rect.center().y - (projected[1] - middle[1]) * scale,
        )
    };

    // Shaded triangles, far ones first (painter's algorithm).
    let stride = model.triangles.len().div_ceil(MAX_TRIANGLES).max(1);
    let light = {
        let v = [0.35_f32, -0.45, 0.82];
        let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|c| c / length)
    };
    let mut faces: Vec<(f32, [egui::Pos2; 3], egui::Color32)> =
        Vec::with_capacity(model.triangles.len() / stride + 1);
    for triangle in model.triangles.iter().step_by(stride) {
        let points: Option<Vec<[f32; 3]>> = triangle
            .iter()
            .map(|&index| model.vertex_with_transform(index as usize, transform))
            .collect();
        let Some(points) = points else { continue };
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
        let mut normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length <= 1e-9 {
            continue;
        }
        normal = normal.map(|c| c / length);
        // Face the viewer so inconsistent triangle winding still shades well.
        let toward_viewer = camera_point(normal, *orbit)[2];
        if toward_viewer < 0.0 {
            normal = normal.map(|c| -c);
        }
        let lit = (normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2]).max(0.0);
        let brightness = 0.38 + 0.62 * lit;
        let color = egui::Color32::from_rgb(
            (242.0 * brightness) as u8,
            (166.0 * brightness) as u8,
            (71.0 * brightness) as u8,
        );
        let depth = points
            .iter()
            .map(|p| camera_point(*p, *orbit)[2])
            .sum::<f32>()
            / 3.0;
        faces.push((
            depth,
            [
                to_screen(points[0]),
                to_screen(points[1]),
                to_screen(points[2]),
            ],
            color,
        ));
    }
    faces.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut mesh = egui::Mesh::default();
    for (_, screen, color) in faces {
        let base = mesh.vertices.len() as u32;
        for point in screen {
            mesh.colored_vertex(point, color);
        }
        mesh.add_triangle(base, base + 1, base + 2);
    }
    painter.add(egui::Shape::mesh(mesh));

    // Stock outline.
    if let Some((origin, size)) = stock {
        let c = corners(origin, size);
        let outline = egui::Stroke::new(
            1.2_f32,
            if dark {
                egui::Color32::from_rgb(120, 160, 205)
            } else {
                egui::Color32::from_rgb(70, 110, 160)
            },
        );
        for (a, b) in [
            (0, 1),
            (1, 3),
            (3, 2),
            (2, 0),
            (4, 5),
            (5, 7),
            (7, 6),
            (6, 4),
            (0, 4),
            (1, 5),
            (2, 6),
            (3, 7),
        ] {
            painter.line_segment([to_screen(c[a]), to_screen(c[b])], outline);
        }
    }

    // Axis triad at the near-bottom corner of the frame.
    let anchor = frame_origin.map(|v| v as f32);
    let length = (frame_size[0].max(frame_size[1]).max(frame_size[2]) * 0.3) as f32;
    let text_color = if dark {
        egui::Color32::from_rgb(210, 212, 222)
    } else {
        egui::Color32::from_rgb(40, 44, 56)
    };
    for (label, direction, color) in [
        ("X", [1.0, 0.0, 0.0], egui::Color32::from_rgb(235, 80, 88)),
        ("Y", [0.0, 1.0, 0.0], egui::Color32::from_rgb(100, 210, 90)),
        (
            "Z up (tool)",
            [0.0, 0.0, 1.0],
            egui::Color32::from_rgb(70, 150, 255),
        ),
    ] {
        let tip = [
            anchor[0] + direction[0] * length,
            anchor[1] + direction[1] * length,
            anchor[2] + direction[2] * length,
        ];
        let (from, to) = (to_screen(anchor), to_screen(tip));
        painter.line_segment([from, to], egui::Stroke::new(2.0_f32, color));
        painter.text(
            to + (to - from).normalized() * 8.0,
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(11.0),
            color,
        );
    }
    painter.text(
        rect.left_bottom() + egui::vec2(8.0, -6.0),
        egui::Align2::LEFT_BOTTOM,
        "Drag to look around · double-click to reset",
        egui::FontId::proportional(10.0),
        text_color.gamma_multiply(0.7),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::StepLengthUnit;

    fn wedge() -> Model {
        Model {
            name: "wedge".into(),
            source_path: "fixture:wedge".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: ModelTransform::default(),
            vertices: vec![
                [0.0, 0.0, 0.0],
                [10.0, 0.0, 0.0],
                [10.0, 6.0, 0.0],
                [0.0, 6.0, 0.0],
                [0.0, 0.0, 4.0],
                [10.0, 0.0, 4.0],
            ],
            triangles: vec![[0, 1, 2], [0, 2, 3], [0, 1, 5], [0, 5, 4]],
            bodies: vec![],
            bounds: [[0.0, 0.0, 0.0], [10.0, 6.0, 4.0]],
            import_notes: vec![],
        }
    }

    /// Run the preview for one frame and count what it painted.
    fn painted(model: &Model, transform: ModelTransform) -> (usize, usize) {
        let ctx = egui::Context::default();
        let mut orbit = [0.0_f32, 0.0];
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(
                    ui,
                    model,
                    transform,
                    Some(([-1.0, -1.0, -1.0], [12.0, 8.0, 6.0])),
                    &mut orbit,
                    true,
                    200.0,
                );
            });
        });
        let mut triangles = 0;
        let mut lines = 0;
        for clipped in &output.shapes {
            let mut stack = vec![&clipped.shape];
            while let Some(shape) = stack.pop() {
                match shape {
                    egui::Shape::Mesh(mesh) => triangles += mesh.indices.len() / 3,
                    egui::Shape::LineSegment { .. } => lines += 1,
                    egui::Shape::Vec(items) => stack.extend(items.iter()),
                    _ => {}
                }
            }
        }
        (triangles, lines)
    }

    #[test]
    fn preview_paints_the_model_and_the_stock_outline() {
        let model = wedge();
        let (triangles, lines) = painted(&model, model.transform);
        assert!(triangles >= model.triangles.len(), "{triangles} triangles");
        // 12 stock edges plus the three axis lines.
        assert!(lines >= 15, "{lines} lines");
    }

    #[test]
    fn preview_follows_the_candidate_orientation_without_touching_the_model() {
        let model = wedge();
        let flipped = model
            .transform
            .rotated_about_world_axis(0, 180.0, [5.0, 3.0, 2.0]);
        let (triangles, _) = painted(&model, flipped);
        assert!(triangles >= model.triangles.len());
        assert_eq!(model.transform, ModelTransform::default());
    }
}
