//! Helical bore milling.
//!
//! Low-torque, high-RPM spindles drill poorly, so holes are *milled*: a
//! smaller end mill circles inside the hole while it descends in a helix. The
//! cutter size is assumed from the hole diameter (no tool-library lookup).
//!
//! * Holes up to about twice the cutter diameter are bored with one
//!   continuous helix on the wall radius; the cutter overlaps the hole centre
//!   so no slug is left behind.
//! * Larger holes are cleared layer by layer: a tight helix ramps to the new
//!   layer, an outward spiral clears the floor and a full circle finishes the
//!   wall.
//!
//! The helix is emitted as short straight feed moves so the same path posts
//! on GRBL/LinuxCNC and drives the simulation and stock preview unchanged.

use serde::{Deserialize, Serialize};

use crate::cam::{tool::Tool, toolpath::Toolpath};

/// Standard end-mill diameters (mm) the planner assumes for bores.
pub const ASSUMED_BIT_SIZES_MM: [f64; 11] =
    [0.8, 1.0, 1.5, 2.0, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0];

/// Smallest cutter-centre orbit radius that still produces a bore.
const MIN_ORBIT_RADIUS_MM: f64 = 0.05;
/// Largest cutter diameter relative to the hole, leaving room to orbit.
const MAX_BIT_TO_HOLE_RATIO: f64 = 0.8;
/// Minimum radial room (diameter difference) between cutter and hole.
const MIN_DIAMETER_MARGIN_MM: f64 = 0.5;
const MAX_LAYERS: usize = 512;

/// A cylindrical hole to mill, in absolute work coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BoreFeature {
    pub center: [f64; 2],
    pub diameter_mm: f64,
    pub top_z: f64,
    pub bottom_z: f64,
}

impl BoreFeature {
    pub fn depth_mm(&self) -> f64 {
        self.top_z - self.bottom_z
    }
}

#[derive(Debug, Clone, Copy)]
pub struct BoreParams {
    pub stepdown_mm: f64,
    pub stepover_ratio: f64,
    pub path_tolerance_mm: f64,
    /// Descent angle of the helix in degrees (the slope of the ramp it cuts).
    /// Low angles are gentle on the cutter and a low-torque spindle.
    pub ramp_angle_deg: f64,
}

/// Default helix descent angle: shallow, to keep the cutter lightly loaded.
pub const DEFAULT_RAMP_ANGLE_DEG: f64 = 2.0;

impl Default for BoreParams {
    fn default() -> Self {
        Self {
            stepdown_mm: 2.0,
            stepover_ratio: 0.4,
            path_tolerance_mm: 0.02,
            ramp_angle_deg: DEFAULT_RAMP_ANGLE_DEG,
        }
    }
}

/// Smallest hole diameter that can be bored with the assumed bit sizes.
pub fn min_bore_diameter_mm() -> f64 {
    // The smallest bit must fit with the required margin.
    (ASSUMED_BIT_SIZES_MM[0] / MAX_BIT_TO_HOLE_RATIO)
        .max(ASSUMED_BIT_SIZES_MM[0] + MIN_DIAMETER_MARGIN_MM)
}

/// Pick the assumed end-mill diameter for a hole: the largest standard size
/// that still leaves room to orbit, or `None` when the hole is too small.
pub fn assumed_bit_diameter(hole_diameter_mm: f64) -> Option<f64> {
    if !hole_diameter_mm.is_finite() {
        return None;
    }
    let limit =
        (hole_diameter_mm * MAX_BIT_TO_HOLE_RATIO).min(hole_diameter_mm - MIN_DIAMETER_MARGIN_MM);
    ASSUMED_BIT_SIZES_MM
        .iter()
        .rev()
        .copied()
        .find(|size| *size <= limit + 1e-9)
}

/// Mill every bore, visiting nearest holes first.
pub fn generate_bores(
    features: &[BoreFeature],
    tool: &Tool,
    params: &BoreParams,
) -> Result<Toolpath, String> {
    tool.validate()?;
    if !params.stepdown_mm.is_finite()
        || params.stepdown_mm <= 0.0
        || !params.stepover_ratio.is_finite()
        || params.stepover_ratio <= 0.0
        || params.stepover_ratio > 1.0
        || !params.path_tolerance_mm.is_finite()
        || params.path_tolerance_mm <= 0.0
        || !params.ramp_angle_deg.is_finite()
        || !(0.2..=45.0).contains(&params.ramp_angle_deg)
    {
        return Err("helical bore parameters must be positive".into());
    }
    let mut remaining: Vec<BoreFeature> = features.to_vec();
    let mut path = Toolpath::default();
    let mut position = [0.0, 0.0];
    while !remaining.is_empty() {
        let (index, _) = remaining
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                distance(position, a.center).total_cmp(&distance(position, b.center))
            })
            .expect("non-empty");
        let feature = remaining.remove(index);
        bore_one(&mut path, &feature, tool, params)?;
        position = feature.center;
    }
    Ok(path)
}

