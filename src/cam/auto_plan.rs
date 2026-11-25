//! Automatic operation planning for an imported, stock-fitted STEP model.
//!
//! The planner looks at the part the way a CAM programmer would and picks the
//! operations it needs instead of asking the user to know them:
//!
//! 1. **Roughing** when the stock has material to remove above the part.
//! 2. **Helical bores** for every round hole found in the model. Holes are
//!    milled, never drilled (low-torque, high-RPM spindle), with a cutter size
//!    assumed from the hole diameter rather than taken from the tool library.
//! 3. **Steep-wall finishing** (waterline) and **shallow-area finishing**
//!    (raster) with a ball-nose cutter, each only where the surface (or, with
//!    the dynamic stock enabled, the material still left on it) needs it.
//!
//! With the *dynamic stock* enabled every generated path is replayed against an
//! in-process stock model: collisions of the holder with uncut stock and
//! rapids into material are reported, holes that earlier operations already
//! cleared are skipped, and finishing is only planned where stock remains. It
//! can be turned off (`MachiningSetup::dynamic_stock_check`), in which case the
//! plan falls back to the surface-slope statistics alone.

use std::collections::HashMap;

use crate::cam::{
    cancel,
    finishing::{self, FinishStrategy, SlopeWindow},
    helical::{self, BoreFeature, BoreParams, ASSUMED_BIT_SIZES_MM},
    operations::{Operation, OperationKind},
    roughing,
    stock_model::{CollisionIssue, InProcessStock},
    tool::{Tool, ToolShape},
    toolpath::Toolpath,
};
use crate::workspace::{
    mesh_query::{MeshQuery, SurfaceSample},
    model::Model,
    setup::{MachiningSetup, WorkAxis},
};

/// Material left by roughing on non-flat parts, for the finishing passes.
const ROUGHING_ALLOWANCE_MM: f64 = 0.2;
/// Surfaces flatter than this (degrees from horizontal) count as flat.
const FLAT_SLOPE_DEG: f64 = 2.0;
/// Smallest area (mm²) worth a finishing operation of its own.
const MIN_FINISH_AREA_MM2: f64 = 1.0;
/// Smallest share of the stock footprint that justifies a finishing operation.
const MIN_FINISH_FRACTION: f64 = 0.005;
/// Tool sizes tried (largest first) before an operation is given up.
const MAX_SIZE_ATTEMPTS: usize = 8;

#[derive(Debug, Clone)]
pub struct PlannerOptions {
    /// Largest roughing end mill to try (mm); smaller sizes are tried if the
    /// stock or the part does not leave room for it.
    pub roughing_tool_mm: f64,
    /// Largest ball-nose finishing cutter to try (mm).
    pub finishing_tool_mm: f64,
    /// Surfaces steeper than this (degrees from horizontal) are "steep".
    pub steep_angle_deg: f64,
    /// Stock left above the part (mm) that is still worth finishing.
    pub residual_tolerance_mm: f64,
}

impl Default for PlannerOptions {
    fn default() -> Self {
        Self {
            roughing_tool_mm: 6.0,
            finishing_tool_mm: 3.0,
            steep_angle_deg: 40.0,
            residual_tolerance_mm: 0.05,
        }
    }
}

impl PlannerOptions {
    pub fn validate(&self) -> Result<(), String> {
        let positive = |value: f64| value.is_finite() && value > 0.0;
        if !positive(self.roughing_tool_mm) || !positive(self.finishing_tool_mm) {
            return Err("planner tool sizes must be positive".into());
        }
        if !self.steep_angle_deg.is_finite() || !(5.0..=85.0).contains(&self.steep_angle_deg) {
            return Err("steep angle must be between 5 and 85 degrees".into());
        }
        if !self.residual_tolerance_mm.is_finite() || self.residual_tolerance_mm < 0.0 {
            return Err("residual tolerance cannot be negative".into());
        }
        Ok(())
    }
}

/// What the planner found out about the part.
#[derive(Debug, Clone, Default)]
pub struct ModelAnalysis {
    /// Round holes found in the model, largest first.
    pub holes: Vec<BoreFeature>,
    /// Area of the stock footprint (mm²).
    pub stock_area_mm2: f64,
    /// Part surface seen from above, by slope.
    pub flat_area_mm2: f64,
    pub shallow_area_mm2: f64,
    pub steep_area_mm2: f64,
    /// Average depth of material above the part, over the stock footprint.
    pub average_removal_depth_mm: f64,
}

pub struct PlanOutcome {
    /// Operations in machining order, with their toolpaths already generated.
    pub operations: Vec<Operation>,
    /// Human-readable findings and warnings.
    pub notes: Vec<String>,
}

/// Top-down height samples of the part over the stock footprint.
struct SurfaceGrid {
    origin: [f64; 2],
    cell_mm: f64,
    columns: usize,
    rows: usize,
    samples: Vec<Option<SurfaceSample>>,
}

impl SurfaceGrid {
    fn build(query: &MeshQuery<'_>, setup: &MachiningSetup) -> Self {
        let size = setup.stock_size_mm;
        let cell_mm = (size[0].max(size[1]) / 160.0).clamp(0.25, 2.0);
        let columns = (size[0] / cell_mm).ceil().max(1.0) as usize;
        let rows = (size[1] / cell_mm).ceil().max(1.0) as usize;
        let origin = [setup.stock_origin_mm[0], setup.stock_origin_mm[1]];
        let mut samples = Vec::with_capacity(columns * rows);
        for row in 0..rows {
            for column in 0..columns {
                let x = origin[0] + (column as f64 + 0.5) * cell_mm;
                let y = origin[1] + (row as f64 + 0.5) * cell_mm;
                samples.push(query.height_at_xy(x, y));
            }
        }
        Self {
            origin,
            cell_mm,
            columns,
            rows,
            samples,
        }
    }

