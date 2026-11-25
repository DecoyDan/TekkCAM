//! Adaptive (engagement-limited) 3D roughing.
//!
//! Instead of raster passes that bury the cutter in a full-width slot, every
//! Z level is cleared with offset loops spaced by a small radial stepover (at
//! most 12 % of the cutter diameter). The loops are cut from the inside of the
//! open area outward, so after the first entry the cutter always works against
//! material it has already opened up, and its radial engagement stays low and
//! nearly constant. Where the engagement still rises (corners, the first pass
//! beside a wall) the feed is reduced in proportion, which keeps the chip load
//! and the load on a low-torque spindle even.
//!
//! How a level is planned:
//! 1. The cutter-centre positions that clear the model (with the allowance) are
//!    found on a grid, using the same probe engine as the other roughing code.
//! 2. A distance field to the unreachable cells gives constant-distance loops
//!    (marching squares), so every loop is exactly one stepover inside the last.
//! 3. A grid of the material still standing at this depth tracks what each loop
//!    removes; it measures the engagement along the path, drops stretches that
//!    only cut air, and decides the feed.
//! 4. Each open area is entered once with a shallow helical ramp (never steeper
//!    than 2°) in space that is already clear; links between loops stay at depth.

use std::collections::{HashMap, VecDeque};

use crate::accel::{Probe, ProbeEngine};
use crate::cam::{cancel, tool::Tool, toolpath::Toolpath};
use crate::workspace::{
    mesh_query::MeshQuery,
    model::Model,
    setup::{MachiningSetup, WorkAxis},
};

/// Radial stepover limit as a fraction of the tool diameter.
pub const MAX_RADIAL_STEPOVER: f64 = 0.12;
/// Steepest entry ramp used (degrees), whatever the setup asks for.
pub const MAX_RAMP_ANGLE_DEG: f64 = 2.0;
const MAX_LEVELS: usize = 512;
const MAX_MOVES: usize = 3_000_000;
const MAX_NODES: usize = 700_000;
/// Engagement (fraction of the cutter circle in material) below which a
/// stretch of path is treated as air.
const MIN_ENGAGEMENT: f64 = 0.02;
const ENGAGEMENT_SAMPLES: usize = 32;
/// Feed is never cut below this fraction of the normal feed.
const MIN_FEED_SCALE: f64 = 0.35;

/// Tuning for [`generate`].
#[derive(Debug, Clone, Copy)]
pub struct AdaptiveParams {
    pub stepover_ratio: f64,
    pub stepdown_mm: f64,
    pub ramp_angle_deg: f64,
    pub allowance_mm: f64,
}

impl AdaptiveParams {
    /// Conservative parameters derived from the project setup and the tool.
    pub fn conservative(setup: &MachiningSetup, tool: &Tool, allowance_mm: f64) -> Self {
        Self {
            stepover_ratio: (setup.stepover_percent / 100.0).clamp(0.02, MAX_RADIAL_STEPOVER),
            // A moderate axial depth: the radial engagement is already small.
            stepdown_mm: setup.stepdown_mm.min(tool.diameter_mm * 0.5).max(0.1),
            ramp_angle_deg: setup.ramp_angle_deg.clamp(0.2, MAX_RAMP_ANGLE_DEG),
            allowance_mm,
        }
    }
}

/// Engagement (fraction of the cutter circle) of a straight pass with the
/// given radial stepover: the steady state the feed is normalised to.
pub fn nominal_engagement(stepover_ratio: f64) -> f64 {
    (1.0 - 2.0 * stepover_ratio).clamp(-1.0, 1.0).acos() / std::f64::consts::TAU
}

struct Grid {
    x0: f64,
    y0: f64,
    h: f64,
    cols: usize,
    rows: usize,
}

impl Grid {
    fn pos(&self, col: usize, row: usize) -> [f64; 2] {
        [self.x0 + col as f64 * self.h, self.y0 + row as f64 * self.h]
    }

    fn nearest(&self, p: [f64; 2]) -> Option<usize> {
        let col = ((p[0] - self.x0) / self.h).round();
        let row = ((p[1] - self.y0) / self.h).round();
        if col < 0.0 || row < 0.0 || col >= self.cols as f64 || row >= self.rows as f64 {
            return None;
        }
        Some(row as usize * self.cols + col as usize)
    }
}

/// Generate adaptive roughing for a Z-up model fitted in its stock.
pub fn generate(
    model: &Model,
    setup: &MachiningSetup,
    tool: &Tool,
    params: &AdaptiveParams,
) -> Result<Toolpath, String> {
    generate_checked(model, setup, tool, params, true)
}