fn bore_one(
    path: &mut Toolpath,
    feature: &BoreFeature,
    tool: &Tool,
    params: &BoreParams,
) -> Result<(), String> {
    if !feature.diameter_mm.is_finite()
        || !feature.top_z.is_finite()
        || !feature.bottom_z.is_finite()
        || feature.center.iter().any(|value| !value.is_finite())
    {
        return Err("bore feature has non-finite values".into());
    }
    if feature.depth_mm() <= 1e-6 {
        return Err("bore has no depth".into());
    }
    let tool_radius = tool.diameter_mm * 0.5;
    let orbit = feature.diameter_mm * 0.5 - tool_radius;
    if orbit < MIN_ORBIT_RADIUS_MM {
        return Err(format!(
            "a {:.2} mm hole is too small to helically bore with a {:.2} mm cutter",
            feature.diameter_mm, tool.diameter_mm
        ));
    }
    let center = feature.center;
    // Depth gained per revolution on a helix of radius `r` at the ramp angle.
    let ramp_tan = params.ramp_angle_deg.to_radians().tan();
    let pitch_at = |radius: f64| {
        (std::f64::consts::TAU * radius * ramp_tan)
            .min(tool.diameter_mm * 0.25)
            .clamp(0.05, params.stepdown_mm.max(0.05))
    };
    let lateral_feed = tool.feed_rate_mm_min;
    let helix_feed = tool.plunge_rate_mm_min;
    let on_circle = |radius: f64, angle: f64| {
        [
            center[0] + radius * angle.cos(),
            center[1] + radius * angle.sin(),
        ]
    };
    let spiral = Spiral {
        center,
        tolerance: params.path_tolerance_mm,
    };

    if orbit <= tool_radius {
        // One helix on the wall radius; the cutter overlaps the centre.
        path.push_rapid(on_circle(orbit, 0.0), feature.top_z);
        let turns = (feature.depth_mm() / pitch_at(orbit)).max(0.25);
        let angle = spiral.run(
            path,
            (orbit, orbit),
            (feature.top_z, feature.bottom_z),
            0.0,
            turns,
            helix_feed,
        );
        // A full circle at the bottom flattens the helix ramp.
        spiral.run(
            path,
            (orbit, orbit),
            (feature.bottom_z, feature.bottom_z),
            angle,
            1.0,
            lateral_feed,
        );
        return Ok(());
    }

    // Large hole: ramp, spiral out, finish the wall, one layer at a time.
    let entry = (0.75 * tool_radius).min(orbit);
    let ring_step = (tool.diameter_mm * params.stepover_ratio).max(0.05);
    let layer_depth = params.stepdown_mm.min(tool.diameter_mm * 0.5).max(0.1);
    path.push_rapid(on_circle(entry, 0.0), feature.top_z);
    let mut z_previous = feature.top_z;
    let mut angle = 0.0;
    for layer in 0..MAX_LAYERS {
        let z_next = (z_previous - layer_depth).max(feature.bottom_z);
        if layer > 0 {
            // Back to the entry radius across the floor already cut.
            path.push_feed(on_circle(entry, angle), z_previous, lateral_feed);
        }
        let ramp_turns = ((z_previous - z_next) / pitch_at(entry)).max(0.25);
        angle = spiral.run(
            path,
            (entry, entry),
            (z_previous, z_next),
            angle,
            ramp_turns,
            helix_feed,
        );
        let clear_turns = ((orbit - entry) / ring_step).max(0.25);
        angle = spiral.run(
            path,
            (entry, orbit),
            (z_next, z_next),
            angle,
            clear_turns,
            lateral_feed,
        );
        angle = spiral.run(
            path,
            (orbit, orbit),
            (z_next, z_next),
            angle,
            1.0,
            lateral_feed,
        );
        z_previous = z_next;
        if z_next <= feature.bottom_z + 1e-9 {
            return Ok(());
        }
    }
    Err("bore needs too many layers; increase the stepdown".into())
}