    fn center(&self, column: usize, row: usize) -> [f64; 2] {
        [
            self.origin[0] + (column as f64 + 0.5) * self.cell_mm,
            self.origin[1] + (row as f64 + 0.5) * self.cell_mm,
        ]
    }

    fn cell_area(&self) -> f64 {
        self.cell_mm * self.cell_mm
    }
}

fn slope_deg(normal_z: f64) -> f64 {
    normal_z.abs().clamp(0.0, 1.0).acos().to_degrees()
}

/// Analyse a model that is already placed inside its stock.
pub fn analyze(
    model: &Model,
    setup: &MachiningSetup,
    options: &PlannerOptions,
) -> Result<ModelAnalysis, String> {
    prepare(model, setup, options)?;
    let query = MeshQuery::new(model);
    let grid = SurfaceGrid::build(&query, setup);
    Ok(analyze_grid(model, setup, options, &query, &grid))
}

fn prepare(model: &Model, setup: &MachiningSetup, options: &PlannerOptions) -> Result<(), String> {
    options.validate()?;
    setup.validate()?;
    model.validate()?;
    if setup.axis != WorkAxis::ZUp {
        return Err("automatic planning currently requires the Z-up work axis".into());
    }
    let bounds = model.transformed_bounds();
    let min = setup.stock_origin_mm;
    for axis in 0..3 {
        let lower = min[axis];
        let upper = min[axis] + setup.stock_size_mm[axis];
        if (bounds[0][axis] as f64) < lower - 1e-6 || (bounds[1][axis] as f64) > upper + 1e-6 {
            return Err(format!(
                "the model does not fit the stock on the {} axis; use Fit stock to model first",
                ["X", "Y", "Z"][axis]
            ));
        }
    }
    Ok(())
}

fn analyze_grid(
    model: &Model,
    setup: &MachiningSetup,
    options: &PlannerOptions,
    query: &MeshQuery<'_>,
    grid: &SurfaceGrid,
) -> ModelAnalysis {
    let stock_top = setup.stock_origin_mm[2] + setup.stock_size_mm[2];
    let floor = setup.stock_origin_mm[2];
    let cell_area = grid.cell_area();
    let mut analysis = ModelAnalysis {
        stock_area_mm2: grid.samples.len() as f64 * cell_area,
        ..Default::default()
    };
    let mut removed = 0.0;
    for sample in &grid.samples {
        match sample {
            Some(surface) => {
                removed += (stock_top - surface.height).max(0.0);
                let slope = slope_deg(surface.normal[2]);
                if slope < FLAT_SLOPE_DEG {
                    analysis.flat_area_mm2 += cell_area;
                } else if slope < options.steep_angle_deg {
                    analysis.shallow_area_mm2 += cell_area;
                } else {
                    analysis.steep_area_mm2 += cell_area;
                }
            }
            None => removed += stock_top - floor,
        }
    }
    analysis.average_removal_depth_mm = removed / grid.samples.len().max(1) as f64;
    analysis.holes = detect_holes(model, query);
    analysis
        .holes
        .sort_by(|a, b| b.diameter_mm.total_cmp(&a.diameter_mm));
    analysis
}

// ───────────────────────── hole detection ─────────────────────────

struct DisjointSets(Vec<usize>);

impl DisjointSets {
    fn find(&mut self, mut item: usize) -> usize {
        while self.0[item] != item {
            self.0[item] = self.0[self.0[item]];
            item = self.0[item];
        }
        item
    }

    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[b] = a;
        }
    }
}

/// Find round holes: connected groups of vertical triangles whose points lie
/// on a full circle around a void (no surface above the circle's centre).
/// Convex cylinders (pins, round outlines) have material at their centre and
/// are not holes.
pub fn detect_holes(model: &Model, query: &MeshQuery<'_>) -> Vec<BoreFeature> {
    let quantize = |point: [f64; 3]| point.map(|value| (value * 1e4).round() as i64);
    let mut ids: HashMap<[i64; 3], usize> = HashMap::new();
    let mut points: Vec<[f64; 3]> = Vec::new();
    let mut sets = DisjointSets(Vec::new());
    for triangle in &model.triangles {
        let corners: Option<Vec<[f64; 3]>> = triangle
            .iter()
            .map(|&index| {
                model
                    .transformed_vertex(index as usize)
                    .map(|v| [v[0] as f64, v[1] as f64, v[2] as f64])
            })
            .collect();
        let Some(corners) = corners else { continue };
        let u = [
            corners[1][0] - corners[0][0],
            corners[1][1] - corners[0][1],
            corners[1][2] - corners[0][2],
        ];
        let v = [
            corners[2][0] - corners[0][0],
            corners[2][1] - corners[0][1],
            corners[2][2] - corners[0][2],
        ];
        let normal = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let length = (normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2]).sqrt();
        if length <= 1e-12 || normal[2].abs() / length > 0.02 {
            continue;
        }
        let mut ring = Vec::with_capacity(3);
        for corner in &corners {
            let key = quantize(*corner);
            let id = *ids.entry(key).or_insert_with(|| {
                points.push(*corner);
                sets.0.push(sets.0.len());
                points.len() - 1
            });
            ring.push(id);
        }
        sets.union(ring[0], ring[1]);
        sets.union(ring[1], ring[2]);
    }
    let mut groups: HashMap<usize, Vec<[f64; 3]>> = HashMap::new();
    for (id, point) in points.iter().enumerate() {
        let root = sets.find(id);
        groups.entry(root).or_default().push(*point);
    }
    let mut holes = Vec::new();
    for group in groups.values() {
        if let Some(hole) = hole_from_wall(group, query) {
            holes.push(hole);
        }
    }
    // Stable order for reproducible plans.
    holes.sort_by(|a, b| {
        b.diameter_mm
            .total_cmp(&a.diameter_mm)
            .then(a.center[0].total_cmp(&b.center[0]))
            .then(a.center[1].total_cmp(&b.center[1]))
    });
    holes
}

