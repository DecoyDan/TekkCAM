use std::collections::{HashMap, HashSet};

use crate::cam::{
    operations::Operation,
    toolpath::{ToolMove, Toolpath},
};
use crate::workspace::{model::Model, object::Geometry};

/// Generate a STEP profile from horizontal face boundaries while preserving
/// each boundary's height relative to the model top.
pub fn generate_level_profile(model: &Model, operation: &Operation) -> Result<Toolpath, String> {
    if operation.source_face_ids.is_empty() {
        return Err("select at least one STEP face for the model profile".into());
    }
    model.validate()?;
    let selected: HashSet<u64> = operation.source_face_ids.iter().copied().collect();
    let top = model.transformed_bounds()[1][2] as f64;
    let mut edges: HashMap<([i64; 3], [i64; 3]), ([f32; 3], [f32; 3], usize)> = HashMap::new();
    for (index, triangle) in model.triangles.iter().enumerate() {
        let Some(face_id) = source_face_id(model, index) else {
            continue;
        };
        if !selected.contains(&face_id) {
            continue;
        }
        let Some(p) = triangle
            .map(|i| model.transformed_vertex(i as usize))
            .into_iter()
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        let n = cross(sub(p[1], p[0]), sub(p[2], p[0]));
        let len = dot(n, n).sqrt();
        if len <= 1e-12 || n[2] <= len * 0.05 {
            continue;
        }
        for (a, b) in [(0, 1), (1, 2), (2, 0)] {
            let quant = |v: [f32; 3]| v.map(|x| (x as f64 * 1e6).round() as i64);
            let ka = quant(p[a]);
            let kb = quant(p[b]);
            let key = if ka < kb { (ka, kb) } else { (kb, ka) };
            let e = edges.entry(key).or_insert((p[a], p[b], 0));
            e.2 += 1;
        }
    }
    let mut levels: std::collections::BTreeMap<i64, Geometry> = std::collections::BTreeMap::new();
    for (_, (a, b, uses)) in edges {
        if uses != 1 || (a[2] - b[2]).abs() > 1e-5 {
            continue;
        }
        let level = ((a[2] as f64) * 1e5).round() as i64;
        levels
            .entry(level)
            .or_default()
            .lines
            .extend([[a[0], a[1]], [b[0], b[1]]]);
    }
    // Faceted or open STEP meshes may not expose horizontal top-face edges.
    // Preserve the legacy projected outline for those models.
    if levels.is_empty() {
        let geometry = selected_face_boundary(model, &operation.source_face_ids)?;
        let mut fallback = operation.clone();
        fallback.generate_from_geometry(&geometry)?;
        let mut path = fallback.toolpath;
        translate_toolpath_z(&mut path, top);
        return Ok(path);
    }
    let mut result = Toolpath::default();
    for (level, geometry) in levels {
        let mut op = operation.clone();
        op.generate_from_geometry(&geometry)?;
        // Geometry-based 2D generation is relative to Z=0. Face levels are
        // already in model/world coordinates, so add the absolute face Z.
        let face_z = level as f64 / 1e5;
        for mut movement in op.toolpath.moves {
            match &mut movement {
                ToolMove::Rapid { z, .. } | ToolMove::Feed { z, .. } | ToolMove::Arc { z, .. } => {
                    *z += face_z
                }
                ToolMove::Drill { depth, retract, .. } => {
                    *depth += face_z;
                    *retract += face_z;
                }
            }
            result.moves.push(movement);
        }
    }
    Ok(result)
}

fn translate_toolpath_z(path: &mut Toolpath, offset: f64) {
    for movement in &mut path.moves {
        match movement {
            ToolMove::Rapid { z, .. } | ToolMove::Feed { z, .. } | ToolMove::Arc { z, .. } => {
                *z += offset;
            }
            ToolMove::Drill { depth, retract, .. } => {
                *depth += offset;
                *retract += offset;
            }
        }
    }
}

