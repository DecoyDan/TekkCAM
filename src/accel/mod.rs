//! Hardware acceleration for toolpath generation and stock simulation.
//!
//! Two jobs dominate the run time of 3-axis STEP machining and are offloaded to
//! the GPU through wgpu compute shaders, with a CPU path that gives the same
//! answers:
//!
//! * **Probe tests** ("is the surface under this cutter or holder no higher
//!   than Z?") used by roughing and by every gouge/holder check. The GPU builds
//!   conservative per-cell height bounds of the mesh and decides the great
//!   majority of probes on its own; anything it cannot decide for certain is
//!   answered by the exact mesh query on the CPU, so the result is identical to
//!   the CPU-only result. The CPU path itself runs on all cores.
//! * **Stock removal**: sweeping the cutter along every move to lower a
//!   height-field, used by the viewport's stock preview and the in-process
//!   stock model.
//!
//! The GPU is optional: without an adapter, with the switch turned off, or
//! after any GPU error, everything runs on the CPU.

mod gpu;

use std::f64::consts::TAU;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, RwLock,
};

pub use gpu::{GpuBounds, GpuContext};

use crate::workspace::mesh_query::MeshQuery;

/// Probes below this count are cheaper to answer on the CPU than to upload.
const GPU_MIN_PROBES: usize = 2_000;
/// Meshes smaller than this are not worth building GPU bounds for.
const GPU_MIN_TRIANGLES: usize = 500;
/// Stock cuts touching fewer grid cells than this stay on the CPU.
const GPU_MIN_CUT_CELLS: usize = 150_000;

// ───────────────────────── global GPU state ─────────────────────────

static GPU: RwLock<Option<Arc<GpuContext>>> = RwLock::new(None);
static GPU_ENABLED: AtomicBool = AtomicBool::new(true);
static GPU_FAILURE: RwLock<Option<String>> = RwLock::new(None);

/// Share the application's wgpu device with the acceleration layer.
pub fn install_shared(
    device: egui_wgpu::wgpu::Device,
    queue: egui_wgpu::wgpu::Queue,
    adapter_name: String,
) -> Result<String, String> {
    let context = GpuContext::from_device(device, queue, adapter_name)?;
    let name = context.adapter_name().to_string();
    *GPU.write().expect("gpu lock") = Some(context);
    *GPU_FAILURE.write().expect("gpu lock") = None;
    Ok(name)
}

/// Open a GPU of our own (command line tools, tests). Returns its name.
pub fn install_headless() -> Result<String, String> {
    let context = GpuContext::headless()?;
    let name = context.adapter_name().to_string();
    *GPU.write().expect("gpu lock") = Some(context);
    *GPU_FAILURE.write().expect("gpu lock") = None;
    Ok(name)
}

/// The user-facing switch ("Use GPU acceleration").
pub fn set_gpu_enabled(enabled: bool) {
    GPU_ENABLED.store(enabled, Ordering::Relaxed);
}

pub fn gpu_enabled() -> bool {
    GPU_ENABLED.load(Ordering::Relaxed)
}

/// The GPU, if one is installed, enabled and has not failed.
pub fn active_gpu() -> Option<Arc<GpuContext>> {
    if !gpu_enabled() {
        return None;
    }
    GPU.read().expect("gpu lock").clone()
}

/// Turn the GPU off after an error; work continues on the CPU.
pub fn report_gpu_failure(error: &str) {
    log::warn!("GPU acceleration disabled: {error}");
    *GPU_FAILURE.write().expect("gpu lock") = Some(error.to_string());
    *GPU.write().expect("gpu lock") = None;
}

/// One line describing the acceleration in use, for settings and logs.
pub fn status() -> String {
    if let Some(error) = GPU_FAILURE.read().expect("gpu lock").as_ref() {
        return format!("GPU unavailable ({error}); using the CPU");
    }
    match GPU.read().expect("gpu lock").as_ref() {
        None => "No GPU adapter; using the CPU".into(),
        Some(_) if !gpu_enabled() => "GPU acceleration is turned off; using the CPU".into(),
        Some(context) => format!("GPU acceleration: {}", context.adapter_name()),
    }
}