/// Emits a spiral/helix as short straight feed moves.
struct Spiral {
    center: [f64; 2],
    tolerance: f64,
}

impl Spiral {
    /// Run `turns` revolutions from `angle`, moving the radius and Z linearly.
    /// Returns the final angle.
    fn run(
        &self,
        path: &mut Toolpath,
        radius: (f64, f64),
        z: (f64, f64),
        angle: f64,
        turns: f64,
        feed: f64,
    ) -> f64 {
        let widest = radius.0.max(radius.1).max(self.tolerance * 2.0);
        let step = 2.0 * (1.0 - self.tolerance / widest).clamp(-1.0, 1.0).acos();
        let per_turn = ((std::f64::consts::TAU / step.max(1e-3)).ceil() as usize).clamp(16, 120);
        let segments = ((turns * per_turn as f64).ceil() as usize).max(1);
        let total_angle = turns * std::f64::consts::TAU;
        for index in 1..=segments {
            let t = index as f64 / segments as f64;
            let theta = angle + total_angle * t;
            let r = radius.0 + (radius.1 - radius.0) * t;
            path.push_feed(
                [
                    self.center[0] + r * theta.cos(),
                    self.center[1] + r * theta.sin(),
                ],
                z.0 + (z.1 - z.0) * t,
                feed,
            );
        }
        angle + total_angle
    }
}