fn hole_from_wall(wall: &[[f64; 3]], query: &MeshQuery<'_>) -> Option<BoreFeature> {
    if wall.len() < 8 {
        return None;
    }
    let (center, radius) = fit_circle(wall)?;
    if radius < 0.1 {
        return None;
    }
    // Every wall point must sit on the circle.
    let tolerance = (radius * 0.02).max(0.03);
    if wall
        .iter()
        .any(|p| ((p[0] - center[0]).hypot(p[1] - center[1]) - radius).abs() > tolerance)
    {
        return None;
    }
    // The wall must go all the way around (largest angular gap ≤ 45°).
    let mut angles: Vec<f64> = wall
        .iter()
        .map(|p| (p[1] - center[1]).atan2(p[0] - center[0]))
        .collect();
    angles.sort_by(f64::total_cmp);
    let mut largest_gap = angles[0] + std::f64::consts::TAU - angles[angles.len() - 1];
    for pair in angles.windows(2) {
        largest_gap = largest_gap.max(pair[1] - pair[0]);
    }
    if largest_gap > std::f64::consts::FRAC_PI_4 {
        return None;
    }
    let (bottom, top) = wall
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), p| {
            (low.min(p[2]), high.max(p[2]))
        });
    if top - bottom < 0.05 {
        return None;
    }
    // A hole is a void: nothing may cover the axis above the mid-height.
    let middle = (top + bottom) * 0.5;
    if query
        .surface_samples_at_xy(center[0], center[1])
        .iter()
        .any(|surface| surface.height > middle + 1e-6)
    {
        return None;
    }
    Some(BoreFeature {
        center,
        diameter_mm: radius * 2.0,
        top_z: top,
        bottom_z: bottom,
    })
}

/// Least-squares circle through XY points (Kåsa fit on centred coordinates).
fn fit_circle(points: &[[f64; 3]]) -> Option<([f64; 2], f64)> {
    let count = points.len() as f64;
    let mean = [
        points.iter().map(|p| p[0]).sum::<f64>() / count,
        points.iter().map(|p| p[1]).sum::<f64>() / count,
    ];
    let (mut suu, mut suv, mut svv) = (0.0, 0.0, 0.0);
    let (mut suuu, mut svvv, mut suvv, mut svuu) = (0.0, 0.0, 0.0, 0.0);
    for p in points {
        let (u, v) = (p[0] - mean[0], p[1] - mean[1]);
        suu += u * u;
        suv += u * v;
        svv += v * v;
        suuu += u * u * u;
        svvv += v * v * v;
        suvv += u * v * v;
        svuu += v * u * u;
    }
    let determinant = suu * svv - suv * suv;
    if determinant.abs() < 1e-12 {
        return None;
    }
    let right = [0.5 * (suuu + suvv), 0.5 * (svvv + svuu)];
    let uc = (right[0] * svv - right[1] * suv) / determinant;
    let vc = (right[1] * suu - right[0] * suv) / determinant;
    let radius = (uc * uc + vc * vc + (suu + svv) / count).sqrt();
    (radius.is_finite()).then_some(([mean[0] + uc, mean[1] + vc], radius))
}

// ───────────────────────── tooling ─────────────────────────

/// An assumed tool (not read from the tool library) with feeds taken from the
/// setup's chip load. Spindle speeds are high because the machine is a
/// low-torque, high-RPM router.
pub fn assumed_tool(
    name: &str,
    diameter_mm: f64,
    shape: ToolShape,
    reach_mm: f64,
    setup: &MachiningSetup,
) -> Tool {
    let rpm = if diameter_mm <= 3.0 {
        18_000.0
    } else {
        12_000.0
    };
    let chip_load = setup.chip_load_mm * (diameter_mm / 6.0).clamp(0.25, 1.0);
    let feed = (rpm * 2.0 * chip_load).clamp(100.0, 3_000.0);
    let flute = reach_mm.max(diameter_mm * 2.0);
    Tool {
        name: name.into(),
        description: "Assumed by the automatic planner (not from the tool library)".into(),
        diameter_mm,
        feed_rate_mm_min: feed,
        plunge_rate_mm_min: (feed * 0.3).max(50.0),
        spindle_rpm: rpm,
        shape,
        flute_length_mm: flute,
        stickout_mm: flute + 10.0,
        corner_radius_mm: 0.0,
        holder_clearance_mm: 0.0,
    }
}

/// Assumed sizes no larger than `limit`, largest first.
fn sizes_up_to(limit: f64) -> Vec<f64> {
    ASSUMED_BIT_SIZES_MM
        .iter()
        .rev()
        .copied()
        .filter(|size| *size <= limit + 1e-9)
        .take(MAX_SIZE_ATTEMPTS)
        .collect()
}

/// Failures that are only found after a whole path was generated and checked.
/// Retrying with a smaller cutter repeats all that work, so only a few are allowed.
fn is_late_failure(error: &str) -> bool {
    error.contains("gouge") || error.contains("collision")
}

/// Late (expensive) failures tolerated before giving up on an operation.
const MAX_LATE_FAILURES: usize = 2;

/// Try tool sizes from the largest down until one generates a path. Stops at
/// once when the job is cancelled.
fn first_size_that_works<T>(
    limit: f64,
    mut attempt: impl FnMut(f64) -> Result<T, String>,
) -> Result<(f64, T), String> {
    let mut last = format!("no assumed tool size is at most {limit:.1} mm");
    let mut late_failures = 0;
    for size in sizes_up_to(limit) {
        cancel::check()?;
        match attempt(size) {
            Ok(value) => return Ok((size, value)),
            Err(error) if cancel::was_cancelled(&error) => return Err(error),
            Err(error) => {
                last = format!("{size:.1} mm: {error}");
                if is_late_failure(&error) {
                    late_failures += 1;
                    if late_failures >= MAX_LATE_FAILURES {
                        break;
                    }
                }
            }
        }
    }
    Err(last)
}