fn generate_checked(
    model: &Model,
    setup: &MachiningSetup,
    tool: &Tool,
    params: &AdaptiveParams,
    validate: bool,
) -> Result<Toolpath, String> {
    setup
        .validate()
        .map_err(|error| format!("setup: {error}"))?;
    tool.validate().map_err(|error| format!("tool: {error}"))?;
    model.validate()?;
    if setup.axis != WorkAxis::ZUp {
        return Err("adaptive roughing currently requires the Z-up work axis".into());
    }
    if !params.allowance_mm.is_finite() || params.allowance_mm < 0.0 {
        return Err("roughing allowance must be non-negative".into());
    }
    let ratio = params.stepover_ratio.clamp(0.02, 0.5);
    let radius = tool.diameter_mm * 0.5;
    let stock_min = setup.stock_origin_mm;
    let stock_top = stock_min[2] + setup.stock_size_mm[2];
    // Cut as deep as the cutter can reach, but no deeper than the stock floor.
    // The part must lie within that reach; a base layer may remain below it.
    let reach_floor = stock_top - (tool.stickout_mm - 1.0).max(0.0);
    let floor = stock_min[2].max(reach_floor);
    let model_bottom = model.transformed_bounds()[0][2] as f64;
    if floor > model_bottom + 1e-6 {
        return Err(format!(
            "tool reach ({:.1} mm) cannot get to the bottom of the part ({:.1} mm below the stock top)",
            tool.stickout_mm,
            stock_top - model_bottom
        ));
    }

    // Grid over the stock plus one cutter radius of air on every side: the
    // cutter centre may leave the stock to clear its edges.
    let h = (tool.diameter_mm / 10.0).clamp(0.2, 0.8);
    let span = [
        setup.stock_size_mm[0] + 2.0 * radius + 4.0 * h,
        setup.stock_size_mm[1] + 2.0 * radius + 4.0 * h,
    ];
    let h = h.max((span[0] * span[1] / MAX_NODES as f64).sqrt());
    let grid = Grid {
        x0: stock_min[0] - radius - 2.0 * h,
        y0: stock_min[1] - radius - 2.0 * h,
        h,
        cols: (span[0] / h).ceil() as usize + 1,
        rows: (span[1] / h).ceil() as usize + 1,
    };
    let nodes = grid.cols * grid.rows;
    let stock_max = [
        stock_min[0] + setup.stock_size_mm[0],
        stock_min[1] + setup.stock_size_mm[1],
    ];

    // Material still standing at each node (top height), -inf in the air.
    let mut standing: Vec<f32> = (0..nodes)
        .map(|index| {
            let p = grid.pos(index % grid.cols, index / grid.cols);
            let inside = p[0] >= stock_min[0] - 1e-6
                && p[0] <= stock_max[0] + 1e-6
                && p[1] >= stock_min[1] - 1e-6
                && p[1] <= stock_max[1] + 1e-6;
            if inside {
                stock_top as f32
            } else {
                f32::NEG_INFINITY
            }
        })
        .collect();

    let query = MeshQuery::new(model);
    let engine = ProbeEngine::new(&query);
    let safe_z = stock_top + setup.clearance_mm + setup.safe_z_mm;
    let nominal = nominal_engagement(ratio);
    let mut planner = LevelPlanner {
        grid,
        radius,
        stepover: ratio * tool.diameter_mm,
        nominal,
        tool,
        params: *params,
        safe_z,
        path: Toolpath::default(),
        position: None,
    };

    let mut z = (stock_top - params.stepdown_mm).max(floor);
    let mut level = 0;
    loop {
        level += 1;
        if level > MAX_LEVELS {
            return Err(
                "adaptive roughing needs too many depth levels; increase the stepdown".into(),
            );
        }
        cancel::check()?;
        let any_reachable = planner.plan_level(
            &engine,
            &mut standing,
            z,
            (z + params.stepdown_mm).min(stock_top),
            level == 1,
        )?;
        if planner.path.moves.len() > MAX_MOVES {
            return Err("adaptive roughing generated too many moves; use a larger cutter".into());
        }
        if !any_reachable || z <= floor + 1e-9 {
            break;
        }
        z = (z - params.stepdown_mm).max(floor);
    }
    if planner.path.is_empty() {
        return Err("adaptive roughing found nothing to cut".into());
    }
    if validate {
        super::roughing::validate_cutting_segments(&planner.path, &engine, tool, setup)?;
    }
    Ok(planner.path)
}

struct LevelPlanner<'a> {
    grid: Grid,
    radius: f64,
    stepover: f64,
    nominal: f64,
    tool: &'a Tool,
    params: AdaptiveParams,
    safe_z: f64,
    path: Toolpath,
    /// Where the cutter is, once it is at cutting depth.
    position: Option<[f64; 2]>,
}

struct Loop {
    k: usize,
    points: Vec<[f64; 2]>,
}