/// Project the outer edges of selected STEP source faces into XY geometry for
/// the existing 2D contour toolpath generator. When every source face is
/// selected, use upward-facing triangles to extract the top-view silhouette;
/// a complete closed shell has no topological boundary of its own.
pub fn selected_face_boundary(model: &Model, face_ids: &[u64]) -> Result<Geometry, String> {
    if face_ids.is_empty() {
        return Err("select at least one STEP face for the model profile".into());
    }
    model.validate()?;
    let selected: HashSet<u64> = face_ids.iter().copied().collect();
    let all_faces = model
        .bodies
        .iter()
        .flat_map(|body| body.triangle_face_ids.iter().copied())
        .collect::<HashSet<_>>();
    let silhouette_mode = !all_faces.is_empty() && all_faces.is_subset(&selected);
    let mut edges: HashMap<([i64; 3], [i64; 3]), ([f32; 3], [f32; 3], usize)> = HashMap::new();
    for (triangle_index, triangle) in model.triangles.iter().enumerate() {
        let Some(face_id) = source_face_id(model, triangle_index) else {
            continue;
        };
        if !selected.contains(&face_id) {
            continue;
        }
        let Some(points) = triangle
            .map(|index| model.transformed_vertex(index as usize))
            .into_iter()
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if silhouette_mode {
            let normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
            let length = dot(normal, normal).sqrt();
            if length <= 1e-12 || normal[2] <= length * 0.05 {
                continue;
            }
        }
        for (edge_a, edge_b) in [(0, 1), (1, 2), (2, 0)] {
            let pa = points[edge_a];
            let pb = points[edge_b];
            if (pa[0] - pb[0]).hypot(pa[1] - pb[1]) <= 1e-6 {
                continue;
            }
            let quantize_3d =
                |point: [f32; 3]| point.map(|value| (value as f64 * 1e6).round() as i64);
            let ka = quantize_3d(pa);
            let kb = quantize_3d(pb);
            let key = if ka < kb { (ka, kb) } else { (kb, ka) };
            let entry = edges.entry(key).or_insert((pa, pb, 0));
            entry.2 += 1;
        }
    }

    let mut boundary = Geometry::default();
    let mut projected_edges = HashSet::new();
    for (_, (a, b, uses)) in edges {
        if uses != 1 {
            continue;
        }
        let (a, b) = ([a[0], a[1]], [b[0], b[1]]);
        let quantize = |point: [f32; 2]| {
            (
                (point[0] as f64 * 1e6).round() as i64,
                (point[1] as f64 * 1e6).round() as i64,
            )
        };
        let ka = quantize(a);
        let kb = quantize(b);
        let key = if ka < kb { (ka, kb) } else { (kb, ka) };
        if projected_edges.insert(key) {
            // Reverse the top-face winding so the profile offset normal points
            // outside a conventional counter-clockwise STEP face boundary.
            boundary.lines.extend([b, a]);
        }
    }
    if boundary.lines.is_empty() {
        return Err("selected STEP faces have no usable projected XY boundary; orient a machinable face toward +Z".into());
    }
    Ok(boundary)
}

/// Return every STEP source face so the 2D profile generator can use the
/// complete shell to calculate its top-view silhouette. Selecting only
/// individually upward-facing triangles can create internal edges and misses
/// the outline behavior needed for an automatic profile.
pub fn automatic_profile_faces(model: &Model) -> Vec<u64> {
    let face_ids: HashSet<u64> = model
        .bodies
        .iter()
        .flat_map(|body| body.triangle_face_ids.iter().copied())
        .collect();
    let mut face_ids: Vec<_> = face_ids.into_iter().collect();
    face_ids.sort_unstable();
    face_ids
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f64; 3] {
    [
        (a[0] - b[0]) as f64,
        (a[1] - b[1]) as f64,
        (a[2] - b[2]) as f64,
    ]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn source_face_id(model: &Model, triangle_index: usize) -> Option<u64> {
    model.bodies.iter().find_map(|body| {
        let local = triangle_index.checked_sub(body.first_triangle)?;
        (local < body.triangle_count)
            .then(|| body.triangle_face_ids.get(local).copied())
            .flatten()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{MeshBody, StepLengthUnit};

    fn faceted_block() -> Model {
        Model {
            name: "block".into(),
            source_path: "fixture:block".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: Default::default(),
            vertices: vec![
                [0., 0., 0.],
                [2., 0., 0.],
                [2., 2., 0.],
                [0., 2., 0.],
                [0., 0., 1.],
                [2., 0., 1.],
                [2., 2., 1.],
                [0., 2., 1.],
            ],
            triangles: vec![
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
            ],
            bodies: vec![MeshBody {
                name: "block".into(),
                source_item_id: 1,
                first_triangle: 0,
                triangle_count: 10,
                triangle_face_ids: vec![10, 10, 20, 20, 21, 21, 22, 22, 23, 23],
                transform: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
                bounds: [[0., 0., 0.], [2., 2., 1.]],
            }],
            bounds: [[0., 0., 0.], [2., 2., 1.]],
            import_notes: vec![],
        }
    }

    #[test]
    fn projects_selected_top_face_boundary_to_xy() {
        let geometry = selected_face_boundary(&faceted_block(), &[10]).unwrap();
        assert_eq!(geometry.lines.len(), 8);
        assert!(geometry
            .lines
            .iter()
            .all(|p| p[0] >= 0.0 && p[0] <= 2.0 && p[1] >= 0.0 && p[1] <= 2.0));
        let mut operation = crate::cam::operations::Operation::model_profile_2d();
        operation.total_depth_mm = 0.5;
        operation.depth_per_pass_mm = 0.5;
        operation.generate_from_geometry(&geometry).unwrap();
        let (min, max) = operation.toolpath.bounds().unwrap();
        assert!((min[0] + operation.tool.diameter_mm * 0.5).abs() < 1e-5);
        assert!((max[0] - (2.0 + operation.tool.diameter_mm * 0.5)).abs() < 1e-5);
    }

    #[test]
    fn rejects_empty_or_non_projectable_face_selection() {
        let model = faceted_block();
        assert!(selected_face_boundary(&model, &[])
            .unwrap_err()
            .contains("select"));
        assert!(selected_face_boundary(&model, &[999])
            .unwrap_err()
            .contains("no usable"));
    }
}
