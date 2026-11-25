// Stock removal: sweep a cutter along line segments and lower a height-field.
//
// One workgroup per segment; its 64 threads share the grid cells under the
// segment's bounding box. Heights are order-preserving u32 keys, so atomicMin
// keeps the lowest cut and the result does not depend on the order of the
// segments.

struct CutGrid {
    origin: vec2<f32>,
    cell: vec2<f32>,
    radius: f32,
    floor_z: f32,
    columns: u32,
    rows: u32,
    count: u32,
    // Corner radius of the cutter: 0 = flat end mill, radius = ball nose,
    // anything in between = bull nose.
    corner: f32,
};

@group(0) @binding(0) var<uniform> g: CutGrid;
// Two vec4 per segment: start (xyz), end (xyz).
@group(0) @binding(1) var<storage, read> cuts: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> heights: array<atomic<u32>>;

fn encode(v: f32) -> u32 {
    let b = bitcast<u32>(v);
    if ((b & 0x80000000u) == 0u) {
        return b | 0x80000000u;
    }
    return ~b;
}

@compute @workgroup_size(64)
fn main(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(num_workgroups) groups: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let segment = wg.x + wg.y * groups.x;
    if (segment >= g.count) {
        return;
    }
    let start = cuts[segment * 2u].xyz;
    let end = cuts[segment * 2u + 1u].xyz;
    let r = g.radius;

    let lo = min(start.xy, end.xy) - vec2<f32>(r);
    let hi = max(start.xy, end.xy) + vec2<f32>(r);
    let min_x = i32(clamp(floor((lo.x - g.origin.x) / g.cell.x), 0.0, f32(g.columns - 1u)));
    let max_x = i32(clamp(ceil((hi.x - g.origin.x) / g.cell.x), 0.0, f32(g.columns - 1u)));
    let min_y = i32(clamp(floor((lo.y - g.origin.y) / g.cell.y), 0.0, f32(g.rows - 1u)));
    let max_y = i32(clamp(ceil((hi.y - g.origin.y) / g.cell.y), 0.0, f32(g.rows - 1u)));
    let width = u32(max_x - min_x + 1);
    let total = width * u32(max_y - min_y + 1);

    let ab = end.xy - start.xy;
    let length_sq = dot(ab, ab);
    let radius_sq = r * r;

    for (var i = lane; i < total; i = i + 64u) {
        let ix = min_x + i32(i % width);
        let iy = min_y + i32(i / width);
        let p = vec2<f32>(
            g.origin.x + f32(ix) * g.cell.x,
            g.origin.y + f32(iy) * g.cell.y,
        );
        // A purely vertical move (a plunge) cuts down to its lowest point.
        var t = 0.0;
        var z = min(start.z, end.z);
        if (length_sq > 1e-12) {
            t = clamp(dot(p - start.xy, ab) / length_sq, 0.0, 1.0);
            z = start.z + (end.z - start.z) * t;
        }
        let nearest = start.xy + ab * t;
        let d = p - nearest;
        let dist_sq = dot(d, d);
        if (dist_sq <= radius_sq) {
            let d = sqrt(dist_sq);
            let flat_radius = r - g.corner;
            if (g.corner > 0.0 && d > flat_radius) {
                let e = d - flat_radius;
                z = z + g.corner - sqrt(max(g.corner * g.corner - e * e, 0.0));
            }
            z = max(z, g.floor_z);
            atomicMin(&heights[u32(iy) * g.columns + u32(ix)], encode(z));
        }
    }
}