/// Setup used when generating the planned finishing passes: the waterline Z
/// step is made fine for a finishing cutter.
pub fn finishing_setup(setup: &MachiningSetup, tool: &Tool) -> MachiningSetup {
    let mut finishing = setup.clone();
    finishing.stepdown_mm = (tool.diameter_mm * 0.2).clamp(0.1, setup.stepdown_mm);
    finishing
}

/// Finishing stepover as a fraction of the (ball-nose) tool diameter.
pub const FINISH_STEPOVER_RATIO: f64 = 0.15;

/// Generate the toolpath of a 3D finishing operation. Operations made by the
/// planner keep their slope window and fine step sizes; hand-made ones use the
/// project setup as before.
pub fn generate_finishing_operation(
    model: &Model,
    setup: &MachiningSetup,
    operation: &Operation,
) -> Result<Toolpath, String> {
    let strategy = match operation.kind {
        OperationKind::FinishRaster3D => FinishStrategy::Raster,
        OperationKind::FinishContour3D => FinishStrategy::ContourParallel,
        OperationKind::FinishWaterline3D => FinishStrategy::Waterline,
        _ => return Err("not a 3D finishing operation".into()),
    };
    let window = operation
        .slope_window
        .map(|[min_deg, max_deg]| SlopeWindow { min_deg, max_deg })
        .unwrap_or(SlopeWindow::ALL);
    let (finish_setup, stepover) = if operation.auto_planned {
        (
            finishing_setup(setup, &operation.tool),
            operation.tool.diameter_mm * operation.stepover_ratio,
        )
    } else {
        (
            setup.clone(),
            operation.tool.diameter_mm * setup.stepover_percent / 100.0,
        )
    };
    finishing::generate_scoped(
        model,
        &finish_setup,
        &operation.tool,
        strategy,
        stepover,
        operation.margin_mm,
        window,
    )
}

fn operation_from(
    name: String,
    kind: OperationKind,
    tool: Tool,
    setup: &MachiningSetup,
    total_depth_mm: f64,
    margin_mm: f64,
    path: Toolpath,
) -> Operation {
    let mut operation = Operation::placeholder();
    operation.name = name;
    operation.kind = kind;
    operation.tool = tool;
    operation.depth_per_pass_mm = setup.stepdown_mm;
    operation.total_depth_mm = total_depth_mm.max(0.1);
    operation.margin_mm = margin_mm;
    operation.generated = !path.is_empty();
    operation.toolpath = path;
    operation.auto_planned = true;
    operation
}

// ───────────────────────── residual stock ─────────────────────────

/// Stock still left on shallow and steep areas (mm²), measured against the
/// part surface seen from above. Stock beside a wall counts as steep.
fn residual_areas(
    stock: &InProcessStock,
    grid: &SurfaceGrid,
    options: &PlannerOptions,
) -> (f64, f64) {
    let cell_area = grid.cell_area();
    let (mut shallow, mut steep) = (0.0, 0.0);
    for row in 0..grid.rows {
        for column in 0..grid.columns {
            let [x, y] = grid.center(column, row);
            let height = stock.height_at(x, y);
            match &grid.samples[row * grid.columns + column] {
                Some(surface) => {
                    if height - surface.height > options.residual_tolerance_mm {
                        if slope_deg(surface.normal[2]) < options.steep_angle_deg {
                            shallow += cell_area;
                        } else {
                            steep += cell_area;
                        }
                    }
                }
                None => {
                    if height - stock.floor_z() > options.residual_tolerance_mm {
                        steep += cell_area;
                    }
                }
            }
        }
    }
    (shallow, steep)
}

// ───────────────────────── the planner ─────────────────────────

