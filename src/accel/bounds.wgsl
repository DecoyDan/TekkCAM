// Conservative per-cell height bounds of a triangle mesh, seen from above.
//
// For every grid cell two values are kept:
//   upper: an upper bound of the highest surface anywhere inside the cell
//   lower: a lower bound of the highest surface anywhere inside the cell
//          (only set when a triangle completely covers the cell)
// A probe that is below `lower` is certainly blocked, one that is above `upper`
// certainly free; everything else is left to the exact CPU test. Heights are
// stored as order-preserving u32 keys so atomicMax works for negative values.

struct Grid {
    origin: vec2<f32>,
    cell: f32,
    margin: f32,
    columns: u32,
    rows: u32,
    count: u32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> grid: Grid;
@group(0) @binding(1) var<storage, read> vertices: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read> triangles: array<vec4<u32>>;
@group(0) @binding(3) var<storage, read_write> upper: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> lower: array<atomic<u32>>;

fn encode(v: f32) -> u32 {
    let b = bitcast<u32>(v);
    if ((b & 0x80000000u) == 0u) {
        return b | 0x80000000u;
    }
    return ~b;
}

// Signed distance from p to the line a->b (positive on the left).
fn edge_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let e = b - a;
    let len = max(length(e), 1e-12);
    return (e.x * (p.y - a.y) - e.y * (p.x - a.x)) / len;
}

// True when the axis-aligned rectangle is entirely outside the triangle's
// half-plane for edge a->b (orient = +1 for counter-clockwise triangles).
fn rect_outside_edge(
    a: vec2<f32>, b: vec2<f32>, orient: f32,
    lo: vec2<f32>, hi: vec2<f32>, tolerance: f32,
) -> bool {
    let e = b - a;
    // Outward normal.
    let n = vec2<f32>(e.y, -e.x) * orient;
    let nearest = vec2<f32>(select(hi.x, lo.x, n.x > 0.0), select(hi.y, lo.y, n.y > 0.0));
    let len = max(length(n), 1e-12);
    return dot(n, nearest - a) / len > tolerance;
}

fn inside_shrunk(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, c: vec2<f32>, orient: f32, eps: f32) -> bool {
    return edge_distance(p, a, b) * orient >= eps
        && edge_distance(p, b, c) * orient >= eps
        && edge_distance(p, c, a) * orient >= eps;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= grid.count) {
        return;
    }
    let t = triangles[gid.x];
    let a = vertices[t.x].xyz;
    let b = vertices[t.y].xyz;
    let c = vertices[t.z].xyz;
    let n = cross(b - a, c - a);
    // Vertical triangles are never seen from above (the CPU query skips them too).
    if (abs(n.z) <= 1e-12) {
        return;
    }
    let nlen = length(n);
    // Evaluating the plane is only well conditioned on gentle slopes.
    let well_conditioned = abs(n.z) >= 0.05 * nlen;
    let zmax = max(a.z, max(b.z, c.z));
    let orient = select(-1.0, 1.0, n.z > 0.0);
    let pad = grid.margin;

    let lo_xy = min(a.xy, min(b.xy, c.xy)) - vec2<f32>(pad);
    let hi_xy = max(a.xy, max(b.xy, c.xy)) + vec2<f32>(pad);
    let x0 = u32(clamp(floor((lo_xy.x - grid.origin.x) / grid.cell), 0.0, f32(grid.columns - 1u)));
    let x1 = u32(clamp(floor((hi_xy.x - grid.origin.x) / grid.cell), 0.0, f32(grid.columns - 1u)));
    let y0 = u32(clamp(floor((lo_xy.y - grid.origin.y) / grid.cell), 0.0, f32(grid.rows - 1u)));
    let y1 = u32(clamp(floor((hi_xy.y - grid.origin.y) / grid.cell), 0.0, f32(grid.rows - 1u)));

    for (var iy = y0; iy <= y1; iy = iy + 1u) {
        for (var ix = x0; ix <= x1; ix = ix + 1u) {
            let cell_lo = vec2<f32>(
                grid.origin.x + f32(ix) * grid.cell,
                grid.origin.y + f32(iy) * grid.cell,
            );
            let cell_hi = cell_lo + vec2<f32>(grid.cell);
            let rect_lo = cell_lo - vec2<f32>(pad);
            let rect_hi = cell_hi + vec2<f32>(pad);
            if (rect_outside_edge(a.xy, b.xy, orient, rect_lo, rect_hi, 0.0)
                || rect_outside_edge(b.xy, c.xy, orient, rect_lo, rect_hi, 0.0)
                || rect_outside_edge(c.xy, a.xy, orient, rect_lo, rect_hi, 0.0)) {
                continue;
            }
            let index = iy * grid.columns + ix;

            var high = zmax;
            if (well_conditioned) {
                let z00 = a.z - (n.x * (rect_lo.x - a.x) + n.y * (rect_lo.y - a.y)) / n.z;
                let z10 = a.z - (n.x * (rect_hi.x - a.x) + n.y * (rect_lo.y - a.y)) / n.z;
                let z01 = a.z - (n.x * (rect_lo.x - a.x) + n.y * (rect_hi.y - a.y)) / n.z;
                let z11 = a.z - (n.x * (rect_hi.x - a.x) + n.y * (rect_hi.y - a.y)) / n.z;
                high = min(zmax, max(max(z00, z10), max(z01, z11)));
            }
            atomicMax(&upper[index], encode(high + pad + abs(high) * 1e-5));

            if (well_conditioned) {
                // The triangle certainly covers the whole cell only if all four
                // corners are inside it (with a little room to spare).
                let eps = pad;
                let p00 = cell_lo;
                let p10 = vec2<f32>(cell_hi.x, cell_lo.y);
                let p01 = vec2<f32>(cell_lo.x, cell_hi.y);
                let p11 = cell_hi;
                if (inside_shrunk(p00, a.xy, b.xy, c.xy, orient, eps)
                    && inside_shrunk(p10, a.xy, b.xy, c.xy, orient, eps)
                    && inside_shrunk(p01, a.xy, b.xy, c.xy, orient, eps)
                    && inside_shrunk(p11, a.xy, b.xy, c.xy, orient, eps)) {
                    let z00 = a.z - (n.x * (p00.x - a.x) + n.y * (p00.y - a.y)) / n.z;
                    let z10 = a.z - (n.x * (p10.x - a.x) + n.y * (p10.y - a.y)) / n.z;
                    let z01 = a.z - (n.x * (p01.x - a.x) + n.y * (p01.y - a.y)) / n.z;
                    let z11 = a.z - (n.x * (p11.x - a.x) + n.y * (p11.y - a.y)) / n.z;
                    let low = min(min(z00, z10), min(z01, z11));
                    atomicMax(&lower[index], encode(low - pad - abs(low) * 1e-5));
                }
            }
        }
    }
}
