use serde::{Deserialize, Serialize};

/// Units found in the STEP global representation context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepLengthUnit {
    Millimeter,
    Centimeter,
    Meter,
    Inch,
    Foot,
    Decimeter,
    Micrometer,
    Kilometer,
    #[default]
    Unknown,
}

impl StepLengthUnit {
    pub fn label(self) -> &'static str {
        match self {
            Self::Millimeter => "mm",
            Self::Centimeter => "cm",
            Self::Meter => "m",
            Self::Inch => "in",
            Self::Foot => "ft",
            Self::Decimeter => "dm",
            Self::Micrometer => "µm",
            Self::Kilometer => "km",
            Self::Unknown => "unknown",
        }
    }

    pub fn millimeters_per_unit(self) -> Option<f64> {
        match self {
            Self::Millimeter => Some(1.0),
            Self::Centimeter => Some(10.0),
            Self::Meter => Some(1000.0),
            Self::Inch => Some(25.4),
            Self::Foot => Some(304.8),
            Self::Decimeter => Some(100.0),
            Self::Micrometer => Some(0.001),
            Self::Kilometer => Some(1_000_000.0),
            Self::Unknown => None,
        }
    }
}

/// A connected tessellated body, indexed into the parent model's face array.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MeshBody {
    pub name: String,
    /// STEP representation item that produced this body.
    #[serde(default)]
    pub source_item_id: u64,
    pub first_triangle: usize,
    pub triangle_count: usize,
    /// Source B-rep face id for each triangle, when available.
    #[serde(default)]
    pub triangle_face_ids: Vec<u64>,
    /// Body-local to model transform as three row-major affine rows.
    #[serde(default = "identity_transform")]
    pub transform: [[f64; 4]; 3],
    /// Millimeters when the STEP unit was recognized; raw coordinates otherwise.
    pub bounds: [[f32; 3]; 2],
}

fn identity_transform() -> [[f64; 4]; 3] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
    ]
}

/// Reversible, project-space placement applied after the imported STEP transform.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelTransform {
    pub scale: f64,
    pub rotation_degrees: [f64; 3],
    pub translation_mm: [f64; 3],
}

impl ModelTransform {
    /// Rotation part as a row-major matrix (`M * p` rotates a column vector).
    fn rotation_matrix(&self) -> [[f64; 3]; 3] {
        let columns = [
            rotate_point([1.0, 0.0, 0.0], self.rotation_degrees),
            rotate_point([0.0, 1.0, 0.0], self.rotation_degrees),
            rotate_point([0.0, 0.0, 1.0], self.rotation_degrees),
        ];
        std::array::from_fn(|row| std::array::from_fn(|column| columns[column][row]))
    }

    /// Turn the placed model by `degrees` about a world axis (0 = X, 1 = Y,
    /// 2 = Z) through `pivot`. Scale is kept; the euler angles and translation
    /// are updated so the pivot does not move.
    pub fn rotated_about_world_axis(
        &self,
        axis: usize,
        degrees: f64,
        pivot: [f64; 3],
    ) -> ModelTransform {
        let mut about = [0.0; 3];
        about[axis.min(2)] = degrees;
        let turn = ModelTransform {
            rotation_degrees: about,
            ..ModelTransform::default()
        }
        .rotation_matrix();
        let current = self.rotation_matrix();
        let combined: [[f64; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|column| (0..3).map(|k| turn[row][k] * current[k][column]).sum())
        });
        let apply = |m: &[[f64; 3]; 3], p: [f64; 3]| -> [f64; 3] {
            std::array::from_fn(|row| (0..3).map(|k| m[row][k] * p[k]).sum())
        };
        let turned_translation = apply(&turn, self.translation_mm);
        let turned_pivot = apply(&turn, pivot);
        ModelTransform {
            scale: self.scale,
            rotation_degrees: euler_from_matrix(&combined),
            translation_mm: std::array::from_fn(|i| {
                turned_translation[i] - turned_pivot[i] + pivot[i]
            }),
        }
    }
}