impl LevelPlanner<'_> {
    /// Plan one Z level. Returns whether the cutter can reach anywhere at this
    /// depth (lower levels reach no more than this one).
    fn plan_level(
        &mut self,
        engine: &ProbeEngine<'_>,
        standing: &mut [f32],
        z: f64,
        slab_top: f64,
        first_level: bool,
    ) -> Result<bool, String> {
        let (cols, rows) = (self.grid.cols, self.grid.rows);
        let h = self.grid.h;

        // 1. Where may the cutter centre be at this depth?
        // One cell of extra clearance: the loops run along cell corners, and the
        // straight chords between them can dip slightly toward a convex corner.
        let probe_radius = self.radius + h;
        let probes: Vec<Probe> = (0..cols * rows)
            .map(|index| {
                let p = self.grid.pos(index % cols, index / cols);
                // A dense ring: with 8 samples a convex corner of the part can
                // slip between two samples and the cutter would gouge it.
                Probe {
                    x: p[0],
                    y: p[1],
                    limit: z - self.params.allowance_mm,
                    radius: probe_radius,
                    ring: 16,
                    center: true,
                }
            })
            .collect();
        let fits = engine.fits(&probes);
        cancel::check()?;
        let mut unreachable: Vec<bool> = fits.iter().map(|fits| !fits).collect();
        for col in 0..cols {
            unreachable[col] = true;
            unreachable[(rows - 1) * cols + col] = true;
        }
        for row in 0..rows {
            unreachable[row * cols] = true;
            unreachable[row * cols + cols - 1] = true;
        }
        if unreachable.iter().all(|blocked| *blocked) {
            return Ok(false);
        }

        // 2. Loops of constant distance from everything unreachable.
        let distance = distance_to_blocked(&unreachable, cols, rows);
        let component = label_components(&unreachable, cols, rows);
        let step_cells = (self.stepover / h).max(0.3);
        let mut loops = extract_loops(&distance, cols, rows, 1.0, step_cells);
        for item in &mut loops {
            for point in &mut item.points {
                *point = [self.grid.x0 + point[0] * h, self.grid.y0 + point[1] * h];
            }
            item.points = simplify_closed(&item.points, 0.02_f64.min(h * 0.25));
        }

        // Where each open area is widest while still holding material: the
        // place to enter it.
        let mut entries: HashMap<usize, (usize, f64)> = HashMap::new();
        for index in 0..cols * rows {
            let id = component[index];
            if id == 0 || (standing[index] as f64) <= z + 1e-6 {
                continue;
            }
            let best = entries.entry(id).or_insert((index, distance[index]));
            if distance[index] > best.1 {
                *best = (index, distance[index]);
            }
        }

        // 3. Group the loops by open area and cut each area.
        let mut by_component: HashMap<usize, Vec<Loop>> = HashMap::new();
        for item in loops {
            let id = self.component_of(item.points[0], &component);
            if id != 0 && entries.contains_key(&id) {
                by_component.entry(id).or_default().push(item);
            }
        }
        let mut ids: Vec<usize> = entries.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let mut loops = by_component.remove(&id).unwrap_or_default();
            let (entry_node, entry_clearance) = entries[&id];
            self.cut_component(
                &mut loops,
                standing,
                &distance,
                (entry_node, entry_clearance),
                z,
                slab_top,
                first_level,
            )?;
        }
        Ok(true)
    }

    /// Open-area number at a point (looking at the neighbouring cells when the
    /// nearest one is blocked).
    fn component_of(&self, p: [f64; 2], component: &[usize]) -> usize {
        let cols = self.grid.cols as isize;
        let Some(node) = self.grid.nearest(p) else {
            return 0;
        };
        for offset in [
            0isize,
            1,
            -1,
            cols,
            -cols,
            cols + 1,
            cols - 1,
            -cols + 1,
            -cols - 1,
        ] {
            let index = node as isize + offset;
            if index >= 0 && (index as usize) < component.len() && component[index as usize] != 0 {
                return component[index as usize];
            }
        }
        0
    }

    /// Whether a straight link stays where the cutter may go.
    fn link_is_clear(&self, from: [f64; 2], to: [f64; 2], distance: &[f64]) -> bool {
        let length = dist(from, to);
        let steps = ((length / (self.grid.h * 0.5)).ceil() as usize).max(1);
        (0..=steps).all(|step| {
            let t = step as f64 / steps as f64;
            let p = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ];
            self.grid
                .nearest(p)
                .is_some_and(|node| distance[node] >= 0.9)
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn cut_component(
        &mut self,
        loops: &mut [Loop],
        standing: &mut [f32],
        distance: &[f64],
        entry: (usize, f64),
        z: f64,
        slab_top: f64,
        first_level: bool,
    ) -> Result<(), String> {
        let cols = self.grid.cols;
        let h = self.grid.h;
        let (entry_node, entry_clearance) = entry;
        let entry_xy = self.grid.pos(entry_node % cols, entry_node / cols);
        // Inside-out: widest loops (largest k) first.
        loops.sort_by(|a, b| b.k.cmp(&a.k));

        // Helical entry in space the cutter can reach, if there is room.
        let helix_radius = (self.radius * 0.45).min((entry_clearance - 1.0) * h * 0.8);
        let mut helix_end: Option<[f64; 2]> = None;
        if helix_radius >= 0.12 * self.tool.diameter_mm {
            let top = self.material_top(standing, entry_xy, self.radius + helix_radius, slab_top);
            let start_z = if first_level { top } else { top.min(slab_top) };
            let rapid_z = start_z + 0.5;
            let tan = self.params.ramp_angle_deg.to_radians().tan();
            let pitch = (std::f64::consts::TAU * helix_radius * tan).max(0.03);
            let turns = ((start_z - z) / pitch).max(0.25);
            let segments = ((turns * 48.0).ceil() as usize).max(8);
            let start = [entry_xy[0] + helix_radius, entry_xy[1]];
            self.path.push_rapid(start, rapid_z);
            self.path
                .push_feed(start, start_z, self.tool.plunge_rate_mm_min);
            let mut last = start;
            for step in 1..=segments {
                let t = step as f64 / segments as f64;
                let angle = turns * std::f64::consts::TAU * t;
                let p = [
                    entry_xy[0] + helix_radius * angle.cos(),
                    entry_xy[1] + helix_radius * angle.sin(),
                ];
                self.path
                    .push_feed(p, start_z + (z - start_z) * t, self.tool.plunge_rate_mm_min);
                self.mark_cleared(standing, p, z);
                last = p;
            }
            // A flat circle at depth finishes the ramp.
            for step in 1..=48 {
                let angle =
                    turns * std::f64::consts::TAU + step as f64 * std::f64::consts::TAU / 48.0;
                let p = [
                    entry_xy[0] + helix_radius * angle.cos(),
                    entry_xy[1] + helix_radius * angle.sin(),
                ];
                self.path.push_feed(p, z, self.tool.feed_rate_mm_min * 0.5);
                self.mark_cleared(standing, p, z);
                last = p;
            }
            helix_end = Some(last);
            self.position = Some(last);
        }

        // Cut the loops from the inside out, nearest loop first within a level.
        let mut remaining: Vec<usize> = (0..loops.len()).collect();
        let mut current = helix_end.unwrap_or(entry_xy);
        let mut needs_entry = helix_end.is_none();
        while !remaining.is_empty() {
            cancel::check()?;
            let top_k = remaining
                .iter()
                .map(|&index| loops[index].k)
                .max()
                .expect("not empty");
            // The nearest loop among those of the current ring.
            let (slot, _) = remaining
                .iter()
                .enumerate()
                .filter(|(_, &index)| loops[index].k == top_k)
                .map(|(slot, &index)| {
                    let nearest = loops[index]
                        .points
                        .iter()
                        .map(|p| dist(*p, current))
                        .fold(f64::INFINITY, f64::min);
                    (slot, nearest)
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .expect("ring not empty");
            let index = remaining.remove(slot);
            let ended = self.cut_loop(
                &loops[index],
                standing,
                distance,
                current,
                z,
                slab_top,
                &mut needs_entry,
            );
            if let Some(end) = ended {
                current = end;
            }
        }
        self.position = Some(current);
        Ok(())
    }

    /// Material top height (highest standing material) around a point.
    fn material_top(&self, standing: &[f32], p: [f64; 2], radius: f64, fallback: f64) -> f64 {
        let mut top = f64::NEG_INFINITY;
        self.for_nodes_within(p, radius, |index| {
            top = top.max(standing[index] as f64);
        });
        if top.is_finite() {
            top
        } else {
            fallback
        }
    }

    fn for_nodes_within(&self, p: [f64; 2], radius: f64, mut visit: impl FnMut(usize)) {
        let g = &self.grid;
        let c0 = (((p[0] - radius - g.x0) / g.h).floor() as isize).max(0);
        let c1 = (((p[0] + radius - g.x0) / g.h).ceil() as isize).min(g.cols as isize - 1);
        let r0 = (((p[1] - radius - g.y0) / g.h).floor() as isize).max(0);
        let r1 = (((p[1] + radius - g.y0) / g.h).ceil() as isize).min(g.rows as isize - 1);
        let radius_sq = radius * radius;
        for row in r0..=r1 {
            let y = g.y0 + row as f64 * g.h;
            for col in c0..=c1 {
                let x = g.x0 + col as f64 * g.h;
                if (x - p[0]).powi(2) + (y - p[1]).powi(2) <= radius_sq {
                    visit(row as usize * g.cols + col as usize);
                }
            }
        }
    }

    /// Lower the standing material to `z` under the cutter at `p`.
    fn mark_cleared(&self, standing: &mut [f32], p: [f64; 2], z: f64) {
        let z = z as f32;
        self.for_nodes_within(p, self.radius, |index| {
            if standing[index] > z {
                standing[index] = z;
            }
        });
    }

    /// Fraction of the cutter circle that is in material still to be cut at
    /// this depth (0 = air, 0.5 = slot).
    fn engagement(&self, standing: &[f32], p: [f64; 2], z: f64) -> f64 {
        let mut inside = 0;
        for sample in 0..ENGAGEMENT_SAMPLES {
            let angle = sample as f64 * std::f64::consts::TAU / ENGAGEMENT_SAMPLES as f64;
            let q = [
                p[0] + self.radius * angle.cos(),
                p[1] + self.radius * angle.sin(),
            ];
            if let Some(node) = self.grid.nearest(q) {
                if standing[node] as f64 > z + 1e-6 {
                    inside += 1;
                }
            }
        }
        inside as f64 / ENGAGEMENT_SAMPLES as f64
    }

    /// Feed multiplier for an engagement: normal up to 1.35x the steady
    /// state, then inversely proportional, never below the minimum.
    fn feed_scale(&self, engagement: f64) -> f64 {
        let reference = self.nominal * 1.35;
        let raw = if engagement <= reference {
            1.0
        } else {
            (reference / engagement).max(MIN_FEED_SCALE)
        };
        (raw * 20.0).round() / 20.0
    }

    /// Cut one loop, segment by segment: measure the engagement against the
    /// material still standing, skip stretches that only cut air, and set the
    /// feed. Returns where the cutter ends up.
    ///
    /// Decisions about reaching a run (links, re-entry) are made against the
    /// material as it stood *before* that segment was cut.
    #[allow(clippy::too_many_arguments)]
    fn cut_loop(
        &mut self,
        item: &Loop,
        standing: &mut [f32],
        distance: &[f64],
        current: [f64; 2],
        z: f64,
        slab_top: f64,
        needs_entry: &mut bool,
    ) -> Option<[f64; 2]> {
        let points = &item.points;
        if points.len() < 3 {
            return None;
        }
        // Start the loop at the point nearest the cutter.
        let start = points
            .iter()
            .enumerate()
            .map(|(index, p)| (index, dist(*p, current)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        let count = points.len() - 1; // closed: last == first
        let ordered: Vec<[f64; 2]> = (0..=count)
            .map(|offset| points[(start + offset) % count])
            .collect();

        let spacing = (self.grid.h * 0.9).max(0.1);
        let mut end = None;
        let mut in_run = false;
        // Nodes lowered while measuring one segment, so they can be put back
        // while the way to the segment is decided.
        let mut undo: Vec<(usize, f32)> = Vec::new();
        for (index, pair) in ordered.windows(2).enumerate() {
            let length = dist(pair[0], pair[1]);
            let samples = ((length / spacing).ceil() as usize).max(1);
            undo.clear();
            let mut peak: f64 = 0.0;
            for step in 0..=samples {
                let t = step as f64 / samples as f64;
                let p = [
                    pair[0][0] + (pair[1][0] - pair[0][0]) * t,
                    pair[0][1] + (pair[1][1] - pair[0][1]) * t,
                ];
                peak = peak.max(self.engagement(standing, p, z));
                self.clear_logged(standing, p, z, &mut undo);
            }
            if peak < MIN_ENGAGEMENT {
                in_run = false;
                continue;
            }
            if !in_run {
                // Back to how the material stood before this segment.
                for (node, old) in undo.iter().rev() {
                    standing[*node] = *old;
                }
                let ahead = &ordered[index..(index + 64).min(ordered.len())];
                self.approach(pair[0], standing, distance, z, slab_top, needs_entry, ahead);
                for (node, _) in &undo {
                    standing[*node] = z as f32;
                }
                in_run = true;
            }
            self.path.push_feed(
                pair[1],
                z,
                (self.tool.feed_rate_mm_min * self.feed_scale(peak)).max(1.0),
            );
            end = Some(pair[1]);
            self.position = end;
        }
        end
    }

    /// Like [`Self::mark_cleared`], remembering the old heights for undoing.
    fn clear_logged(
        &self,
        standing: &mut [f32],
        p: [f64; 2],
        z: f64,
        undo: &mut Vec<(usize, f32)>,
    ) {
        let z = z as f32;
        self.for_nodes_within(p, self.radius, |index| {
            if standing[index] > z {
                undo.push((index, standing[index]));
                standing[index] = z;
            }
        });
    }

    /// Get the cutter to `target` at cutting depth: a feed link when it is
    /// already at depth close by, otherwise a retract and a re-entry from
    /// space that is already clear (or a ramp along the run).
    #[allow(clippy::too_many_arguments)]
    fn approach(
        &mut self,
        target: [f64; 2],
        standing: &[f32],
        distance: &[f64],
        z: f64,
        slab_top: f64,
        needs_entry: &mut bool,
        run: &[[f64; 2]],
    ) {
        if !*needs_entry {
            if let Some(from) = self.position {
                // Short links stay at depth; the loops are one stepover apart.
                if dist(from, target) <= 4.0 * self.tool.diameter_mm.max(self.stepover)
                    && self.link_is_clear(from, target, distance)
                {
                    self.path
                        .push_feed(target, z, self.tool.feed_rate_mm_min * 0.6);
                    return;
                }
            }
        }
        // Re-enter one stepover toward the open side, where earlier loops have
        // already cut at this depth.
        let inward = self.inward_point(target, distance);
        if let Some(entry) = inward {
            // One stepover inward the cutter's footprint stays inside what the
            // earlier loops have already cleared.
            if self.material_top(standing, entry, self.radius * 0.9, f64::NEG_INFINITY) <= z + 1e-6
            {
                self.path.push_rapid(entry, z + 0.5);
                self.path.push_feed(entry, z, self.tool.plunge_rate_mm_min);
                self.path
                    .push_feed(target, z, self.tool.feed_rate_mm_min * 0.6);
                *needs_entry = false;
                return;
            }
        }
        // No clear space beside it: ramp down along the run itself.
        self.ramp_along(run, z, slab_top);
        *needs_entry = false;
    }

    /// A point one stepover from `p` toward higher distance (the open side).
    fn inward_point(&self, p: [f64; 2], distance: &[f64]) -> Option<[f64; 2]> {
        let node = self.grid.nearest(p)?;
        let (cols, rows) = (self.grid.cols, self.grid.rows);
        let (col, row) = (node % cols, node / cols);
        if col == 0 || row == 0 || col + 1 >= cols || row + 1 >= rows {
            return None;
        }
        let gx = distance[node + 1] - distance[node - 1];
        let gy = distance[node + cols] - distance[node - cols];
        let length = gx.hypot(gy);
        if length < 1e-9 {
            return None;
        }
        Some([
            p[0] + gx / length * self.stepover,
            p[1] + gy / length * self.stepover,
        ])
    }

    /// Descend gradually along a run (back and forth when it is short), then
    /// walk back to the start of the run at depth, along the run itself (a
    /// straight return could cross the part).
    fn ramp_along(&mut self, run: &[[f64; 2]], z: f64, slab_top: f64) {
        if run.len() < 2 {
            return;
        }
        let tan = self.params.ramp_angle_deg.to_radians().tan();
        self.path.push_rapid(run[0], slab_top + 0.5);
        self.path
            .push_feed(run[0], slab_top, self.tool.plunge_rate_mm_min);
        let mut height = slab_top;
        let mut index = 0usize;
        let mut forward = true;
        let mut guard = 0;
        while height > z + 1e-9 && guard < 10_000 {
            guard += 1;
            if forward {
                while index + 1 < run.len() && height > z + 1e-9 {
                    height = (height - dist(run[index], run[index + 1]) * tan).max(z);
                    index += 1;
                    self.path
                        .push_feed(run[index], height, self.tool.plunge_rate_mm_min);
                }
            } else {
                while index > 0 && height > z + 1e-9 {
                    height = (height - dist(run[index], run[index - 1]) * tan).max(z);
                    index -= 1;
                    self.path
                        .push_feed(run[index], height, self.tool.plunge_rate_mm_min);
                }
            }
            forward = !forward;
        }
        // At depth: back along the run to its start.
        while index > 0 {
            index -= 1;
            self.path
                .push_feed(run[index], z, self.tool.feed_rate_mm_min * 0.5);
        }
        self.position = Some(run[0]);
    }
}

fn dist(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

// ───────────────────────── geometry helpers ─────────────────────────

/// Distance (in cells) from every cell to the nearest blocked cell.
fn distance_to_blocked(blocked: &[bool], cols: usize, rows: usize) -> Vec<f64> {
    const INF: f64 = 1e20;
    let mut d: Vec<f64> = blocked.iter().map(|b| if *b { 0.0 } else { INF }).collect();
    let longest = cols.max(rows);
    let mut line = vec![0.0; longest];
    let mut out = vec![0.0; longest];
    let mut v = vec![0usize; longest];
    let mut z = vec![0.0; longest + 1];
    for col in 0..cols {
        for row in 0..rows {
            line[row] = d[row * cols + col];
        }
        edt_1d(&line[..rows], &mut out[..rows], &mut v, &mut z);
        for row in 0..rows {
            d[row * cols + col] = out[row];
        }
    }
    for row in 0..rows {
        line[..cols].copy_from_slice(&d[row * cols..(row + 1) * cols]);
        edt_1d(&line[..cols], &mut out[..cols], &mut v, &mut z);
        d[row * cols..(row + 1) * cols].copy_from_slice(&out[..cols]);
    }
    d.iter().map(|value| value.sqrt()).collect()
}

/// One-dimensional squared distance transform (Felzenszwalb & Huttenlocher).
fn edt_1d(f: &[f64], d: &mut [f64], v: &mut [usize], z: &mut [f64]) {
    let n = f.len();
    if n == 0 {
        return;
    }
    let mut k = 0;
    v[0] = 0;
    z[0] = f64::NEG_INFINITY;
    z[1] = f64::INFINITY;
    for q in 1..n {
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f64) - (f[p] + (p * p) as f64))
                / (2.0 * q as f64 - 2.0 * p as f64);
            if s <= z[k] && k > 0 {
                k -= 1;
                continue;
            }
            if s <= z[k] {
                // k == 0: the new parabola dominates the first.
                v[0] = q;
                z[0] = f64::NEG_INFINITY;
                z[1] = f64::INFINITY;
            } else {
                k += 1;
                v[k] = q;
                z[k] = s;
                z[k + 1] = f64::INFINITY;
            }
            break;
        }
    }
    k = 0;
    for (q, slot) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f64 {
            k += 1;
        }
        let p = v[k];
        *slot = ((q as f64 - p as f64).powi(2)) + f[p];
    }
}

/// Connected open areas (4-neighbourhood), numbered from 1; blocked cells are 0.
fn label_components(blocked: &[bool], cols: usize, rows: usize) -> Vec<usize> {
    let mut label = vec![0usize; blocked.len()];
    let mut next = 0;
    let mut queue = VecDeque::new();
    for start in 0..blocked.len() {
        if blocked[start] || label[start] != 0 {
            continue;
        }
        next += 1;
        label[start] = next;
        queue.push_back(start);
        while let Some(index) = queue.pop_front() {
            let (col, row) = (index % cols, index / cols);
            let mut neighbours = [usize::MAX; 4];
            if col > 0 {
                neighbours[0] = index - 1;
            }
            if col + 1 < cols {
                neighbours[1] = index + 1;
            }
            if row > 0 {
                neighbours[2] = index - cols;
            }
            if row + 1 < rows {
                neighbours[3] = index + cols;
            }
            for neighbour in neighbours {
                if neighbour != usize::MAX && !blocked[neighbour] && label[neighbour] == 0 {
                    label[neighbour] = next;
                    queue.push_back(neighbour);
                }
            }
        }
    }
    label
}

/// Closed loops of constant distance, `base + k * step` cells from the blocked
/// cells, for k = 0, 1, 2 …, in cell coordinates.
fn extract_loops(distance: &[f64], cols: usize, rows: usize, base: f64, step: f64) -> Vec<Loop> {
    let mut by_k: HashMap<usize, Vec<[[f64; 2]; 2]>> = HashMap::new();
    for row in 0..rows - 1 {
        for col in 0..cols - 1 {
            let bl = distance[row * cols + col];
            let br = distance[row * cols + col + 1];
            let tr = distance[(row + 1) * cols + col + 1];
            let tl = distance[(row + 1) * cols + col];
            let low = bl.min(br).min(tr).min(tl);
            let high = bl.max(br).max(tr).max(tl);
            if high < base {
                continue;
            }
            // Levels v with low < v <= high.
            let k_first = if low < base {
                0
            } else {
                ((low - base) / step).floor() as usize + 1
            };
            let k_last = ((high - base) / step).floor() as usize;
            for k in k_first..=k_last {
                let v = base + k as f64 * step;
                if !(low < v && v <= high) {
                    continue;
                }
                let cross = |a: f64, b: f64| (v - a) / (b - a);
                let (x, y) = (col as f64, row as f64);
                let inside = [bl >= v, br >= v, tr >= v, tl >= v];
                let bottom = (inside[0] != inside[1]).then(|| [x + cross(bl, br), y]);
                let right = (inside[1] != inside[2]).then(|| [x + 1.0, y + cross(br, tr)]);
                let top = (inside[3] != inside[2]).then(|| [x + cross(tl, tr), y + 1.0]);
                let left = (inside[0] != inside[3]).then(|| [x, y + cross(bl, tl)]);
                let found: Vec<[f64; 2]> =
                    [bottom, right, top, left].into_iter().flatten().collect();
                let segments = by_k.entry(k).or_default();
                match found.len() {
                    2 => segments.push([found[0], found[1]]),
                    4 => {
                        // Saddle: decide by the cell centre.
                        let centre = (bl + br + tr + tl) * 0.25 >= v;
                        let (b, r, t, l) = (
                            bottom.expect("saddle"),
                            right.expect("saddle"),
                            top.expect("saddle"),
                            left.expect("saddle"),
                        );
                        if inside[0] == centre {
                            // Same side as the centre: connected through it,
                            // so cut off the other two corners.
                            segments.push([b, r]);
                            segments.push([t, l]);
                        } else {
                            segments.push([b, l]);
                            segments.push([r, t]);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let mut loops = Vec::new();
    for (k, segments) in by_k {
        for mut points in stitch(&segments) {
            if points.len() < 4 || dist(points[0], *points.last().expect("point")) > 1e-3 {
                continue; // open lines at the edge of the grid are not cut
            }
            // Counter-clockwise, so every loop is cut in the same direction.
            let area: f64 = points
                .windows(2)
                .map(|pair| pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1])
                .sum::<f64>()
                * 0.5;
            if area < 0.0 {
                points.reverse();
            }
            loops.push(Loop { k, points });
        }
    }
    loops.sort_by(|a, b| a.k.cmp(&b.k));
    loops
}

/// Join line segments end to end into polylines.
fn stitch(segments: &[[[f64; 2]; 2]]) -> Vec<Vec<[f64; 2]>> {
    let key = |p: [f64; 2]| ((p[0] * 1e4).round() as i64, (p[1] * 1e4).round() as i64);
    let mut adjacency: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
    for (index, segment) in segments.iter().enumerate() {
        adjacency.entry(key(segment[0])).or_default().push(index);
        adjacency.entry(key(segment[1])).or_default().push(index);
    }
    let mut used = vec![false; segments.len()];
    let mut result = Vec::new();
    let next_from = |adjacency: &HashMap<(i64, i64), Vec<usize>>,
                     used: &mut [bool],
                     point: [f64; 2]|
     -> Option<[f64; 2]> {
        let at = key(point);
        let candidates = adjacency.get(&at)?;
        let index = *candidates.iter().find(|index| !used[**index])?;
        used[index] = true;
        let segment = segments[index];
        Some(if key(segment[0]) == at {
            segment[1]
        } else {
            segment[0]
        })
    };
    for start in 0..segments.len() {
        if used[start] {
            continue;
        }
        used[start] = true;
        let mut line: VecDeque<[f64; 2]> = VecDeque::from([segments[start][0], segments[start][1]]);
        while let Some(next) = next_from(&adjacency, &mut used, *line.back().expect("point")) {
            line.push_back(next);
        }
        while let Some(next) = next_from(&adjacency, &mut used, *line.front().expect("point")) {
            line.push_front(next);
        }
        result.push(line.into_iter().collect());
    }
    result
}

/// Ramer–Douglas–Peucker on a closed polyline (first point == last point).
fn simplify_closed(points: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    if points.len() < 5 {
        return points.to_vec();
    }
    // Split at the point farthest from the start so both halves are open.
    let far = (1..points.len() - 1)
        .max_by(|&a, &b| dist(points[a], points[0]).total_cmp(&dist(points[b], points[0])))
        .expect("points");
    let mut first = rdp(&points[..=far], tolerance);
    let second = rdp(&points[far..], tolerance);
    first.extend_from_slice(&second[1..]);
    first
}

fn rdp(points: &[[f64; 2]], tolerance: f64) -> Vec<[f64; 2]> {
    if points.len() < 3 {
        return points.to_vec();
    }
    let (a, b) = (points[0], points[points.len() - 1]);
    let mut worst = (0.0, 0);
    for (index, p) in points.iter().enumerate().take(points.len() - 1).skip(1) {
        let deviation = segment_distance(*p, a, b);
        if deviation > worst.0 {
            worst = (deviation, index);
        }
    }
    if worst.0 <= tolerance {
        return vec![a, b];
    }
    let mut left = rdp(&points[..=worst.1], tolerance);
    let right = rdp(&points[worst.1..], tolerance);
    left.pop();
    left.extend(right);
    left
}

fn segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let length_sq = ab[0] * ab[0] + ab[1] * ab[1];
    if length_sq < 1e-18 {
        return dist(p, a);
    }
    let t = (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / length_sq).clamp(0.0, 1.0);
    dist(p, [a[0] + ab[0] * t, a[1] + ab[1] * t])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distance_transform_matches_brute_force() {
        let (cols, rows) = (23, 17);
        let mut blocked = vec![false; cols * rows];
        for index in 0..blocked.len() {
            let (c, r) = (index % cols, index / cols);
            blocked[index] = c == 0 || r == 0 || c == cols - 1 || r == rows - 1;
        }
        blocked[8 * cols + 12] = true;
        blocked[3 * cols + 5] = true;
        let fast = distance_to_blocked(&blocked, cols, rows);
        for index in 0..blocked.len() {
            let (c, r) = ((index % cols) as f64, (index / cols) as f64);
            let slow = blocked
                .iter()
                .enumerate()
                .filter(|(_, b)| **b)
                .map(|(i, _)| ((i % cols) as f64 - c).hypot((i / cols) as f64 - r))
                .fold(f64::INFINITY, f64::min);
            assert!(
                (fast[index] - slow).abs() < 1e-9,
                "cell {index}: {} vs {slow}",
                fast[index]
            );
        }
    }

    #[test]
    fn contours_of_a_square_room_are_nested_loops_one_step_apart() {
        let (cols, rows) = (41, 41);
        let mut blocked = vec![false; cols * rows];
        for index in 0..blocked.len() {
            let (c, r) = (index % cols, index / cols);
            blocked[index] = c == 0 || r == 0 || c == cols - 1 || r == rows - 1;
        }
        let distance = distance_to_blocked(&blocked, cols, rows);
        let loops = extract_loops(&distance, cols, rows, 1.0, 2.0);
        // Distances 1, 3, 5 … 19 from the wall: ten rings, all closed and CCW.
        let mut ks: Vec<usize> = loops.iter().map(|l| l.k).collect();
        ks.sort_unstable();
        ks.dedup();
        assert!(ks.len() >= 9, "{ks:?}");
        for item in &loops {
            let first = item.points[0];
            let last = *item.points.last().unwrap();
            assert!(dist(first, last) < 1e-3);
            let area: f64 = item
                .points
                .windows(2)
                .map(|pair| pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1])
                .sum::<f64>()
                * 0.5;
            assert!(area > 0.0, "loop {} is clockwise", item.k);
            // Every point of ring k is the same distance from the wall.
            let expected = 1.0 + item.k as f64 * 2.0;
            for p in &item.points {
                let to_wall = p[0].min(p[1]).min(40.0 - p[0]).min(40.0 - p[1]);
                assert!(
                    (to_wall - expected).abs() < 0.05,
                    "ring {} at {p:?}",
                    item.k
                );
            }
        }
    }

    use crate::cam::stock_model::InProcessStock;
    use crate::cam::toolpath::ToolMove;
    use crate::workspace::model::{MeshBody, StepLengthUnit};

    /// A closed box `lo..hi` in X/Y from z = 0 to `top`.
    fn block(lo: f32, hi: f32, top: f32) -> Model {
        let vertices = vec![
            [lo, lo, 0.0],
            [hi, lo, 0.0],
            [hi, hi, 0.0],
            [lo, hi, 0.0],
            [lo, lo, top],
            [hi, lo, top],
            [hi, hi, top],
            [lo, hi, top],
        ];
        let triangles = vec![
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
        ];
        let bounds = [[lo, lo, 0.0], [hi, hi, top]];
        Model {
            name: "block".into(),
            source_path: "fixture:block".into(),
            visible: true,
            source_schema: vec![],
            source_unit: StepLengthUnit::Millimeter,
            units_are_millimeters: true,
            transform: Default::default(),
            vertices,
            triangles,
            bodies: vec![MeshBody {
                name: "block".into(),
                source_item_id: 1,
                first_triangle: 0,
                triangle_count: 12,
                triangle_face_ids: (1..=12).collect(),
                transform: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
                bounds,
            }],
            bounds,
            import_notes: vec![],
        }
    }

    fn stock_setup() -> MachiningSetup {
        MachiningSetup {
            stock_size_mm: [60.0, 60.0, 12.0],
            boundary_margin_mm: 1.0,
            stepdown_mm: 2.0,
            // Coarse sampling keeps debug-build tests quick.
            path_tolerance_mm: 0.5,
            ..MachiningSetup::default()
        }
    }

    fn end_mill(diameter: f64) -> Tool {
        Tool {
            diameter_mm: diameter,
            flute_length_mm: 15.0,
            stickout_mm: 30.0,
            feed_rate_mm_min: 1200.0,
            plunge_rate_mm_min: 300.0,
            ..Tool::default()
        }
    }

    fn roughing_of_a_block() -> (Toolpath, Tool, MachiningSetup) {
        let model = block(20.0, 40.0, 7.0);
        let setup = stock_setup();
        let tool = end_mill(4.0);
        let params = AdaptiveParams::conservative(&setup, &tool, 0.0);
        let path = generate(&model, &setup, &tool, &params).expect("adaptive roughing");
        (path, tool, setup)
    }

    #[test]
    #[ignore]
    fn debug_probe_at_corner() {
        let model = block(20.0, 40.0, 7.0);
        let query = MeshQuery::new(&model);
        let engine = ProbeEngine::with_gpu(&query, None);
        for (x, y) in [(19.6, 41.6), (18.8, 41.2), (19.6, 42.0), (18.8, 41.6)] {
            let probe = Probe {
                x,
                y,
                limit: 6.0,
                radius: 2.2,
                ring: 16,
                center: true,
            };
            eprintln!("PROBE ({x},{y}) fits={:?}", engine.fits(&[probe]));
        }
    }

    #[test]
    fn roughing_a_block_clears_the_waste_and_never_touches_the_part() {
        let (path, tool, setup) = roughing_of_a_block();
        assert!(!path.is_empty());
        let mut stock = InProcessStock::from_setup(&setup).unwrap();
        stock.apply_path(&path, &tool);
        // The top of the part is untouched (no gouging) ...
        for (x, y) in [(30.0, 30.0), (22.0, 22.0), (38.0, 38.0), (30.0, 21.0)] {
            assert!(
                stock.height_at(x, y) >= 7.0 - 0.02,
                "part top gouged at ({x}, {y}): {}",
                stock.height_at(x, y)
            );
        }
        // ... the waste beside it is cleared to the floor, right up to the wall
        // (a 4 mm cutter can get within its radius of the 20 mm wall).
        for (x, y) in [
            (5.0, 5.0),
            (55.0, 30.0),
            (30.0, 6.0),
            (18.5, 30.0),
            (30.0, 41.5),
        ] {
            assert!(
                stock.height_at(x, y) <= 0.3,
                "waste left at ({x}, {y}): {}",
                stock.height_at(x, y)
            );
        }
    }

    #[test]
    fn entry_ramps_are_gentle_and_there_are_no_deep_plunges() {
        let (path, _, _) = roughing_of_a_block();
        let limit = MAX_RAMP_ANGLE_DEG.to_radians().tan() * 1.3;
        let mut position: Option<[f64; 3]> = None;
        let mut descending_moves = 0;
        for movement in &path.moves {
            match movement {
                ToolMove::Rapid { to, z } => position = Some([to[0], to[1], *z]),
                ToolMove::Feed { to, z, .. } => {
                    if let Some(start) = position {
                        let run = (to[0] - start[0]).hypot(to[1] - start[1]);
                        let drop = start[2] - *z;
                        if drop > 1e-6 {
                            descending_moves += 1;
                            if run < 1e-6 {
                                // A vertical move: only the short drop into
                                // space the cutter has already cleared.
                                assert!(drop <= 3.1, "plunge of {drop} mm");
                            } else {
                                assert!(
                                    drop / run <= limit,
                                    "descent slope {} over {run:.2} mm",
                                    drop / run
                                );
                            }
                        }
                    }
                    position = Some([to[0], to[1], *z]);
                }
                _ => {}
            }
        }
        assert!(descending_moves > 0);
    }

    #[test]
    fn feed_is_modulated_but_never_above_the_tool_feed() {
        let (path, tool, _) = roughing_of_a_block();
        let feeds: Vec<f64> = path
            .moves
            .iter()
            .filter_map(|m| match m {
                ToolMove::Feed { feed_mm_min, .. } => Some(*feed_mm_min),
                _ => None,
            })
            .collect();
        assert!(feeds
            .iter()
            .all(|feed| *feed <= tool.feed_rate_mm_min + 1e-9));
        assert!(feeds.iter().any(|feed| *feed < tool.feed_rate_mm_min * 0.9));
        // Never slower than the floor on the cutting passes (entry ramps use the
        // plunge rate, which is lower by design).
        assert!(feeds
            .iter()
            .all(|feed| *feed >= tool.feed_rate_mm_min * MIN_FEED_SCALE * 0.4 - 1e-9));
    }

    #[test]
    fn radial_stepover_is_conservative_whatever_the_setup_asks_for() {
        let mut setup = stock_setup();
        setup.stepover_percent = 60.0;
        let params = AdaptiveParams::conservative(&setup, &end_mill(6.0), 0.0);
        assert!(params.stepover_ratio <= MAX_RADIAL_STEPOVER + 1e-12);
        setup.ramp_angle_deg = 30.0;
        let params = AdaptiveParams::conservative(&setup, &end_mill(6.0), 0.0);
        assert!(params.ramp_angle_deg <= MAX_RAMP_ANGLE_DEG + 1e-12);
        // Axial depth is limited to half the cutter diameter.
        assert!(params.stepdown_mm <= 3.0 + 1e-12);
    }

    #[test]
    fn a_cutter_too_short_to_reach_the_part_bottom_is_rejected() {
        let model = block(20.0, 40.0, 7.0);
        let setup = stock_setup();
        let mut tool = end_mill(4.0);
        tool.stickout_mm = 5.0;
        tool.flute_length_mm = 5.0;
        let params = AdaptiveParams::conservative(&setup, &tool, 0.0);
        let error = generate(&model, &setup, &tool, &params).unwrap_err();
        assert!(error.contains("reach"), "{error}");
    }

    #[test]
    fn a_cancelled_job_stops_adaptive_roughing() {
        let model = block(20.0, 40.0, 7.0);
        let setup = stock_setup();
        let tool = end_mill(4.0);
        let params = AdaptiveParams::conservative(&setup, &tool, 0.0);
        let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let _control = cancel::install(cancel::Control {
            cancel: cancel_flag,
            progress: None,
        });
        let error = generate(&model, &setup, &tool, &params).unwrap_err();
        assert!(cancel::was_cancelled(&error), "{error}");
    }

    #[test]
    fn nominal_engagement_grows_with_stepover() {
        assert!(nominal_engagement(0.05) < nominal_engagement(0.12));
        assert!((nominal_engagement(0.5) - 0.25).abs() < 1e-9);
        assert!((nominal_engagement(0.12) - 0.1125).abs() < 0.002);
    }

    #[test]
    fn simplification_keeps_a_closed_loop_within_tolerance() {
        let circle: Vec<[f64; 2]> = (0..=200)
            .map(|step| {
                let a = step as f64 * std::f64::consts::TAU / 200.0;
                [10.0 * a.cos(), 10.0 * a.sin()]
            })
            .collect();
        let simple = simplify_closed(&circle, 0.02);
        assert!(simple.len() < circle.len());
        assert!(dist(simple[0], *simple.last().unwrap()) < 1e-9);
        for p in &circle {
            let nearest = simple
                .windows(2)
                .map(|pair| segment_distance(*p, pair[0], pair[1]))
                .fold(f64::INFINITY, f64::min);
            assert!(nearest <= 0.021, "{nearest}");
        }
    }
}