/// Plan the operations for `model` and generate every toolpath.
pub fn plan(
    model: &Model,
    setup: &MachiningSetup,
    options: &PlannerOptions,
) -> Result<PlanOutcome, String> {
    cancel::progress("Analysing the model…");
    prepare(model, setup, options)?;
    let query = MeshQuery::new(model);
    let grid = SurfaceGrid::build(&query, setup);
    cancel::check()?;
    let analysis = analyze_grid(model, setup, options, &query, &grid);
    cancel::check()?;
    let dynamic = setup.dynamic_stock_check;
    let mut stock = InProcessStock::from_setup(setup)?;
    let mut notes = vec![format!(
        "Analysis: {:.0} mm² flat, {:.0} mm² shallow, {:.0} mm² steep; {:.1} mm of stock to remove on average; {} round hole(s).",
        analysis.flat_area_mm2,
        analysis.shallow_area_mm2,
        analysis.steep_area_mm2,
        analysis.average_removal_depth_mm,
        analysis.holes.len()
    )];
    notes.push(if dynamic {
        "Dynamic stock collision checking is ON.".into()
    } else {
        "Dynamic stock collision checking is OFF; the plan uses surface slopes only.".into()
    });

    let mut operations: Vec<Operation> = Vec::new();
    let stock_depth = setup.stock_size_mm[2];
    let model_bottom = model.transformed_bounds()[0][2] as f64;
    let stock_top = setup.stock_origin_mm[2] + stock_depth;
    let depth_into_stock = (stock_top - model_bottom).max(0.1);

    // 1 ── roughing ───────────────────────────────────────────────────────
    let non_flat = analysis.shallow_area_mm2 + analysis.steep_area_mm2;
    let allowance = if non_flat > analysis.stock_area_mm2 * 0.01 {
        ROUGHING_ALLOWANCE_MM
    } else {
        0.0
    };
    if analysis.average_removal_depth_mm >= 0.1 {
        let stage = std::time::Instant::now();
        let attempt = first_size_that_works(options.roughing_tool_mm, |diameter| {
            cancel::progress(format!("Roughing with a {diameter} mm end mill…"));
            let tool = assumed_tool(
                &format!("Rough end mill {diameter} mm"),
                diameter,
                ToolShape::Flat,
                stock_depth + 1.0,
                setup,
            );
            let path = roughing::generate(model, setup, &tool, allowance, None)?;
            Ok((tool, path))
        });
        match attempt {
            Ok((diameter, (tool, path))) => {
                notes.push(format!(
                    "Roughing took {:.1} s ({} moves).",
                    stage.elapsed().as_secs_f64(),
                    path.moves.len()
                ));
                if diameter + 1e-9 < options.roughing_tool_mm {
                    notes.push(format!(
                        "Roughing cutter reduced to {diameter} mm so it fits the stock margin and the part."
                    ));
                }
                record_collisions(&mut notes, "3D roughing", &mut stock, &path, &tool, dynamic);
                operations.push(operation_from(
                    format!("Rough · Ø{diameter} flat"),
                    OperationKind::Roughing3D,
                    tool,
                    setup,
                    depth_into_stock,
                    allowance,
                    path,
                ));
            }
            Err(error) if cancel::was_cancelled(&error) => return Err(error),
            Err(error) => notes.push(format!("⚠ Roughing skipped: {error}")),
        }
    } else {
        notes.push("Roughing not needed: the part fills the stock.".into());
    }

    // 2 ── helical bores ──────────────────────────────────────────────────
    cancel::check()?;
    cancel::progress(format!(
        "Planning {} helical bore(s)…",
        analysis.holes.len()
    ));
    plan_bores(
        &analysis.holes,
        setup,
        dynamic,
        &mut stock,
        &mut operations,
        &mut notes,
    );

    // 3 ── finishing ──────────────────────────────────────────────────────
    let (need_steep, need_shallow) = if dynamic {
        let (shallow, steep) = residual_areas(&stock, &grid, options);
        notes.push(format!(
            "Stock left after roughing and boring: {shallow:.0} mm² on shallow areas, {steep:.0} mm² on steep walls."
        ));
        (
            worth_finishing(steep, analysis.stock_area_mm2),
            worth_finishing(shallow, analysis.stock_area_mm2),
        )
    } else {
        (
            worth_finishing(analysis.steep_area_mm2, analysis.stock_area_mm2),
            worth_finishing(
                analysis.shallow_area_mm2 + analysis.flat_area_mm2,
                analysis.stock_area_mm2,
            ),
        )
    };
    let finish_passes = [
        (
            need_steep,
            FinishStrategy::Waterline,
            OperationKind::FinishWaterline3D,
            "steep walls",
            "waterline",
            SlopeWindow {
                min_deg: options.steep_angle_deg,
                max_deg: 90.0,
            },
        ),
        (
            need_shallow,
            FinishStrategy::Raster,
            OperationKind::FinishRaster3D,
            "shallow areas",
            "raster",
            SlopeWindow {
                min_deg: 0.0,
                max_deg: options.steep_angle_deg,
            },
        ),
    ];
    for (needed, strategy, kind, label, how, window) in finish_passes {
        if !needed {
            notes.push(format!("No finishing needed on {label}."));
            continue;
        }
        let stage = std::time::Instant::now();
        let attempt = first_size_that_works(options.finishing_tool_mm, |diameter| {
            cancel::progress(format!("Finishing {label} with a {diameter} mm ball nose…"));
            let tool = assumed_tool(
                &format!("Ball nose {diameter} mm"),
                diameter,
                ToolShape::BallNose,
                depth_into_stock + 1.0,
                setup,
            );
            let finishing_setup = finishing_setup(setup, &tool);
            let stepover = (diameter * FINISH_STEPOVER_RATIO).max(0.05);
            let path = finishing::generate_scoped(
                model,
                &finishing_setup,
                &tool,
                strategy,
                stepover,
                0.0,
                window,
            )?;
            Ok((tool, path))
        });
        match attempt {
            Ok((diameter, (tool, path))) => {
                notes.push(format!(
                    "Finishing {label} took {:.1} s ({} moves).",
                    stage.elapsed().as_secs_f64(),
                    path.moves.len()
                ));
                record_collisions(&mut notes, label, &mut stock, &path, &tool, dynamic);
                let mut operation = operation_from(
                    format!("Finish {label} · Ø{diameter} ball ({how})"),
                    kind,
                    tool,
                    setup,
                    depth_into_stock,
                    0.0,
                    path,
                );
                operation.stepover_ratio = FINISH_STEPOVER_RATIO;
                operation.slope_window = Some([window.min_deg, window.max_deg]);
                operations.push(operation);
            }
            Err(error) if cancel::was_cancelled(&error) => return Err(error),
            Err(error) => notes.push(format!("⚠ Finishing of {label} skipped: {error}")),
        }
    }

    if operations.is_empty() {
        notes.push("No operations were needed for this part.".into());
    }
    Ok(PlanOutcome { operations, notes })
}

fn worth_finishing(area_mm2: f64, stock_area_mm2: f64) -> bool {
    area_mm2 >= MIN_FINISH_AREA_MM2.max(stock_area_mm2 * MIN_FINISH_FRACTION)
}

/// Replay a path on the stock and note any collisions found.
fn record_collisions(
    notes: &mut Vec<String>,
    label: &str,
    stock: &mut InProcessStock,
    path: &Toolpath,
    tool: &Tool,
    check: bool,
) {
    let issues = stock.simulate(path, tool, check);
    report_issues(notes, label, &issues);
}