// ───────────────────────── probes ─────────────────────────

/// "Is the highest surface under every sample point at most `limit`?"
///
/// The samples are the point itself (when `center`) and `ring` points evenly
/// spaced on a circle of `radius` around it, starting at angle 0.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Probe {
    pub x: f64,
    pub y: f64,
    pub limit: f64,
    pub radius: f64,
    pub ring: u32,
    pub center: bool,
}

impl Probe {
    /// The cutter-access test of `MeshQuery::cutter_accessible`.
    pub fn cutter(x: f64, y: f64, z: f64, radius: f64, tolerance: f64) -> Self {
        Self {
            x,
            y,
            limit: z + tolerance,
            radius,
            ring: 8,
            center: true,
        }
    }

    /// A single point.
    pub fn point(x: f64, y: f64, limit: f64) -> Self {
        Self {
            x,
            y,
            limit,
            radius: 0.0,
            ring: 0,
            center: true,
        }
    }

    /// A ring of `samples` points around `(x, y)` (holder envelope).
    pub fn ring(x: f64, y: f64, limit: f64, radius: f64, samples: u32) -> Self {
        Self {
            x,
            y,
            limit,
            radius,
            ring: samples,
            center: false,
        }
    }

    fn is_finite(&self) -> bool {
        [self.x, self.y, self.limit, self.radius]
            .iter()
            .all(|value| value.is_finite())
    }
}

/// How the last batch of probes was decided (for diagnostics and tests).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProbeStats {
    pub total: usize,
    pub decided_on_gpu: usize,
    pub checked_exactly: usize,
}

/// Answers probe batches for one mesh, on the GPU when possible.
pub struct ProbeEngine<'a> {
    query: &'a MeshQuery<'a>,
    bounds: Option<GpuBounds>,
    last: std::cell::Cell<ProbeStats>,
}

impl<'a> ProbeEngine<'a> {
    /// Use the installed GPU (if enabled).
    pub fn new(query: &'a MeshQuery<'a>) -> Self {
        Self::with_gpu(query, active_gpu())
    }

    /// Use a specific GPU, or none.
    pub fn with_gpu(query: &'a MeshQuery<'a>, gpu: Option<Arc<GpuContext>>) -> Self {
        let triangles = &query.model().triangles;
        let bounds = gpu
            .filter(|_| triangles.len() >= GPU_MIN_TRIANGLES)
            .and_then(|gpu| match gpu.build_bounds(query.vertices(), triangles) {
                Ok(bounds) => Some(bounds),
                Err(error) => {
                    report_gpu_failure(&error);
                    None
                }
            });
        Self {
            query,
            bounds,
            last: std::cell::Cell::new(ProbeStats::default()),
        }
    }

    pub fn uses_gpu(&self) -> bool {
        self.bounds.is_some()
    }

    pub fn last_stats(&self) -> ProbeStats {
        self.last.get()
    }

    /// For every probe: `true` when the surface under all its sample points is
    /// no higher than its limit. Identical to the exact CPU answer.
    pub fn fits(&self, probes: &[Probe]) -> Vec<bool> {
        let mut stats = ProbeStats {
            total: probes.len(),
            ..Default::default()
        };
        if let Some(bounds) = self
            .bounds
            .as_ref()
            .filter(|_| probes.len() >= GPU_MIN_PROBES)
        {
            match bounds.classify(probes) {
                Ok(classes) => {
                    let mut answers: Vec<bool> = classes.iter().map(|class| *class == 0).collect();
                    let undecided: Vec<usize> = classes
                        .iter()
                        .enumerate()
                        .filter(|(_, class)| **class == 2)
                        .map(|(index, _)| index)
                        .collect();
                    stats.decided_on_gpu = probes.len() - undecided.len();
                    stats.checked_exactly = undecided.len();
                    let subset: Vec<Probe> = undecided.iter().map(|&index| probes[index]).collect();
                    for (index, fits) in undecided
                        .iter()
                        .zip(exact_fits_parallel(self.query, &subset))
                    {
                        answers[*index] = fits;
                    }
                    self.last.set(stats);
                    return answers;
                }
                Err(error) => report_gpu_failure(&error),
            }
        }
        stats.checked_exactly = probes.len();
        self.last.set(stats);
        exact_fits_parallel(self.query, probes)
    }
}

