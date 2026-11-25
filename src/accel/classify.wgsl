// Classify cutter/holder probes against the conservative height bounds.
//
// A probe asks "is the surface under every sample point at most `limit`?".
// Result per probe: 0 = certainly yes, 1 = certainly no, 2 = undecided (the CPU
// answers those with the exact mesh query).

struct Grid {
    origin: vec2<f32>,
    cell: f32,
    margin: f32,
    columns: u32,
    rows: u32,
    count: u32,
    _pad: u32,
};

struct Probe {
    x: f32,
    y: f32,
    limit: f32,
    radius: f32,
    ring: u32,
    center: u32,
    _p0: u32,
    _p1: u32,
};

@group(0) @binding(0) var<uniform> grid: Grid;
@group(0) @binding(1) var<storage, read> probes: array<Probe>;
@group(0) @binding(2) var<storage, read> upper: array<u32>;
@group(0) @binding(3) var<storage, read> lower: array<u32>;
@group(0) @binding(4) var<storage, read_write> result: array<u32>;

fn decode(k: u32) -> f32 {
    if ((k & 0x80000000u) != 0u) {
        return bitcast<f32>(k & 0x7fffffffu);
    }
    return bitcast<f32>(~k);
}

fn classify_sample(x: f32, y: f32, limit: f32) -> u32 {
    let fx = (x - grid.origin.x) / grid.cell;
    let fy = (y - grid.origin.y) / grid.cell;
    if (fx < 0.0 || fy < 0.0 || fx >= f32(grid.columns) || fy >= f32(grid.rows)) {
        return 0u; // outside the mesh footprint: no surface
    }
    let index = u32(fy) * grid.columns + u32(fx);
    let ku = upper[index];
    if (ku == 0u) {
        return 0u; // nothing above this cell
    }
    if (limit >= decode(ku)) {
        return 0u;
    }
    let kl = lower[index];
    if (kl != 0u && limit < decode(kl)) {
        return 1u;
    }
    return 2u;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= grid.count) {
        return;
    }
    let p = probes[gid.x];
    var undecided = false;
    if (p.center != 0u) {
        let c = classify_sample(p.x, p.y, p.limit);
        if (c == 1u) {
            result[gid.x] = 1u;
            return;
        }
        undecided = undecided || (c == 2u);
    }
    let ring = min(p.ring, 64u);
    for (var k = 0u; k < ring; k = k + 1u) {
        let angle = f32(k) * 6.2831853 / f32(ring);
        let c = classify_sample(
            p.x + p.radius * cos(angle),
            p.y + p.radius * sin(angle),
            p.limit,
        );
        if (c == 1u) {
            result[gid.x] = 1u;
            return;
        }
        undecided = undecided || (c == 2u);
    }
    result[gid.x] = select(0u, 2u, undecided);
}