pub fn report_issues(notes: &mut Vec<String>, label: &str, issues: &[CollisionIssue]) {
    if issues.is_empty() {
        return;
    }
    notes.push(format!(
        "⚠ {label}: {} possible stock collision(s) found by the dynamic stock check.",
        issues.len()
    ));
    notes.extend(
        issues
            .iter()
            .take(3)
            .map(|issue| format!("    {}", issue.describe())),
    );
}

/// Group holes by assumed cutter, largest cutter first so counterbores are
/// milled before the smaller bore that continues below them.
fn plan_bores(
    holes: &[BoreFeature],
    setup: &MachiningSetup,
    dynamic: bool,
    stock: &mut InProcessStock,
    operations: &mut Vec<Operation>,
    notes: &mut Vec<String>,
) {
    let stock_top = stock.top_z();
    let stock_floor = stock.floor_z();
    let mut groups: Vec<(f64, Vec<BoreFeature>)> = Vec::new();
    for hole in holes {
        let Some(bit) = helical::assumed_bit_diameter(hole.diameter_mm) else {
            notes.push(format!(
                "⚠ Hole Ø{:.2} mm at X{:.2} Y{:.2} is below the {:.1} mm minimum for helical boring; it is not drilled (low-torque spindle).",
                hole.diameter_mm,
                hole.center[0],
                hole.center[1],
                helical::min_bore_diameter_mm()
            ));
            continue;
        };
        let mut feature = *hole;
        feature.top_z = feature.top_z.min(stock_top);
        feature.bottom_z = feature.bottom_z.max(stock_floor);
        if dynamic {
            // Start where material still is; skip what roughing already cleared.
            let radius = (feature.diameter_mm * 0.5 - 0.01).max(0.01);
            let remaining =
                stock.max_height_in_disc_exact(feature.center[0], feature.center[1], radius);
            if remaining <= feature.bottom_z + 0.05 {
                notes.push(format!(
                    "Hole Ø{:.2} mm at X{:.2} Y{:.2} was already cleared by earlier operations.",
                    feature.diameter_mm, feature.center[0], feature.center[1]
                ));
                continue;
            }
            feature.top_z = feature.top_z.min(remaining);
        }
        if feature.depth_mm() <= 1e-3 {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(size, _)| (*size - bit).abs() < 1e-9)
        {
            Some((_, list)) => list.push(feature),
            None => groups.push((bit, vec![feature])),
        }
    }
    groups.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (bit, mut features) in groups {
        features.sort_by(|a, b| b.top_z.total_cmp(&a.top_z));
        let deepest = features
            .iter()
            .map(BoreFeature::depth_mm)
            .fold(0.0_f64, f64::max);
        let tool = assumed_tool(
            &format!("Bore end mill {bit} mm"),
            bit,
            ToolShape::Flat,
            deepest + 1.0,
            setup,
        );
        let params = BoreParams {
            stepdown_mm: setup.stepdown_mm,
            stepover_ratio: (setup.stepover_percent / 100.0).min(0.5),
            path_tolerance_mm: setup.path_tolerance_mm,
        };
        match helical::generate_bores(&features, &tool, &params) {
            Ok(path) => {
                let label = format!("Helical bore Ø{bit}");
                record_collisions(notes, &label, stock, &path, &tool, dynamic);
                let mut operation = operation_from(
                    format!("{label} · {} hole(s)", features.len()),
                    OperationKind::HelicalBore,
                    tool,
                    setup,
                    deepest,
                    0.0,
                    path,
                );
                operation.stepover_ratio = params.stepover_ratio;
                operation.bores = features;
                operations.push(operation);
            }
            Err(error) => notes.push(format!("⚠ Helical bore Ø{bit} skipped: {error}")),
        }
    }
}