/// The exact CPU answer for one probe.
pub fn exact_fits(query: &MeshQuery<'_>, probe: &Probe) -> bool {
    if !probe.is_finite() || probe.radius < 0.0 {
        return false;
    }
    let free = |x: f64, y: f64| {
        query
            .height_at_xy(x, y)
            .is_none_or(|surface| surface.height <= probe.limit)
    };
    if probe.center && !free(probe.x, probe.y) {
        return false;
    }
    let ring = probe.ring.min(64);
    (0..ring).all(|sample| {
        let angle = sample as f64 * TAU / ring as f64;
        free(
            probe.x + probe.radius * angle.cos(),
            probe.y + probe.radius * angle.sin(),
        )
    })
}

/// Exact answers for many probes, spread over all CPU cores. Stops early
/// (leaving `false`s) when the running job is cancelled.
pub fn exact_fits_parallel(query: &MeshQuery<'_>, probes: &[Probe]) -> Vec<bool> {
    const CHUNK: usize = 2048;
    let mut answers = vec![false; probes.len()];
    if probes.len() <= CHUNK {
        for (answer, probe) in answers.iter_mut().zip(probes) {
            *answer = exact_fits(query, probe);
        }
        return answers;
    }
    let workers = std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .min(16);
    let next = AtomicUsize::new(0);
    let stop = AtomicBool::new(false);
    let slots: Vec<std::sync::Mutex<&mut [bool]>> = answers
        .chunks_mut(CHUNK)
        .map(std::sync::Mutex::new)
        .collect();
    std::thread::scope(|scope| {
        for worker in 0..workers {
            let (next, stop, slots) = (&next, &stop, &slots);
            let work = move || loop {
                // Only the thread that owns the job's cancel flag can see it.
                if worker == 0 && crate::cam::cancel::is_cancelled() {
                    stop.store(true, Ordering::Relaxed);
                }
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let chunk = next.fetch_add(1, Ordering::Relaxed);
                if chunk >= slots.len() {
                    break;
                }
                let mut slot = slots[chunk].lock().expect("probe slot");
                let first = chunk * CHUNK;
                for (offset, answer) in slot.iter_mut().enumerate() {
                    *answer = exact_fits(query, &probes[first + offset]);
                }
            };
            if worker == 0 {
                work();
            } else {
                scope.spawn(work);
            }
        }
    });
    drop(slots);
    answers
}

// ───────────────────────── stock cutting ─────────────────────────

/// A regular grid of node heights: node `(column, row)` sits at
/// `origin + (column × cell.x, row × cell.y)` and is stored at
/// `row × columns + column`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSpec {
    pub origin: [f64; 2],
    pub cell: [f64; 2],
    pub columns: usize,
    pub rows: usize,
    /// No cut goes below this height.
    pub floor: f32,
}

/// A straight cutter movement `(start, end)` in world coordinates.
pub type Cut = [[f32; 3]; 2];

/// The cutter shape swept along a cut: a cylinder of `radius` whose lower edge
/// is rounded by `corner` (0 = flat end mill, `radius` = ball nose, in between =
/// bull nose).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CutTool {
    pub radius: f32,
    pub corner: f32,
}

impl CutTool {
    /// The cutter's real geometry: end mills are flat, ball-nose tools fully
    /// round, bull-nose tools rounded by their corner radius.
    pub fn from_tool(tool: &crate::cam::tool::Tool) -> Self {
        use crate::cam::tool::ToolShape;
        let radius = (tool.diameter_mm * 0.5) as f32;
        let corner = match tool.shape {
            ToolShape::Flat => 0.0,
            ToolShape::BallNose => radius,
            ToolShape::BullNose => (tool.corner_radius_mm as f32).clamp(0.0, radius),
        };
        Self { radius, corner }
    }

    pub fn flat(radius: f32) -> Self {
        Self {
            radius,
            corner: 0.0,
        }
    }

    pub fn ball(radius: f32) -> Self {
        Self {
            radius,
            corner: radius,
        }
    }