/// Euler angles (applied X, then Y, then Z) that reproduce `matrix`. Angles
/// within a hair of a multiple of 90° are snapped so repeated quarter turns
/// do not drift.
fn euler_from_matrix(matrix: &[[f64; 3]; 3]) -> [f64; 3] {
    let snap = |degrees: f64| {
        let quarter = (degrees / 90.0).round() * 90.0;
        let value = if (degrees - quarter).abs() < 1e-6 {
            quarter
        } else {
            degrees
        };
        // Avoid "-0" and keep angles in (-180, 180].
        let wrapped = (value + 180.0).rem_euclid(360.0) - 180.0;
        if wrapped <= -180.0 + 1e-9 {
            180.0
        } else if wrapped.abs() < 1e-9 {
            0.0
        } else {
            wrapped
        }
    };
    let sin_beta = (-matrix[2][0]).clamp(-1.0, 1.0);
    let beta = sin_beta.asin();
    if beta.cos().abs() > 1e-6 {
        let alpha = matrix[2][1].atan2(matrix[2][2]);
        let gamma = matrix[1][0].atan2(matrix[0][0]);
        return [
            snap(alpha.to_degrees()),
            snap(beta.to_degrees()),
            snap(gamma.to_degrees()),
        ];
    }
    // Gimbal lock (Y at ±90°): X and Z turn about the same axis, so keep Z at 0
    // and find the X angle that reproduces the matrix.
    let beta_degrees = beta.to_degrees().round();
    let mut best = (f64::INFINITY, 0.0);
    for step in 0..360 {
        let alpha = step as f64;
        let candidate = ModelTransform {
            rotation_degrees: [alpha, beta_degrees, 0.0],
            ..ModelTransform::default()
        }
        .rotation_matrix();
        let error: f64 = (0..3)
            .flat_map(|r| (0..3).map(move |c| (r, c)))
            .map(|(r, c)| (candidate[r][c] - matrix[r][c]).abs())
            .sum();
        if error < best.0 {
            best = (error, alpha);
        }
    }
    [snap(best.1), snap(beta_degrees), 0.0]
}

