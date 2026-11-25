//! Lighting for the 3D view.
//!
//! The scene is drawn from vertex colours, so all lighting is baked on the CPU:
//! a hemisphere ambient term, a warm key light, a cool fill, a soft rim and a
//! specular highlight, plus ambient occlusion and cast shadows sampled from a
//! height-field of the part or the cut stock.
//!
//! Colours handed in are display (sRGB) colours. Lighting is done in linear
//! light, and the result is gamma-encoded unless the render target already
//! encodes sRGB itself.

use std::sync::atomic::{AtomicBool, Ordering};

static TARGET_IS_SRGB: AtomicBool = AtomicBool::new(false);

/// Tell the lighting whether the framebuffer encodes sRGB on write (then colours
/// must be left linear) or stores them as they are.
pub fn set_target_is_srgb(srgb: bool) {
    TARGET_IS_SRGB.store(srgb, Ordering::Relaxed);
}

fn decode(channel: f32) -> f32 {
    channel.clamp(0.0, 1.0).powf(2.2)
}

fn encode(channel: f32) -> f32 {
    let value = channel.clamp(0.0, 1.0);
    if TARGET_IS_SRGB.load(Ordering::Relaxed) {
        value
    } else {
        value.powf(1.0 / 2.2)
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-8);
    [v[0] / length, v[1] / length, v[2] / length]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Direction toward the key light (world coordinates, Z up).
pub fn key_light() -> [f32; 3] {
    normalize([0.42, -0.5, 0.76])
}

/// How a surface reflects light.
#[derive(Debug, Clone, Copy)]
pub struct Surface {
    /// Strength of the specular highlight (0 = matte).
    pub specular: f32,
    /// Highlight tightness.
    pub shininess: f32,
}

impl Surface {
    pub const MATTE: Self = Self {
        specular: 0.05,
        shininess: 16.0,
    };
    pub const WOOD: Self = Self {
        specular: 0.12,
        shininess: 28.0,
    };
    pub const METAL: Self = Self {
        specular: 0.55,
        shininess: 54.0,
    };
}

/// Final display colour of a surface point.
///
/// * `base` — surface colour (sRGB)
/// * `normal` — surface normal (either side; it is turned toward the viewer)
/// * `view` — unit vector from the surface toward the viewer
/// * `ambient_occlusion` — 1 = open, 0 = fully enclosed
/// * `shadow` — 1 = lit by the key light, 0 = in its shadow
pub fn shade(
    base: [f32; 3],
    normal: [f32; 3],
    view: [f32; 3],
    surface: Surface,
    ambient_occlusion: f32,
    shadow: f32,
) -> [f32; 3] {
    let mut n = normalize(normal);
    if dot(n, view) < 0.0 {
        n = [-n[0], -n[1], -n[2]];
    }
    let base = base.map(decode);
    let key = key_light();
    let fill = normalize([-0.62, -0.28, 0.5]);

    // Sky above, warm ground bounce below.
    let up = 0.5 + 0.5 * n[2];
    let sky = [0.70, 0.78, 0.92];
    let ground = [0.34, 0.30, 0.26];
    let ambient: [f32; 3] =
        std::array::from_fn(|i| (ground[i] * (1.0 - up) + sky[i] * up) * 0.52 * ambient_occlusion);

    // Wrapped diffuse keeps the shaded side readable instead of black.
    let key_amount = ((dot(n, key) + 0.18) / 1.18).max(0.0) * 1.05 * shadow;
    let key_color = [1.0, 0.95, 0.86];
    let fill_amount = dot(n, fill).max(0.0) * 0.30 * (0.35 + 0.65 * ambient_occlusion);
    let fill_color = [0.82, 0.90, 1.0];
    let rim = (1.0 - dot(n, view).max(0.0)).powi(3) * 0.10 * ambient_occlusion;

    let half = normalize([key[0] + view[0], key[1] + view[1], key[2] + view[2]]);
    let specular = dot(n, half).max(0.0).powf(surface.shininess) * surface.specular * shadow;

    std::array::from_fn(|i| {
        let lit = ambient[i] + key_amount * key_color[i] + fill_amount * fill_color[i] + rim;
        // Gentle highlight roll-off instead of a hard clip.
        let color = base[i] * lit + specular;
        encode(color / (1.0 + 0.12 * color))
    })
}

/// A top-down height-field used to bake occlusion and shadows into colours.
pub struct HeightField {
    origin: [f32; 2],
    cell: [f32; 2],
    cols: usize,
    rows: usize,
    heights: Vec<f32>,
}

const NO_SURFACE: f32 = f32::NEG_INFINITY;

impl HeightField {
    /// From a regular grid of node heights (`cols × rows`, node `(c, r)` at
    /// `origin + (c, r) × cell`).
    pub fn from_grid(
        origin: [f32; 2],
        cell: [f32; 2],
        cols: usize,
        rows: usize,
        heights: Vec<f32>,
    ) -> Self {
        debug_assert_eq!(heights.len(), cols * rows);
        Self {
            origin,
            cell,
            cols,
            rows,
            heights,
        }
    }

    /// Highest surface over each cell of a triangle mesh seen from above.
    pub fn from_triangles(
        triangles: impl Iterator<Item = [[f32; 3]; 3]>,
        bounds: ([f32; 2], [f32; 2]),
        max_side: usize,
    ) -> Self {
        let (low, high) = bounds;
        let extent = (high[0] - low[0]).max(high[1] - low[1]).max(1e-3);
        let cell = (extent / max_side as f32).max(0.05);
        let cols = ((high[0] - low[0]) / cell).ceil() as usize + 2;
        let rows = ((high[1] - low[1]) / cell).ceil() as usize + 2;
        let origin = [low[0] - cell, low[1] - cell];
        let mut field = Self {
            origin,
            cell: [cell, cell],
            cols,
            rows,
            heights: vec![NO_SURFACE; cols * rows],
        };
        for triangle in triangles {
            field.raster_triangle(triangle);
        }
        field
    }

    fn raster_triangle(&mut self, t: [[f32; 3]; 3]) {
        let min_x = t.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
        let max_x = t.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max);
        let min_y = t.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
        let max_y = t.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max);
        let c0 = (((min_x - self.origin[0]) / self.cell[0]).floor() as isize).max(0);
        let c1 =
            (((max_x - self.origin[0]) / self.cell[0]).ceil() as isize).min(self.cols as isize - 1);
        let r0 = (((min_y - self.origin[1]) / self.cell[1]).floor() as isize).max(0);
        let r1 =
            (((max_y - self.origin[1]) / self.cell[1]).ceil() as isize).min(self.rows as isize - 1);
        let denominator =
            (t[1][1] - t[2][1]) * (t[0][0] - t[2][0]) + (t[2][0] - t[1][0]) * (t[0][1] - t[2][1]);
        if denominator.abs() < 1e-12 {
            // Vertical triangle: it still occludes, so keep its highest point.
            for p in t {
                let c = ((p[0] - self.origin[0]) / self.cell[0]).round() as isize;
                let r = ((p[1] - self.origin[1]) / self.cell[1]).round() as isize;
                if c >= 0 && r >= 0 && (c as usize) < self.cols && (r as usize) < self.rows {
                    let index = r as usize * self.cols + c as usize;
                    self.heights[index] = self.heights[index].max(p[2]);
                }
            }
            return;
        }
        let slack = -0.02;
        for row in r0..=r1 {
            let y = self.origin[1] + row as f32 * self.cell[1];
            for col in c0..=c1 {
                let x = self.origin[0] + col as f32 * self.cell[0];
                let a = ((t[1][1] - t[2][1]) * (x - t[2][0]) + (t[2][0] - t[1][0]) * (y - t[2][1]))
                    / denominator;
                let b = ((t[2][1] - t[0][1]) * (x - t[2][0]) + (t[0][0] - t[2][0]) * (y - t[2][1]))
                    / denominator;
                let c = 1.0 - a - b;
                if a >= slack && b >= slack && c >= slack {
                    let z = a * t[0][2] + b * t[1][2] + c * t[2][2];
                    let index = row as usize * self.cols + col as usize;
                    if z > self.heights[index] {
                        self.heights[index] = z;
                    }
                }
            }
        }
    }

    /// Height at a point (bilinear); `NEG_INFINITY` where there is no surface.
    pub fn height(&self, x: f32, y: f32) -> f32 {
        let fx = (x - self.origin[0]) / self.cell[0];
        let fy = (y - self.origin[1]) / self.cell[1];
        if fx < 0.0 || fy < 0.0 || fx >= (self.cols - 1) as f32 || fy >= (self.rows - 1) as f32 {
            return NO_SURFACE;
        }
        let (c, r) = (fx as usize, fy as usize);
        let (tx, ty) = (fx - c as f32, fy - r as f32);
        let at = |c: usize, r: usize| self.heights[r * self.cols + c];
        let corners = [at(c, r), at(c + 1, r), at(c, r + 1), at(c + 1, r + 1)];
        if corners.iter().any(|h| *h == NO_SURFACE) {
            // Avoid smearing a cliff edge into the void.
            return corners.iter().cloned().fold(NO_SURFACE, f32::max);
        }
        let bottom = corners[0] * (1.0 - tx) + corners[1] * tx;
        let top = corners[2] * (1.0 - tx) + corners[3] * tx;
        bottom * (1.0 - ty) + top * ty
    }

    /// Ambient occlusion at a point: how much of the sky it can see, from the
    /// horizon angle in eight directions within `radius`.
    pub fn ambient_occlusion(&self, p: [f32; 3], radius: f32) -> f32 {
        const DIRECTIONS: usize = 8;
        const STEPS: usize = 5;
        let mut occlusion = 0.0;
        for direction in 0..DIRECTIONS {
            let angle = direction as f32 * std::f32::consts::TAU / DIRECTIONS as f32;
            let (dy, dx) = angle.sin_cos();
            let mut horizon: f32 = 0.0;
            for step in 1..=STEPS {
                let distance = radius * step as f32 / STEPS as f32;
                let h = self.height(p[0] + dx * distance, p[1] + dy * distance);
                if h.is_finite() && h > p[2] + 0.01 {
                    horizon = horizon.max(((h - p[2]) / distance).atan());
                }
            }
            occlusion += horizon.sin();
        }
        (1.0 - 0.85 * occlusion / DIRECTIONS as f32).clamp(0.15, 1.0)
    }

    /// Soft shadow of the key light at a point: 1 = lit, 0 = shadowed.
    pub fn shadow(&self, p: [f32; 3], light: [f32; 3], reach: f32) -> f32 {
        let horizontal = light[0].hypot(light[1]);
        if horizontal < 1e-4 {
            return 1.0;
        }
        let (dx, dy) = (light[0] / horizontal, light[1] / horizontal);
        let rise = light[2] / horizontal; // height gained per unit of run
        let steps = 28;
        let mut lit: f32 = 1.0;
        for step in 1..=steps {
            let distance = reach * (step as f32 / steps as f32).powf(1.5);
            let ray = p[2] + 0.02 + rise * distance;
            let h = self.height(p[0] + dx * distance, p[1] + dy * distance);
            if h.is_finite() && h > ray {
                // Penumbra: shallow blockers close to the point shade softly.
                lit = lit.min((1.0 - ((h - ray) / (0.35 * distance + 0.4)).min(1.0)).max(0.0));
            }
        }
        0.12 + 0.88 * lit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shading_is_brighter_facing_the_light_and_never_black() {
        set_target_is_srgb(false);
        let base = [0.6, 0.4, 0.2];
        let view = normalize([0.3, 0.4, 0.86]);
        let toward = shade(base, key_light(), view, Surface::MATTE, 1.0, 1.0);
        let away = shade(base, [-0.6, 0.5, 0.2], view, Surface::MATTE, 1.0, 1.0);
        let sum = |c: [f32; 3]| c[0] + c[1] + c[2];
        assert!(sum(toward) > sum(away));
        assert!(
            away.iter().all(|c| *c > 0.12),
            "shadowed side too dark: {away:?}"
        );
        assert!(toward.iter().all(|c| *c <= 1.0));
    }

    #[test]
    fn shadow_and_occlusion_darken_the_result() {
        set_target_is_srgb(false);
        let base = [0.6, 0.4, 0.2];
        let view = [0.0, 0.0, 1.0];
        let open = shade(base, [0.0, 0.0, 1.0], view, Surface::WOOD, 1.0, 1.0);
        let shaded = shade(base, [0.0, 0.0, 1.0], view, Surface::WOOD, 0.4, 0.15);
        assert!(shaded[0] < open[0] && shaded[1] < open[1]);
    }

    #[test]
    fn materials_are_lifted_by_the_gamma_encode_on_non_srgb_targets() {
        let base = [0.5, 0.5, 0.5];
        let view = [0.0, 0.0, 1.0];
        set_target_is_srgb(true);
        let linear = shade(base, [0.0, 0.0, 1.0], view, Surface::MATTE, 1.0, 1.0);
        set_target_is_srgb(false);
        let encoded = shade(base, [0.0, 0.0, 1.0], view, Surface::MATTE, 1.0, 1.0);
        assert!(encoded[0] > linear[0]);
    }

    fn wall_field() -> HeightField {
        // A 10 x 10 flat floor at z = 0 with a 4 mm high wall at x = 5.
        let mut heights = vec![0.0; 21 * 21];
        for row in 0..21 {
            for col in 10..12 {
                heights[row * 21 + col] = 4.0;
            }
        }
        HeightField::from_grid([0.0, 0.0], [0.5, 0.5], 21, 21, heights)
    }

    #[test]
    fn occlusion_is_lower_beside_a_wall_than_in_the_open() {
        let field = wall_field();
        let beside = field.ambient_occlusion([4.0, 5.0, 0.0], 3.0);
        let open = field.ambient_occlusion([1.0, 5.0, 0.0], 0.8);
        assert!(beside < open, "{beside} vs {open}");
        assert!(open > 0.95);
    }

    #[test]
    fn a_wall_casts_a_shadow_away_from_the_light() {
        let field = wall_field();
        let light = normalize([1.0, 0.0, 0.5]); // light from +X, 27 degrees up
                                                // Behind the wall (x < 5) is shadowed; in front of it (x > 6) is lit.
        let behind = field.shadow([3.0, 5.0, 0.0], light, 8.0);
        let front = field.shadow([8.0, 5.0, 0.0], light, 8.0);
        assert!(behind < 0.4, "behind {behind}");
        assert!(front > 0.9, "front {front}");
    }

    #[test]
    fn triangles_are_rasterised_into_a_top_down_height_field() {
        let triangles = vec![
            [[0.0, 0.0, 2.0], [4.0, 0.0, 2.0], [4.0, 4.0, 2.0]],
            [[0.0, 0.0, 2.0], [4.0, 4.0, 2.0], [0.0, 4.0, 2.0]],
        ];
        let field =
            HeightField::from_triangles(triangles.into_iter(), ([0.0, 0.0], [4.0, 4.0]), 32);
        assert!((field.height(2.0, 2.0) - 2.0).abs() < 1e-3);
        assert!(!field.height(9.0, 9.0).is_finite());
    }
}