    /// Height of the cutter's lower surface above its tip at horizontal distance
    /// `distance` from the axis.
    pub fn profile(&self, distance: f64) -> f64 {
        let corner = (self.corner as f64).clamp(0.0, self.radius as f64);
        let flat = self.radius as f64 - corner;
        if corner > 0.0 && distance > flat {
            let across = distance - flat;
            corner - (corner * corner - across * across).max(0.0).sqrt()
        } else {
            0.0
        }
    }
}

/// Lower a height-field with swept-cutter cuts: every node within the cutter
/// radius of a segment is lowered to the segment's height there (plus the
/// cutter's corner profile). Order of the cuts does not matter.
pub fn cut_capsules(spec: &GridSpec, heights: &mut [f32], cuts: &[Cut], tool: CutTool) {
    if cuts.is_empty() || heights.len() != spec.columns * spec.rows {
        return;
    }
    if let Some(gpu) = active_gpu() {
        if estimated_cells(spec, cuts, tool) >= GPU_MIN_CUT_CELLS {
            match gpu.cut_capsules(spec, heights, cuts, tool) {
                Ok(()) => return,
                Err(error) => report_gpu_failure(&error),
            }
        }
    }
    cut_capsules_cpu(spec, heights, cuts, tool);
}

/// Number of grid nodes under the bounding boxes of all cuts.
fn estimated_cells(spec: &GridSpec, cuts: &[Cut], tool: CutTool) -> usize {
    let r = tool.radius as f64;
    cuts.iter()
        .map(|cut| {
            let width = ((cut[0][0] - cut[1][0]).abs() as f64 + 2.0 * r) / spec.cell[0] + 1.0;
            let height = ((cut[0][1] - cut[1][1]).abs() as f64 + 2.0 * r) / spec.cell[1] + 1.0;
            (width * height) as usize
        })
        .sum()
}

/// CPU reference of the cutting kernel.
pub fn cut_capsules_cpu(spec: &GridSpec, heights: &mut [f32], cuts: &[Cut], tool: CutTool) {
    let radius = tool.radius as f64;
    let radius_sq = radius * radius;
    let last_column = spec.columns as isize - 1;
    let last_row = spec.rows as isize - 1;
    for cut in cuts {
        let start = cut[0].map(f64::from);
        let end = cut[1].map(f64::from);
        let min_x = (((start[0].min(end[0]) - radius - spec.origin[0]) / spec.cell[0]).floor()
            as isize)
            .clamp(0, last_column);
        let max_x = (((start[0].max(end[0]) + radius - spec.origin[0]) / spec.cell[0]).ceil()
            as isize)
            .clamp(0, last_column);
        let min_y = (((start[1].min(end[1]) - radius - spec.origin[1]) / spec.cell[1]).floor()
            as isize)
            .clamp(0, last_row);
        let max_y = (((start[1].max(end[1]) + radius - spec.origin[1]) / spec.cell[1]).ceil()
            as isize)
            .clamp(0, last_row);
        let ab = [end[0] - start[0], end[1] - start[1]];
        let length_sq = ab[0] * ab[0] + ab[1] * ab[1];
        for iy in min_y..=max_y {
            let y = spec.origin[1] + iy as f64 * spec.cell[1];
            for ix in min_x..=max_x {
                let x = spec.origin[0] + ix as f64 * spec.cell[0];
                // A purely vertical move (a plunge) cuts down to its lowest point.
                let (t, depth) = if length_sq > 1e-12 {
                    let t = (((x - start[0]) * ab[0] + (y - start[1]) * ab[1]) / length_sq)
                        .clamp(0.0, 1.0);
                    (t, start[2] + (end[2] - start[2]) * t)
                } else {
                    (0.0, start[2].min(end[2]))
                };
                let (dx, dy) = (x - (start[0] + ab[0] * t), y - (start[1] + ab[1] * t));
                let dist_sq = dx * dx + dy * dy;
                if dist_sq > radius_sq {
                    continue;
                }
                let z = depth + tool.profile(dist_sq.sqrt());
                let z = (z as f32).max(spec.floor);
                let node = iy as usize * spec.columns + ix as usize;
                if z < heights[node] {
                    heights[node] = z;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
