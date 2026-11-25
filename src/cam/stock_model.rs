//! In-process stock model: a 2.5D height-field of the material that is left
//! after the operations simulated so far.
//!
//! The planner replays every generated toolpath against this model. Each move
//! is first checked against the stock as it is *at that moment* (dynamic
//! collision detection) and is then cut into it, so later operations see what
//! earlier ones removed. The model is a height-field, so it represents 3-axis
//! work from above only and, like the rest of the STEP machining code, is an
//! approximation (sampled cutter envelope, finite cell size).

use crate::cam::{
    tool::{Tool, ToolShape},
    toolpath::{ToolMove, Toolpath},
};
use crate::workspace::setup::{MachiningSetup, WorkAxis};

/// Largest number of height cells kept for one stock.
const MAX_CELLS: usize = 640 * 640;
/// Vertical tolerance (mm) before stock above the tool counts as a collision.
const COLLISION_TOLERANCE_MM: f64 = 0.02;
/// Most collision reports kept per path so one bad pass cannot flood the log.
const MAX_ISSUES: usize = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionKind {
    /// The non-cutting part of the tool (shank/holder) touches uncut stock.
    Holder,
    /// A rapid move drives the tool into uncut stock.
    Rapid,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CollisionIssue {
    pub move_index: usize,
    pub kind: CollisionKind,
    pub position: [f64; 3],
}

impl CollisionIssue {
    pub fn describe(&self) -> String {
        let what = match self.kind {
            CollisionKind::Holder => "tool holder/shank would hit uncut stock",
            CollisionKind::Rapid => "rapid move would plunge into uncut stock",
        };
        format!(
            "move {}: {what} at X{:.2} Y{:.2} Z{:.2}",
            self.move_index + 1,
            self.position[0],
            self.position[1],
            self.position[2]
        )
    }
}

#[derive(Debug, Clone)]
pub struct InProcessStock {
    origin: [f64; 2],
    cell_mm: f64,
    columns: usize,
    rows: usize,
    heights: Vec<f32>,
    top_z: f64,
    floor_z: f64,
}

impl InProcessStock {
    /// Full, uncut stock for a Z-up setup.
    pub fn from_setup(setup: &MachiningSetup) -> Result<Self, String> {
        setup.validate()?;
        if setup.axis != WorkAxis::ZUp {
            return Err("the in-process stock model requires the Z-up work axis".into());
        }
        let size = setup.stock_size_mm;
        let longest = size[0].max(size[1]);
        let mut cell_mm = (longest / 256.0).clamp(0.25, 1.0);
        loop {
            let columns = (size[0] / cell_mm).ceil() as usize + 1;
            let rows = (size[1] / cell_mm).ceil() as usize + 1;
            if columns.saturating_mul(rows) <= MAX_CELLS {
                let top_z = setup.stock_origin_mm[2] + size[2];
                return Ok(Self {
                    origin: [setup.stock_origin_mm[0], setup.stock_origin_mm[1]],
                    cell_mm,
                    columns,
                    rows,
                    heights: vec![top_z as f32; columns * rows],
                    top_z,
                    floor_z: setup.stock_origin_mm[2],
                });
            }
            cell_mm *= 1.25;
        }
    }

    pub fn cell_mm(&self) -> f64 {
        self.cell_mm
    }

    pub fn top_z(&self) -> f64 {
        self.top_z
    }

    pub fn floor_z(&self) -> f64 {
        self.floor_z
    }

    /// Stock height at an XY position; outside the stock footprint there is no
    /// material, so the floor height is returned.
    pub fn height_at(&self, x: f64, y: f64) -> f64 {
        match self.cell_index(x, y) {
            Some(index) => self.heights[index] as f64,
            None => self.floor_z,
        }
    }

    /// Highest stock sampled at the centre and around the rim of a disc.
    pub fn max_height_in_disc(&self, x: f64, y: f64, radius: f64) -> f64 {
        let mut highest = self.height_at(x, y);
        if radius <= 1e-9 {
            return highest;
        }
        let samples =
            ((std::f64::consts::TAU * radius / self.cell_mm).ceil() as usize).clamp(8, 48);
        for step in 0..samples {
            let angle = step as f64 * std::f64::consts::TAU / samples as f64;
            highest =
                highest.max(self.height_at(x + radius * angle.cos(), y + radius * angle.sin()));
        }
        // A mid-radius ring catches ridges thinner than the disc itself.
        if radius > 2.0 * self.cell_mm {
            for step in 0..samples / 2 {
                let angle = step as f64 * std::f64::consts::TAU / (samples / 2) as f64;
                highest = highest.max(self.height_at(
                    x + radius * 0.5 * angle.cos(),
                    y + radius * 0.5 * angle.sin(),
                ));
            }
        }
        highest
    }

    /// Highest stock found anywhere in a disc (every cell, not just samples).
    pub fn max_height_in_disc_exact(&self, x: f64, y: f64, radius: f64) -> f64 {
        let mut highest = f64::NEG_INFINITY;
        self.for_cells_in_disc(x, y, radius, |_, _, index| {
            highest = highest.max(self.heights[index] as f64);
        });
        if highest.is_finite() {
            highest
        } else {
            self.height_at(x, y)
        }
    }

    /// Replay a toolpath: every move is checked against the stock as it stands
    /// *before* that move, then cut into it. Returns the collisions found.
    pub fn simulate(
        &mut self,
        path: &Toolpath,
        tool: &Tool,
        check_collisions: bool,
    ) -> Vec<CollisionIssue> {
        if !check_collisions {
            self.apply_path(path, tool);
            return Vec::new();
        }
        let mut issues = Vec::new();
        let mut position: Option<[f64; 3]> = None;
        for (index, movement) in path.moves.iter().enumerate() {
            if index % 256 == 0 && crate::cam::cancel::is_cancelled() {
                break;
            }
            match movement {
                ToolMove::Rapid { to, z } => {
                    let target = [to[0], to[1], *z];
                    if check_collisions {
                        self.check_rapid(index, target, tool, &mut issues);
                    }
                    position = Some(target);
                }
                ToolMove::Feed { to, z, .. } | ToolMove::Arc { to, z, .. } => {
                    let target = [to[0], to[1], *z];
                    let start = position.unwrap_or(target);
                    self.sweep(index, start, target, tool, check_collisions, &mut issues);
                    position = Some(target);
                }
                ToolMove::Drill {
                    at, depth, retract, ..
                } => {
                    let top = [at[0], at[1], *retract];
                    let bottom = [at[0], at[1], *depth];
                    self.sweep(index, top, bottom, tool, check_collisions, &mut issues);
                    position = Some(top);
                }
            }
        }
        issues
    }

    /// Cut a toolpath without checking it. The sweep of the cutter along every
    /// move runs on the GPU when one is available (see [`crate::accel`]).
    pub fn apply_path(&mut self, path: &Toolpath, tool: &Tool) {
        let mut cuts: Vec<crate::accel::Cut> = Vec::with_capacity(path.moves.len());
        let mut position: Option<[f64; 3]> = None;
        for movement in &path.moves {
            match movement {
                ToolMove::Rapid { to, z } => position = Some([to[0], to[1], *z]),
                ToolMove::Feed { to, z, .. } | ToolMove::Arc { to, z, .. } => {
                    let end = [to[0], to[1], *z];
                    let start = position.unwrap_or(end);
                    cuts.push([start.map(|v| v as f32), end.map(|v| v as f32)]);
                    position = Some(end);
                }
                ToolMove::Drill {
                    at, depth, retract, ..
                } => {
                    cuts.push([
                        [at[0] as f32, at[1] as f32, *retract as f32],
                        [at[0] as f32, at[1] as f32, *depth as f32],
                    ]);
                    position = Some([at[0], at[1], *retract]);
                }
            }
        }
        let spec = crate::accel::GridSpec {
            origin: self.origin,
            cell: [self.cell_mm, self.cell_mm],
            columns: self.columns,
            rows: self.rows,
            floor: self.floor_z as f32,
        };
        let cutter = crate::accel::CutTool::from_tool(tool);
        crate::accel::cut_capsules(&spec, &mut self.heights, &cuts, cutter);
    }

    fn check_rapid(
        &self,
        index: usize,
        target: [f64; 3],
        tool: &Tool,
        issues: &mut Vec<CollisionIssue>,
    ) {
        if issues.len() >= MAX_ISSUES {
            return;
        }
        // The controller retracts to a safe height, travels in XY and then
        // descends, so only the final descent can meet uncut stock.
        let radius = self.envelope_radius(tool.diameter_mm * 0.5);
        if self.max_height_in_disc(target[0], target[1], radius)
            > target[2] + COLLISION_TOLERANCE_MM
        {
            issues.push(CollisionIssue {
                move_index: index,
                kind: CollisionKind::Rapid,
                position: target,
            });
        }
    }

    fn sweep(
        &mut self,
        index: usize,
        start: [f64; 3],
        end: [f64; 3],
        tool: &Tool,
        check_collisions: bool,
        issues: &mut Vec<CollisionIssue>,
    ) {
        let length = ((end[0] - start[0]).powi(2)
            + (end[1] - start[1]).powi(2)
            + (end[2] - start[2]).powi(2))
        .sqrt();
        let spacing = (self.cell_mm * 0.75).max(0.05);
        let samples = (length / spacing).ceil().max(1.0) as usize;
        let radius = tool.diameter_mm * 0.5;
        let holder_radius = self.envelope_radius(radius + tool.holder_clearance_mm);
        let mut reported = false;
        for step in 0..=samples {
            let t = step as f64 / samples as f64;
            let point = [
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
                start[2] + (end[2] - start[2]) * t,
            ];
            if check_collisions && !reported && issues.len() < MAX_ISSUES {
                let holder_bottom = point[2] + tool.flute_length_mm;
                if self.max_height_in_disc(point[0], point[1], holder_radius)
                    > holder_bottom + COLLISION_TOLERANCE_MM
                {
                    issues.push(CollisionIssue {
                        move_index: index,
                        kind: CollisionKind::Holder,
                        position: point,
                    });
                    reported = true;
                }
            }
            self.cut_at(point, tool);
        }
    }

    /// Collision envelopes are shrunk by one cell so a tool that merely touches
    /// a wall (stock at exactly its radius) is not reported as a collision.
    fn envelope_radius(&self, radius: f64) -> f64 {
        (radius - self.cell_mm).max(0.0)
    }

    /// Lower the stock under the cutter at one tool-tip position.
    fn cut_at(&mut self, point: [f64; 3], tool: &Tool) {
        let radius = tool.diameter_mm * 0.5;
        let tip = point[2];
        if tip >= self.top_z {
            return;
        }
        let profile = crate::accel::CutTool::from_tool(tool);
        let floor = self.floor_z as f32;
        let min_column = ((point[0] - radius - self.origin[0]) / self.cell_mm).floor() as isize;
        let max_column = ((point[0] + radius - self.origin[0]) / self.cell_mm).ceil() as isize;
        let min_row = ((point[1] - radius - self.origin[1]) / self.cell_mm).floor() as isize;
        let max_row = ((point[1] + radius - self.origin[1]) / self.cell_mm).ceil() as isize;
        let mut cut_any = false;
        // Written straight into the height-field: this runs for every sample of
        // every move, so it must not allocate.
        for row in min_row.max(0)..=max_row.min(self.rows as isize - 1) {
            let cy = self.origin[1] + row as f64 * self.cell_mm;
            for column in min_column.max(0)..=max_column.min(self.columns as isize - 1) {
                let cx = self.origin[0] + column as f64 * self.cell_mm;
                let (dx, dy) = (cx - point[0], cy - point[1]);
                let distance = dx.hypot(dy);
                if distance > radius + 1e-9 {
                    continue;
                }
                cut_any = true;
                let surface = profile.profile(distance.min(radius));
                let height = ((tip + surface) as f32).max(floor);
                let index = row as usize * self.columns + column as usize;
                if height < self.heights[index] {
                    self.heights[index] = height;
                }
            }
        }
        if !cut_any {
            // Cutter smaller than a cell: still cut the cell under its centre.
            if let Some(index) = self.cell_index(point[0], point[1]) {
                let height = (tip as f32).max(floor);
                if height < self.heights[index] {
                    self.heights[index] = height;
                }
            }
        }
    }

    /// Visit every cell whose centre is inside a disc (always at least the cell
    /// under the disc centre). `visit` receives the cell-centre offset from the
    /// disc centre and the cell's index.
    fn for_cells_in_disc(
        &self,
        x: f64,
        y: f64,
        radius: f64,
        mut visit: impl FnMut(f64, f64, usize),
    ) {
        let min_column = ((x - radius - self.origin[0]) / self.cell_mm).floor() as isize;
        let max_column = ((x + radius - self.origin[0]) / self.cell_mm).ceil() as isize;
        let min_row = ((y - radius - self.origin[1]) / self.cell_mm).floor() as isize;
        let max_row = ((y + radius - self.origin[1]) / self.cell_mm).ceil() as isize;
        let mut visited = false;
        for row in min_row.max(0)..=max_row.min(self.rows as isize - 1) {
            for column in min_column.max(0)..=max_column.min(self.columns as isize - 1) {
                let cx = self.origin[0] + column as f64 * self.cell_mm;
                let cy = self.origin[1] + row as f64 * self.cell_mm;
                let (dx, dy) = (cx - x, cy - y);
                if dx.hypot(dy) <= radius + 1e-9 {
                    visited = true;
                    visit(dx, dy, row as usize * self.columns + column as usize);
                }
            }
        }
        if !visited {
            if let Some(index) = self.cell_index(x, y) {
                visit(0.0, 0.0, index);
            }
        }
    }

    fn cell_index(&self, x: f64, y: f64) -> Option<usize> {
        let column = ((x - self.origin[0]) / self.cell_mm).round();
        let row = ((y - self.origin[1]) / self.cell_mm).round();
        if column < 0.0 || row < 0.0 || column >= self.columns as f64 || row >= self.rows as f64 {
            return None;
        }
        Some(row as usize * self.columns + column as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> MachiningSetup {
        MachiningSetup {
            stock_size_mm: [40.0, 40.0, 10.0],
            ..MachiningSetup::default()
        }
    }

    fn tool(diameter: f64) -> Tool {
        Tool {
            diameter_mm: diameter,
            flute_length_mm: 6.0,
            stickout_mm: 20.0,
            ..Tool::default()
        }
    }

    #[test]
    fn untouched_stock_is_flat_at_the_top() {
        let stock = InProcessStock::from_setup(&setup()).unwrap();
        assert!((stock.height_at(20.0, 20.0) - 10.0).abs() < 1e-6);
        // Outside the footprint there is no material.
        assert!((stock.height_at(-5.0, 20.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn a_feed_move_removes_stock_under_the_cutter() {
        let mut stock = InProcessStock::from_setup(&setup()).unwrap();
        let mut path = Toolpath::default();
        path.push_rapid([10.0, 20.0], 12.0);
        path.push_feed([10.0, 20.0], 6.0, 200.0);
        path.push_feed([30.0, 20.0], 6.0, 800.0);
        stock.apply_path(&path, &tool(4.0));
        assert!((stock.height_at(20.0, 20.0) - 6.0).abs() < 1e-6);
        assert!((stock.height_at(20.0, 21.5) - 6.0).abs() < 1e-6);
        // Outside the cutter radius the stock is untouched.
        assert!((stock.height_at(20.0, 23.0) - 10.0).abs() < 1e-6);
    }

    #[test]
    fn rapid_plunge_into_uncut_stock_is_a_collision() {
        let mut stock = InProcessStock::from_setup(&setup()).unwrap();
        let mut path = Toolpath::default();
        path.push_rapid([20.0, 20.0], 4.0);
        let issues = stock.simulate(&path, &tool(4.0), true);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].kind, CollisionKind::Rapid);
        // A rapid that stays above the stock is fine.
        let mut clear = Toolpath::default();
        clear.push_rapid([20.0, 20.0], 12.0);
        assert!(stock.simulate(&clear, &tool(4.0), true).is_empty());
    }

    #[test]
    fn deep_plunge_beyond_the_flute_length_hits_the_holder() {
        let mut stock = InProcessStock::from_setup(&setup()).unwrap();
        let mut path = Toolpath::default();
        path.push_rapid([20.0, 20.0], 12.0);
        // Flute is 6 mm; going down to Z=2 leaves 8 mm of tool in the stock.
        // The holder is wider than the cutter, so it cannot follow it into the
        // narrow hole the cutter makes.
        path.push_feed([20.0, 20.0], 2.0, 100.0);
        let mut wide_holder = tool(4.0);
        wide_holder.holder_clearance_mm = 3.0;
        let issues = stock.simulate(&path, &wide_holder, true);
        assert!(issues
            .iter()
            .any(|issue| issue.kind == CollisionKind::Holder));
    }

    #[test]
    fn collision_checks_can_be_turned_off() {
        let mut stock = InProcessStock::from_setup(&setup()).unwrap();
        let mut path = Toolpath::default();
        path.push_rapid([20.0, 20.0], 4.0);
        assert!(stock.simulate(&path, &tool(4.0), false).is_empty());
    }

    #[test]
    fn ball_nose_leaves_a_curved_floor() {
        let mut stock = InProcessStock::from_setup(&setup()).unwrap();
        let mut ball = tool(4.0);
        ball.shape = ToolShape::BallNose;
        let mut path = Toolpath::default();
        path.push_rapid([20.0, 20.0], 12.0);
        path.push_feed([20.0, 20.0], 5.0, 100.0);
        stock.apply_path(&path, &ball);
        assert!((stock.height_at(20.0, 20.0) - 5.0).abs() < 1e-6);
        // Away from the tip the ball nose has cut less deeply.
        let off_centre = stock.height_at(21.75, 20.0);
        assert!(off_centre > 5.5 && off_centre < 6.5, "{off_centre}");
    }
}