/// Collisions of one path against the stock left by the operations before it.
/// Used when a single operation is (re)generated by hand.
pub fn check_against_earlier_operations(
    setup: &MachiningSetup,
    earlier: &[&Operation],
    tool: &Tool,
    path: &Toolpath,
) -> Result<Vec<CollisionIssue>, String> {
    let mut stock = InProcessStock::from_setup(setup)?;
    for operation in earlier {
        if operation.generated && !operation.excluded_from_toolpath {
            stock.apply_path(&operation.toolpath, &operation.tool);
        }
    }
    Ok(stock.simulate(path, tool, true))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::model::{MeshBody, StepLengthUnit};

    fn model_from(vertices: Vec<[f32; 3]>, triangles: Vec<[u32; 3]>) -> Model {
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for vertex in &vertices {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex[axis]);
                max[axis] = max[axis].max(vertex[axis]);
            }
        }
        let triangle_count = triangles.len();
        Model {
            name: "fixture".into(),
            source_path: "fixture:plan".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: Default::default(),
            vertices,
            triangles,
            bodies: vec![MeshBody {
                name: "fixture".into(),
                source_item_id: 1,
                first_triangle: 0,
                triangle_count,
                triangle_face_ids: (1..=triangle_count as u64).collect(),
                transform: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
                bounds: [min, max],
            }],
            bounds: [min, max],
            import_notes: vec![],
        }
    }

    /// A cylinder side wall (no caps) split into `segments` quads.
    fn cylinder_wall(
        vertices: &mut Vec<[f32; 3]>,
        triangles: &mut Vec<[u32; 3]>,
        center: [f32; 2],
        radius: f32,
        z: [f32; 2],
        segments: usize,
    ) {
        let base = vertices.len() as u32;
        for step in 0..segments {
            let angle = step as f32 * std::f32::consts::TAU / segments as f32;
            let (x, y) = (
                center[0] + radius * angle.cos(),
                center[1] + radius * angle.sin(),
            );
            vertices.push([x, y, z[0]]);
            vertices.push([x, y, z[1]]);
        }
        for step in 0..segments as u32 {
            let next = (step + 1) % segments as u32;
            let (a, b) = (base + step * 2, base + step * 2 + 1);
            let (c, d) = (base + next * 2, base + next * 2 + 1);
            triangles.push([a, c, d]);
            triangles.push([a, d, b]);
        }
    }

    /// A flat plate top at height `top` over [lo, hi]², with an optional round
    /// opening whose wall is also modelled.
    fn plate(lo: f32, hi: f32, top: f32, hole: Option<([f32; 2], f32)>) -> Model {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        let cells = ((hi - lo) / 1.0) as usize;
        for row in 0..cells {
            for column in 0..cells {
                let (x0, y0) = (lo + column as f32, lo + row as f32);
                let (cx, cy) = (x0 + 0.5, y0 + 0.5);
                if let Some((center, radius)) = hole {
                    if (cx - center[0]).hypot(cy - center[1]) < radius + 0.75 {
                        continue;
                    }
                }
                let base = vertices.len() as u32;
                vertices.extend([
                    [x0, y0, top],
                    [x0 + 1.0, y0, top],
                    [x0 + 1.0, y0 + 1.0, top],
                    [x0, y0 + 1.0, top],
                ]);
                triangles.push([base, base + 1, base + 2]);
                triangles.push([base, base + 2, base + 3]);
            }
        }
        if let Some((center, radius)) = hole {
            cylinder_wall(
                &mut vertices,
                &mut triangles,
                center,
                radius,
                [0.0, top],
                32,
            );
        }
        model_from(vertices, triangles)
    }

    fn stock_setup() -> MachiningSetup {
        MachiningSetup {
            stock_size_mm: [40.0, 40.0, 10.0],
            stock_origin_mm: [0.0, 0.0, 0.0],
            boundary_margin_mm: 1.0,
            // Coarse sampling keeps the tests fast in debug builds.
            path_tolerance_mm: 0.5,
            ..MachiningSetup::default()
        }
    }

    #[test]
    fn circle_fit_recovers_centre_and_radius() {
        let points: Vec<[f64; 3]> = (0..24)
            .map(|step| {
                let angle = step as f64 * std::f64::consts::TAU / 24.0;
                [5.0 + 3.0 * angle.cos(), -2.0 + 3.0 * angle.sin(), 0.0]
            })
            .collect();
        let (center, radius) = fit_circle(&points).unwrap();
        assert!((center[0] - 5.0).abs() < 1e-9 && (center[1] + 2.0).abs() < 1e-9);
        assert!((radius - 3.0).abs() < 1e-9);
    }

    #[test]
    fn a_round_wall_around_a_void_is_found_as_a_hole() {
        let model = plate(5.0, 35.0, 7.0, Some(([20.0, 20.0], 4.0)));
        let query = MeshQuery::new(&model);
        let holes = detect_holes(&model, &query);
        assert_eq!(holes.len(), 1);
        let hole = holes[0];
        assert!((hole.center[0] - 20.0).abs() < 0.01 && (hole.center[1] - 20.0).abs() < 0.01);
        assert!((hole.diameter_mm - 8.0).abs() < 0.02);
        assert!((hole.top_z - 7.0).abs() < 1e-6 && hole.bottom_z.abs() < 1e-6);
    }

    #[test]
    fn a_round_pin_with_a_cap_is_not_a_hole() {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        cylinder_wall(
            &mut vertices,
            &mut triangles,
            [10.0, 10.0],
            3.0,
            [0.0, 6.0],
            24,
        );
        // Cap over the pin.
        let base = vertices.len() as u32;
        vertices.extend([
            [7.0, 7.0, 6.0],
            [13.0, 7.0, 6.0],
            [13.0, 13.0, 6.0],
            [7.0, 13.0, 6.0],
        ]);
        triangles.push([base, base + 1, base + 2]);
        triangles.push([base, base + 2, base + 3]);
        let model = model_from(vertices, triangles);
        let query = MeshQuery::new(&model);
        assert!(detect_holes(&model, &query).is_empty());
    }

    #[test]
    fn a_wavy_wall_is_not_a_round_hole() {
        let mut vertices = Vec::new();
        let mut triangles = Vec::new();
        // A square ring wall has vertical faces but is not circular.
        let corners = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        for step in 0..4 {
            let (a, b) = (corners[step], corners[(step + 1) % 4]);
            let base = vertices.len() as u32;
            vertices.extend([
                [a[0], a[1], 0.0],
                [b[0], b[1], 0.0],
                [b[0], b[1], 5.0],
                [a[0], a[1], 5.0],
            ]);
            triangles.push([base, base + 1, base + 2]);
            triangles.push([base, base + 2, base + 3]);
        }
        let model = model_from(vertices, triangles);
        let query = MeshQuery::new(&model);
        assert!(detect_holes(&model, &query).is_empty());
    }

    #[test]
    fn analysis_measures_flat_area_and_material_to_remove() {
        let model = plate(5.0, 35.0, 7.0, None);
        let analysis = analyze(&model, &stock_setup(), &PlannerOptions::default()).unwrap();
        assert!(analysis.flat_area_mm2 > 800.0 && analysis.flat_area_mm2 < 1000.0);
        assert_eq!(analysis.steep_area_mm2, 0.0);
        assert!(analysis.average_removal_depth_mm > 3.0);
        assert!(analysis.holes.is_empty());
    }

    #[test]
    fn model_outside_the_stock_is_rejected_with_a_hint() {
        let model = plate(5.0, 45.0, 7.0, None);
        let error = analyze(&model, &stock_setup(), &PlannerOptions::default()).unwrap_err();
        assert!(error.contains("Fit stock to model"));
    }

    #[test]
    fn plan_roughs_then_finishes_the_leftover_plateau() {
        let model = plate(8.0, 32.0, 7.0, None);
        let outcome = plan(&model, &stock_setup(), &PlannerOptions::default()).unwrap();
        let kinds: Vec<_> = outcome
            .operations
            .iter()
            .map(|op| op.kind.clone())
            .collect();
        assert_eq!(
            kinds.first(),
            Some(&OperationKind::Roughing3D),
            "{:?}",
            outcome.notes
        );
        assert!(
            kinds.contains(&OperationKind::FinishRaster3D),
            "{:?}",
            outcome.notes
        );
        for operation in &outcome.operations {
            assert!(operation.auto_planned && operation.generated);
            assert!(operation.tool.validate().is_ok());
        }
        // Finishing uses a ball-nose cutter restricted to shallow slopes.
        let raster = outcome
            .operations
            .iter()
            .find(|op| op.kind == OperationKind::FinishRaster3D)
            .unwrap();
        assert_eq!(raster.tool.shape, ToolShape::BallNose);
        assert_eq!(raster.slope_window.unwrap()[0], 0.0);
    }

    #[test]
    fn plan_mills_holes_helically_and_never_drills_them() {
        let model = plate(8.0, 32.0, 7.0, Some(([20.0, 20.0], 4.0)));
        let outcome = plan(&model, &stock_setup(), &PlannerOptions::default()).unwrap();
        let bore = outcome
            .operations
            .iter()
            .find(|op| op.kind == OperationKind::HelicalBore)
            .unwrap_or_else(|| panic!("no bore planned: {:?}", outcome.notes));
        assert_eq!(bore.bores.len(), 1);
        assert!(bore.tool.diameter_mm < 8.0 && bore.tool.diameter_mm >= 0.8);
        assert!(bore
            .toolpath
            .moves
            .iter()
            .all(|m| !matches!(m, crate::cam::toolpath::ToolMove::Drill { .. })));
        assert!(outcome
            .operations
            .iter()
            .all(|op| op.kind != OperationKind::Drill));
        // Bores come after roughing and before finishing.
        let kinds: Vec<_> = outcome
            .operations
            .iter()
            .map(|op| op.kind.clone())
            .collect();
        let bore_at = kinds
            .iter()
            .position(|k| *k == OperationKind::HelicalBore)
            .unwrap();
        let rough_at = kinds
            .iter()
            .position(|k| *k == OperationKind::Roughing3D)
            .unwrap();
        assert!(rough_at < bore_at);
    }

    #[test]
    fn holes_that_are_too_small_are_reported_not_drilled() {
        let model = plate(8.0, 32.0, 7.0, Some(([20.0, 20.0], 0.5)));
        let outcome = plan(&model, &stock_setup(), &PlannerOptions::default()).unwrap();
        assert!(outcome
            .operations
            .iter()
            .all(|op| op.kind != OperationKind::HelicalBore && op.kind != OperationKind::Drill));
        assert!(outcome
            .notes
            .iter()
            .any(|note| note.contains("not drilled")));
    }

    #[test]
    fn dynamic_stock_check_can_be_disabled() {
        let model = plate(8.0, 32.0, 7.0, None);
        let mut setup = stock_setup();
        setup.dynamic_stock_check = false;
        let outcome = plan(&model, &setup, &PlannerOptions::default()).unwrap();
        assert!(outcome.notes.iter().any(|note| note.contains("OFF")));
        assert!(!outcome
            .notes
            .iter()
            .any(|note| note.contains("collision(s)")));
        assert!(!outcome.operations.is_empty());
    }

    #[test]
    fn a_cancelled_plan_stops_immediately_and_changes_nothing() {
        let model = plate(8.0, 32.0, 7.0, None);
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let _control = cancel::install(cancel::Control {
            cancel,
            progress: None,
        });
        let started = std::time::Instant::now();
        let error = plan(&model, &stock_setup(), &PlannerOptions::default())
            .err()
            .expect("cancelled plans return an error");
        assert!(cancel::was_cancelled(&error), "{error}");
        assert!(started.elapsed().as_secs() < 5);
    }

    #[test]
    fn the_planner_reports_progress_and_is_not_slowed_by_fine_path_tolerances() {
        let model = plate(8.0, 32.0, 7.0, None);
        // The default 0.02 mm path tolerance used to make the gouge checks sample
        // every 0.02 mm, which stalled the planner on real parts.
        let mut setup = stock_setup();
        setup.path_tolerance_mm = 0.02;
        let (sender, receiver) = std::sync::mpsc::channel();
        let _control = cancel::install(cancel::Control {
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            progress: Some(sender),
        });
        let started = std::time::Instant::now();
        let outcome = plan(&model, &setup, &PlannerOptions::default()).unwrap();
        assert!(!outcome.operations.is_empty());
        assert!(
            started.elapsed().as_secs() < 30,
            "planning took {:?}",
            started.elapsed()
        );
        let messages: Vec<String> = receiver.try_iter().collect();
        assert!(
            messages.iter().any(|m| m.contains("Analysing")),
            "{messages:?}"
        );
        assert!(
            messages.iter().any(|m| m.contains("Roughing")),
            "{messages:?}"
        );
    }

    #[test]
    fn each_operation_gets_a_different_colour() {
        use crate::cam::operations::{operation_color, OPERATION_COLORS};
        let colors: std::collections::HashSet<[u32; 3]> = (0..OPERATION_COLORS.len())
            .map(|index| operation_color(index).map(f32::to_bits))
            .collect();
        assert_eq!(colors.len(), OPERATION_COLORS.len());
    }

    #[test]
    fn assumed_tools_are_not_library_tools_and_validate() {
        let setup = MachiningSetup::default();
        let tool = assumed_tool("Bore", 3.0, ToolShape::Flat, 12.0, &setup);
        assert!(tool.validate().is_ok());
        assert!(tool.description.contains("Assumed"));
        assert!(tool.spindle_rpm >= 12_000.0);
        assert!(tool.flute_length_mm >= 12.0);
    }
}