fn distance(a: [f64; 2], b: [f64; 2]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cam::toolpath::ToolMove;

    fn tool(diameter: f64) -> Tool {
        Tool {
            diameter_mm: diameter,
            flute_length_mm: 20.0,
            stickout_mm: 30.0,
            ..Tool::default()
        }
    }

    fn feed_points(path: &Toolpath) -> Vec<[f64; 3]> {
        path.moves
            .iter()
            .filter_map(|movement| match movement {
                ToolMove::Feed { to, z, .. } => Some([to[0], to[1], *z]),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bit_size_is_assumed_from_the_hole_and_tiny_holes_are_refused() {
        assert_eq!(assumed_bit_diameter(10.0), Some(8.0));
        assert_eq!(assumed_bit_diameter(4.0), Some(3.0));
        assert_eq!(assumed_bit_diameter(2.0), Some(1.5));
        assert_eq!(assumed_bit_diameter(1.3), Some(0.8));
        assert_eq!(assumed_bit_diameter(1.0), None);
        assert_eq!(assumed_bit_diameter(f64::NAN), None);
        assert!((min_bore_diameter_mm() - 1.3).abs() < 1e-9);
        // Never larger than the hole allows.
        for hole in [1.4, 2.5, 5.0, 9.0, 16.0, 40.0] {
            let bit = assumed_bit_diameter(hole).unwrap();
            assert!(bit <= hole * 0.8 + 1e-9 && bit <= hole - 0.5 + 1e-9);
        }
    }

    #[test]
    fn small_hole_is_one_helix_on_the_wall_radius_ending_at_the_bottom() {
        let feature = BoreFeature {
            center: [10.0, 20.0],
            diameter_mm: 6.0,
            top_z: 5.0,
            bottom_z: -3.0,
        };
        let path = generate_bores(&[feature], &tool(3.0), &BoreParams::default()).unwrap();
        // Positioned above the hole, never plunging straight down.
        assert!(matches!(path.moves[0], ToolMove::Rapid { z, .. } if (z - 5.0).abs() < 1e-9));
        let points = feed_points(&path);
        assert!(points.len() > 30);
        let orbit = 1.5; // (6 - 3) / 2
        for point in &points {
            let radius = (point[0] - 10.0).hypot(point[1] - 20.0);
            assert!((radius - orbit).abs() < 1e-6, "radius {radius}");
            assert!(point[2] <= 5.0 + 1e-9 && point[2] >= -3.0 - 1e-9);
        }
        // Z never rises, and the hole is bored to full depth.
        assert!(points
            .windows(2)
            .all(|pair| pair[1][2] <= pair[0][2] + 1e-9));
        assert!((points.last().unwrap()[2] + 3.0).abs() < 1e-9);
        // No vertical plunge: every feed move changes XY.
        assert!(!path
            .moves
            .iter()
            .any(|m| matches!(m, ToolMove::Drill { .. })));
    }

    #[test]
    fn the_helix_never_descends_steeper_than_the_ramp_angle() {
        let feature = BoreFeature {
            center: [0.0, 0.0],
            diameter_mm: 14.0,
            top_z: 0.0,
            bottom_z: -3.0,
        };
        let params = BoreParams {
            ramp_angle_deg: 2.0,
            ..BoreParams::default()
        };
        let path = generate_bores(&[feature], &tool(6.0), &params).unwrap();
        let points = feed_points(&path);
        // Slope of every move that descends: depth gained over horizontal run.
        let steepest = points
            .windows(2)
            .filter(|pair| pair[1][2] < pair[0][2] - 1e-9)
            .map(|pair| {
                let run = (pair[1][0] - pair[0][0]).hypot(pair[1][1] - pair[0][1]);
                (pair[0][2] - pair[1][2]) / run.max(1e-9)
            })
            .fold(0.0_f64, f64::max);
        assert!(
            steepest <= params.ramp_angle_deg.to_radians().tan() * 1.15,
            "steepest descent slope {steepest}"
        );
        // A steeper request is rejected only outside the supported range.
        assert!(generate_bores(
            &[feature],
            &tool(6.0),
            &BoreParams {
                ramp_angle_deg: 80.0,
                ..params
            }
        )
        .is_err());
    }

    #[test]
    fn helix_descends_by_a_limited_pitch_per_revolution() {
        let feature = BoreFeature {
            center: [0.0, 0.0],
            diameter_mm: 8.0,
            top_z: 0.0,
            bottom_z: -4.0,
        };
        let path = generate_bores(&[feature], &tool(4.0), &BoreParams::default()).unwrap();
        let points = feed_points(&path);
        // Largest Z drop between neighbouring moves stays far below the pitch.
        let max_step = points
            .windows(2)
            .map(|pair| pair[0][2] - pair[1][2])
            .fold(0.0_f64, f64::max);
        assert!(max_step < 0.1, "step {max_step}");
    }

    #[test]
    fn large_hole_is_cleared_layer_by_layer_and_stays_inside_the_wall() {
        let feature = BoreFeature {
            center: [0.0, 0.0],
            diameter_mm: 30.0,
            top_z: 0.0,
            bottom_z: -4.0,
        };
        let cutter = tool(6.0);
        let path = generate_bores(&[feature], &cutter, &BoreParams::default()).unwrap();
        let points = feed_points(&path);
        let orbit = 15.0 - 3.0;
        assert!(points.iter().all(|p| p[0].hypot(p[1]) <= orbit + 1e-6));
        // The wall radius is reached on every layer floor, including the last.
        let at_wall_at_bottom = points
            .iter()
            .any(|p| (p[0].hypot(p[1]) - orbit).abs() < 1e-6 && (p[2] + 4.0).abs() < 1e-9);
        assert!(at_wall_at_bottom);
        // Layers are stepped: more than one distinct floor height.
        let mut floors: Vec<i64> = points
            .iter()
            .filter(|p| (p[0].hypot(p[1]) - orbit).abs() < 1e-6)
            .map(|p| (p[2] * 1000.0).round() as i64)
            .collect();
        floors.sort_unstable();
        floors.dedup();
        assert!(floors.len() >= 2);
        // The centre is covered: the innermost orbit is within the cutter radius.
        let innermost = points
            .iter()
            .map(|p| p[0].hypot(p[1]))
            .fold(f64::INFINITY, f64::min);
        assert!(innermost <= 3.0);
    }

    #[test]
    fn holes_too_small_for_the_cutter_are_rejected() {
        let feature = BoreFeature {
            center: [0.0, 0.0],
            diameter_mm: 3.0,
            top_z: 0.0,
            bottom_z: -2.0,
        };
        let error = generate_bores(&[feature], &tool(3.0), &BoreParams::default()).unwrap_err();
        assert!(error.contains("too small"));
        let flat = BoreFeature {
            top_z: -2.0,
            bottom_z: -2.0,
            diameter_mm: 8.0,
            ..feature
        };
        assert!(generate_bores(&[flat], &tool(3.0), &BoreParams::default()).is_err());
    }

    #[test]
    fn nearest_hole_is_bored_first() {
        let near = BoreFeature {
            center: [1.0, 0.0],
            diameter_mm: 6.0,
            top_z: 0.0,
            bottom_z: -1.0,
        };
        let far = BoreFeature {
            center: [50.0, 0.0],
            ..near
        };
        let path = generate_bores(&[far, near], &tool(3.0), &BoreParams::default()).unwrap();
        let first_rapid = path.moves.iter().find_map(|m| match m {
            ToolMove::Rapid { to, .. } => Some(*to),
            _ => None,
        });
        assert!((first_rapid.unwrap()[0] - (1.0 + 1.5)).abs() < 1e-6);
    }
}