impl Default for ModelTransform {
    fn default() -> Self {
        Self {
            scale: 1.0,
            rotation_degrees: [0.0; 3],
            translation_mm: [0.0; 3],
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct StockEnvelope {
    pub stock_origin_mm: [f64; 3],
    pub stock_size_mm: [f64; 3],
    pub xy_margin_mm: f64,
    pub z_margin_mm: f64,
}

/// Tessellated display representation of one imported STEP file.
/// Recognized units are normalized to millimeters; unknown units retain raw values.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub name: String,
    pub source_path: String,
    pub visible: bool,
    #[serde(default)]
    pub source_schema: Vec<String>,
    #[serde(default)]
    pub source_unit: StepLengthUnit,
    #[serde(default)]
    pub units_are_millimeters: bool,
    #[serde(default)]
    pub transform: ModelTransform,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    #[serde(default)]
    pub bodies: Vec<MeshBody>,
    /// Millimeters when the STEP unit was recognized; raw coordinates otherwise.
    pub bounds: [[f32; 3]; 2],
    #[serde(default)]
    pub import_notes: Vec<String>,
}

impl Model {
    pub fn transformed_vertex(&self, index: usize) -> Option<[f32; 3]> {
        self.vertices
            .get(index)
            .map(|vertex| transform_point(*vertex, self.transform).map(|value| value as f32))
    }

    pub fn transformed_bounds(&self) -> [[f32; 3]; 2] {
        self.bounds_with_transform(self.transform)
    }

    /// Bounds the model would have under `transform` (exact for 90° steps),
    /// without changing the model. Used to preview an orientation change.
    pub fn bounds_with_transform(&self, transform: ModelTransform) -> [[f32; 3]; 2] {
        let points = bounds_corners(self.bounds).map(|point| transform_point_f64(point, transform));
        reduce_bounds(points)
    }

    /// A vertex as it would be placed under `transform`.
    pub fn vertex_with_transform(
        &self,
        index: usize,
        transform: ModelTransform,
    ) -> Option<[f32; 3]> {
        self.vertices
            .get(index)
            .map(|vertex| transform_point(*vertex, transform))
    }

    pub fn reset_transform(&mut self) {
        self.transform = ModelTransform::default();
    }

    /// Calculate stock bounds around the model's existing transformed geometry.
    /// This never changes the model transform or its dimensions.
    pub fn stock_envelope(
        &self,
        setup: &crate::workspace::setup::MachiningSetup,
        xy_tool_clearance_mm: f64,
    ) -> Result<StockEnvelope, String> {
        self.stock_envelope_with_transform(setup, xy_tool_clearance_mm, self.transform)
    }

    /// Stock bounds around the model as it would sit under `transform`.
    pub fn stock_envelope_with_transform(
        &self,
        setup: &crate::workspace::setup::MachiningSetup,
        xy_tool_clearance_mm: f64,
        transform: ModelTransform,
    ) -> Result<StockEnvelope, String> {
        use crate::workspace::setup::WorkAxis;

        setup.validate()?;
        if !xy_tool_clearance_mm.is_finite() || xy_tool_clearance_mm < 0.0 {
            return Err("tool clearance must be a finite non-negative value".into());
        }
        let bounds = self.bounds_with_transform(transform);
        if bounds.iter().flatten().any(|value| !value.is_finite()) {
            return Err("model bounds must be finite to fit the stock".into());
        }
        let dims =
            std::array::from_fn::<_, 3, _>(|axis| (bounds[1][axis] - bounds[0][axis]).abs() as f64);
        if dims
            .iter()
            .any(|dimension| !dimension.is_finite() || *dimension <= 1e-9)
        {
            return Err("model must have positive finite bounds to fit the stock".into());
        }

        let xy_margin = setup.boundary_margin_mm.max(xy_tool_clearance_mm);
        let z_margin = setup.boundary_margin_mm;
        let stock_origin_mm = [
            bounds[0][0] as f64 - xy_margin,
            bounds[0][1] as f64 - xy_margin,
            match setup.axis {
                WorkAxis::ZUp => bounds[0][2] as f64 - z_margin,
                WorkAxis::ZDown => bounds[1][2] as f64 + z_margin,
            },
        ];
        let stock_size_mm = [
            dims[0] + xy_margin * 2.0,
            dims[1] + xy_margin * 2.0,
            dims[2] + z_margin * 2.0,
        ];
        if stock_origin_mm.iter().any(|value| !value.is_finite())
            || stock_size_mm.iter().any(|value| !value.is_finite())
        {
            return Err("stock envelope exceeds the supported coordinate range".into());
        }
        Ok(StockEnvelope {
            stock_origin_mm,
            stock_size_mm,
            xy_margin_mm: xy_margin,
            z_margin_mm: z_margin,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if !self.transform.scale.is_finite()
            || self.transform.scale <= 0.0
            || self
                .transform
                .rotation_degrees
                .iter()
                .any(|value| !value.is_finite())
            || self
                .transform
                .translation_mm
                .iter()
                .any(|value| !value.is_finite())
        {
            return Err(
                "model transform must have a positive finite scale and finite translation".into(),
            );
        }
        if self
            .transformed_bounds()
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err("transformed model bounds exceed the supported coordinate range".into());
        }
        if self.vertices.is_empty() || self.triangles.is_empty() {
            return Err("STEP import produced no displayable triangles".into());
        }
        if self.bodies.is_empty()
            || self.bodies.iter().any(|body| {
                body.first_triangle + body.triangle_count > self.triangles.len()
                    || body.triangle_face_ids.len() != body.triangle_count
            })
        {
            return Err("STEP mesh has invalid body ranges or face identity data".into());
        }
        if self
            .triangles
            .iter()
            .flatten()
            .any(|&i| i as usize >= self.vertices.len())
        {
            return Err("STEP mesh contains an invalid triangle index".into());
        }
        if self.vertices.iter().flatten().any(|v| !v.is_finite()) {
            return Err("STEP mesh contains non-finite coordinates".into());
        }
        Ok(())
    }
}

fn rotate_point(mut point: [f64; 3], degrees: [f64; 3]) -> [f64; 3] {
    for (axis, angle) in degrees.into_iter().enumerate() {
        let radians = angle.to_radians();
        let (sin, cos) = radians.sin_cos();
        point = match axis {
            0 => [
                point[0],
                point[1] * cos - point[2] * sin,
                point[1] * sin + point[2] * cos,
            ],
            1 => [
                point[0] * cos + point[2] * sin,
                point[1],
                -point[0] * sin + point[2] * cos,
            ],
            _ => [
                point[0] * cos - point[1] * sin,
                point[0] * sin + point[1] * cos,
                point[2],
            ],
        };
    }
    point
}

fn transform_point(point: [f32; 3], transform: ModelTransform) -> [f32; 3] {
    transform_point_f64(point.map(f64::from), transform).map(|value| value as f32)
}

fn transform_point_f64(point: [f64; 3], transform: ModelTransform) -> [f64; 3] {
    let rotated = rotate_point(point, transform.rotation_degrees);
    std::array::from_fn(|axis| rotated[axis] * transform.scale + transform.translation_mm[axis])
}

fn bounds_corners(bounds: [[f32; 3]; 2]) -> [[f64; 3]; 8] {
    let [min, max] = bounds;
    [
        [min[0] as f64, min[1] as f64, min[2] as f64],
        [max[0] as f64, min[1] as f64, min[2] as f64],
        [min[0] as f64, max[1] as f64, min[2] as f64],
        [max[0] as f64, max[1] as f64, min[2] as f64],
        [min[0] as f64, min[1] as f64, max[2] as f64],
        [max[0] as f64, min[1] as f64, max[2] as f64],
        [min[0] as f64, max[1] as f64, max[2] as f64],
        [max[0] as f64, max[1] as f64, max[2] as f64],
    ]
}

fn reduce_bounds(points: [[f64; 3]; 8]) -> [[f32; 3]; 2] {
    let mut bounds = [[f64::INFINITY; 3], [f64::NEG_INFINITY; 3]];
    for point in points {
        for axis in 0..3 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    bounds.map(|side| side.map(|value| value as f32))
}

#[cfg(test)]
mod transform_tests {
    use super::*;

    /// A 10 × 4 × 2 block whose origin is not at its centre.
    fn block() -> Model {
        Model {
            name: "block".into(),
            source_path: "fixture:block".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: ModelTransform::default(),
            vertices: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [10.0, 4.0, 2.0]],
            triangles: vec![[0, 1, 2]],
            bodies: vec![],
            bounds: [[0.0, 0.0, 0.0], [10.0, 4.0, 2.0]],
            import_notes: vec![],
        }
    }

    fn center(bounds: [[f32; 3]; 2]) -> [f32; 3] {
        std::array::from_fn(|axis| (bounds[0][axis] + bounds[1][axis]) * 0.5)
    }

    fn extents(bounds: [[f32; 3]; 2]) -> [f32; 3] {
        std::array::from_fn(|axis| bounds[1][axis] - bounds[0][axis])
    }

    fn turn(model: &Model, transform: ModelTransform, axis: usize, degrees: f64) -> ModelTransform {
        let bounds = model.bounds_with_transform(transform);
        let pivot = center(bounds).map(f64::from);
        transform.rotated_about_world_axis(axis, degrees, pivot)
    }

    #[test]
    fn quarter_turn_about_z_swaps_the_footprint_and_keeps_the_centre() {
        let model = block();
        let before = model.bounds_with_transform(model.transform);
        let after_transform = turn(&model, model.transform, 2, 90.0);
        let after = model.bounds_with_transform(after_transform);
        assert_eq!(extents(after), [4.0, 10.0, 2.0]);
        for axis in 0..3 {
            assert!((center(after)[axis] - center(before)[axis]).abs() < 1e-4);
        }
    }

    #[test]
    fn flipping_over_about_x_puts_the_old_bottom_on_top() {
        let model = block();
        // Vertex 0 is on the bottom face (z = 0); after a flip it is on top.
        let flipped = turn(&model, model.transform, 0, 180.0);
        let bounds = model.bounds_with_transform(flipped);
        let moved = model.vertex_with_transform(0, flipped).unwrap();
        assert!((moved[2] - bounds[1][2]).abs() < 1e-4);
        assert_eq!(extents(bounds), [10.0, 4.0, 2.0]);
    }

    #[test]
    fn four_quarter_turns_return_to_the_start() {
        let model = block();
        for axis in 0..3 {
            let mut transform = model.transform;
            for _ in 0..4 {
                transform = turn(&model, transform, axis, 90.0);
            }
            let bounds = model.bounds_with_transform(transform);
            let original = model.bounds_with_transform(model.transform);
            for side in 0..2 {
                for k in 0..3 {
                    assert!((bounds[side][k] - original[side][k]).abs() < 1e-4);
                }
            }
            assert_eq!(transform.rotation_degrees, [0.0; 3]);
        }
    }

    #[test]
    fn turns_about_different_axes_compose_in_the_world_frame() {
        let model = block();
        // X then Y quarter turns through the world axes, checked against the
        // placed vertex positions rather than the euler angles.
        let mut transform = model.transform;
        transform = turn(&model, transform, 0, 90.0);
        transform = turn(&model, transform, 1, 90.0);
        transform = turn(&model, transform, 2, 90.0);
        transform = turn(&model, transform, 0, -90.0);
        let bounds = model.bounds_with_transform(transform);
        // Every placed vertex stays inside the placed bounds, and the volume is kept.
        for index in 0..model.vertices.len() {
            let v = model.vertex_with_transform(index, transform).unwrap();
            for k in 0..3 {
                assert!(v[k] >= bounds[0][k] - 1e-4 && v[k] <= bounds[1][k] + 1e-4);
            }
        }
        let volume = |b: [[f32; 3]; 2]| extents(b).iter().product::<f32>();
        assert!((volume(bounds) - 80.0).abs() < 1e-3);
        // All angles stay exact multiples of 90°.
        assert!(transform
            .rotation_degrees
            .iter()
            .all(|angle| (angle / 90.0 - (angle / 90.0).round()).abs() < 1e-9));
    }

    #[test]
    fn stock_envelope_follows_the_previewed_orientation() {
        let model = block();
        let setup = crate::workspace::setup::MachiningSetup::default();
        let upright = model.stock_envelope(&setup, 0.0).unwrap();
        let turned = model.stock_envelope_with_transform(
            &setup,
            0.0,
            turn(&model, model.transform, 2, 90.0),
        );
        let turned = turned.unwrap();
        assert!((upright.stock_size_mm[0] - turned.stock_size_mm[1]).abs() < 1e-6);
        assert!((upright.stock_size_mm[1] - turned.stock_size_mm[0]).abs() < 1e-6);
        // The model itself was not touched.
        assert_eq!(model.transform, ModelTransform::default());
    }
}
