use super::*;
use crate::workspace::model::{MeshBody, Model, StepLengthUnit};

/// A rolling terrain with a few vertical cliffs, `n × n` cells of 0.5 mm.
fn terrain(n: usize) -> Model {
    let mut vertices = Vec::new();
    for iy in 0..=n {
        for ix in 0..=n {
            let (x, y) = (ix as f32 * 0.5, iy as f32 * 0.5);
            let mut z = 3.0 + (x * 0.35).sin() * (y * 0.25).cos() * 2.0;
            // A plateau with steep edges.
            if (8.0..14.0).contains(&x) && (6.0..12.0).contains(&y) {
                z += 4.0;
            }
            vertices.push([x, y, z]);
        }
    }
    let mut triangles = Vec::new();
    let stride = n as u32 + 1;
    for iy in 0..n as u32 {
        for ix in 0..n as u32 {
            let a = iy * stride + ix;
            triangles.push([a, a + 1, a + stride + 1]);
            triangles.push([a, a + stride + 1, a + stride]);
        }
    }
    let count = triangles.len();
    let extent = n as f32 * 0.5;
    Model {
        name: "terrain".into(),
        source_path: "fixture:terrain".into(),
        visible: true,
        source_schema: vec![],
        source_unit: StepLengthUnit::Millimeter,
        units_are_millimeters: true,
        transform: Default::default(),
        vertices,
        triangles,
        bodies: vec![MeshBody {
            name: "terrain".into(),
            source_item_id: 1,
            first_triangle: 0,
            triangle_count: count,
            triangle_face_ids: vec![],
            transform: [[1., 0., 0., 0.], [0., 1., 0., 0.], [0., 0., 1., 0.]],
            bounds: [[0., 0., 0.], [extent, extent, 10.0]],
        }],
        bounds: [[0., 0., 0.], [extent, extent, 10.0]],
        import_notes: vec![],
    }
}

/// Deterministic pseudo-random numbers in `[0, 1)`.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f64) / (1u64 << 31) as f64
    }
}

fn random_probes(count: usize, extent: f64) -> Vec<Probe> {
    let mut random = Lcg(7);
    (0..count)
        .map(|index| {
            let x = random.next() * (extent + 6.0) - 3.0;
            let y = random.next() * (extent + 6.0) - 3.0;
            let limit = random.next() * 12.0 - 1.0;
            let radius = random.next() * 3.0;
            match index % 3 {
                0 => Probe::cutter(x, y, limit, radius, 0.0),
                1 => Probe::ring(x, y, limit, radius, 16),
                _ => Probe::point(x, y, limit),
            }
        })
        .collect()
}

#[test]
fn engine_without_a_gpu_matches_the_exact_query() {
    let model = terrain(60);
    let query = MeshQuery::new(&model);
    let engine = ProbeEngine::with_gpu(&query, None);
    assert!(!engine.uses_gpu());
    let probes = random_probes(6000, 30.0);
    let answers = engine.fits(&probes);
    for (probe, answer) in probes.iter().zip(&answers) {
        assert_eq!(*answer, exact_fits(&query, probe));
    }
    assert_eq!(engine.last_stats().checked_exactly, probes.len());
}

#[test]
fn cutter_probe_is_the_same_test_as_cutter_accessible() {
    let model = terrain(30);
    let query = MeshQuery::new(&model);
    let mut random = Lcg(3);
    for _ in 0..300 {
        let (x, y, z) = (
            random.next() * 20.0 - 2.0,
            random.next() * 20.0 - 2.0,
            random.next() * 10.0,
        );
        let radius = random.next() * 2.0;
        assert_eq!(
            exact_fits(&query, &Probe::cutter(x, y, z, radius, 0.05)),
            query.cutter_accessible(x, y, z, radius, 0.05),
        );
    }
}

#[test]
fn parallel_exact_answers_match_the_serial_ones() {
    let model = terrain(40);
    let query = MeshQuery::new(&model);
    let probes = random_probes(9000, 20.0);
    let parallel = exact_fits_parallel(&query, &probes);
    for (probe, answer) in probes.iter().zip(&parallel) {
        assert_eq!(*answer, exact_fits(&query, probe));
    }
}

/// One GPU context shared by every test. Opening several devices at the same
/// moment from parallel tests can wedge some drivers, and the application
/// itself only ever has one device, so the tests use one too.
fn gpu_or_skip() -> Option<Arc<GpuContext>> {
    static SHARED: std::sync::OnceLock<Option<Arc<GpuContext>>> = std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| match GpuContext::headless() {
            Ok(gpu) => {
                eprintln!("testing on {}", gpu.adapter_name());
                Some(gpu)
            }
            Err(error) => {
                eprintln!("skipping GPU tests: {error}");
                None
            }
        })
        .clone()
}

#[test]
fn gpu_probe_answers_are_identical_to_the_cpu_and_mostly_decided_on_the_gpu() {
    let Some(gpu) = gpu_or_skip() else { return };
    let model = terrain(80);
    let query = MeshQuery::new(&model);
    let engine = ProbeEngine::with_gpu(&query, Some(gpu));
    assert!(engine.uses_gpu());
    let probes = random_probes(40_000, 40.0);
    let answers = engine.fits(&probes);
    let mut wrong = 0;
    for (probe, answer) in probes.iter().zip(&answers) {
        if *answer != exact_fits(&query, probe) {
            wrong += 1;
        }
    }
    assert_eq!(wrong, 0, "GPU-assisted answers differ from the exact query");
    let stats = engine.last_stats();
    assert_eq!(stats.total, probes.len());
    assert!(
        stats.decided_on_gpu * 2 > stats.total,
        "only {} of {} probes were decided on the GPU",
        stats.decided_on_gpu,
        stats.total
    );
}

#[test]
fn gpu_handles_a_transformed_model_and_negative_heights() {
    let Some(gpu) = gpu_or_skip() else { return };
    let mut model = terrain(60);
    model.transform.rotation_degrees = [0.0, 0.0, 90.0];
    model.transform.translation_mm = [40.0, -5.0, -7.0];
    let query = MeshQuery::new(&model);
    let engine = ProbeEngine::with_gpu(&query, Some(gpu));
    let mut random = Lcg(11);
    let probes: Vec<Probe> = (0..20_000)
        .map(|_| {
            Probe::cutter(
                random.next() * 50.0 + 20.0,
                random.next() * 50.0 - 10.0,
                random.next() * 14.0 - 9.0,
                random.next() * 2.0,
                0.0,
            )
        })
        .collect();
    let answers = engine.fits(&probes);
    for (probe, answer) in probes.iter().zip(&answers) {
        assert_eq!(*answer, exact_fits(&query, probe));
    }
}

fn grid() -> GridSpec {
    GridSpec {
        origin: [-2.0, 1.0],
        cell: [0.25, 0.4],
        columns: 160,
        rows: 90,
        floor: -5.0,
    }
}

fn random_cuts(count: usize) -> Vec<Cut> {
    let mut random = Lcg(5);
    (0..count)
        .map(|_| {
            let a = [
                random.next() as f32 * 36.0 - 2.0,
                random.next() as f32 * 36.0 + 1.0,
                random.next() as f32 * 8.0 - 6.0,
            ];
            let b = [
                a[0] + (random.next() as f32 - 0.5) * 8.0,
                a[1] + (random.next() as f32 - 0.5) * 8.0,
                a[2] + (random.next() as f32 - 0.5) * 3.0,
            ];
            [a, b]
        })
        .collect()
}

#[test]
fn cpu_cutting_lowers_only_nodes_inside_the_cutter_and_respects_the_floor() {
    let spec = grid();
    let mut heights = vec![10.0_f32; spec.columns * spec.rows];
    let tool = CutTool {
        radius: 2.0,
        corner: 0.0,
    };
    cut_capsules_cpu(
        &spec,
        &mut heights,
        &[[[10.0, 10.0, 4.0], [10.0, 10.0, 4.0]]],
        tool,
    );
    let node = |x: f64, y: f64| {
        let column = ((x - spec.origin[0]) / spec.cell[0]).round() as usize;
        let row = ((y - spec.origin[1]) / spec.cell[1]).round() as usize;
        heights[row * spec.columns + column]
    };
    assert_eq!(node(10.0, 10.0), 4.0);
    assert_eq!(node(11.5, 10.0), 4.0);
    assert_eq!(node(13.0, 10.0), 10.0);
    // Cuts never go below the floor.
    let mut deep = vec![10.0_f32; spec.columns * spec.rows];
    cut_capsules_cpu(
        &spec,
        &mut deep,
        &[[[10.0, 10.0, -20.0], [12.0, 10.0, -20.0]]],
        tool,
    );
    assert!(deep.iter().all(|height| *height >= -5.0));
    assert_eq!(deep.iter().cloned().fold(f32::INFINITY, f32::min), -5.0);
}

#[test]
fn ball_nose_cuts_leave_a_curved_floor_and_cut_order_does_not_matter() {
    let spec = grid();
    let tool = CutTool::ball(2.0);
    let cuts = random_cuts(50);
    let mut forward = vec![10.0_f32; spec.columns * spec.rows];
    cut_capsules_cpu(&spec, &mut forward, &cuts, tool);
    let mut reversed_cuts = cuts.clone();
    reversed_cuts.reverse();
    let mut backward = vec![10.0_f32; spec.columns * spec.rows];
    cut_capsules_cpu(&spec, &mut backward, &reversed_cuts, tool);
    assert_eq!(forward, backward);
    // Away from the axis the ball has cut less deeply.
    let mut single = vec![10.0_f32; spec.columns * spec.rows];
    // y = 9.8 is exactly on a grid row (origin 1.0 + 22 × 0.4).
    cut_capsules_cpu(
        &spec,
        &mut single,
        &[[[10.0, 9.8, 4.0], [10.0, 9.8, 4.0]]],
        tool,
    );
    let at = |x: f64, y: f64| {
        let column = ((x - spec.origin[0]) / spec.cell[0]).round() as usize;
        let row = ((y - spec.origin[1]) / spec.cell[1]).round() as usize;
        single[row * spec.columns + column]
    };
    assert!((at(10.0, 9.8) - 4.0).abs() < 1e-6);
    assert!(at(11.5, 9.8) > 4.5 && at(11.5, 9.8) < 6.5);
}

#[test]
fn gpu_cutting_matches_the_cpu_reference() {
    let Some(gpu) = gpu_or_skip() else { return };
    let spec = grid();
    for ball in [false, true] {
        let tool = if ball {
            CutTool::ball(1.75)
        } else {
            CutTool::flat(1.75)
        };
        let cuts = random_cuts(400);
        let mut expected = vec![10.0_f32; spec.columns * spec.rows];
        cut_capsules_cpu(&spec, &mut expected, &cuts, tool);
        let mut actual = vec![10.0_f32; spec.columns * spec.rows];
        gpu.cut_capsules(&spec, &mut actual, &cuts, tool).unwrap();
        let worst = expected
            .iter()
            .zip(&actual)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            worst < 2e-3,
            "ball={ball}: largest height difference {worst}"
        );
    }
}

#[test]
fn gpu_cutting_handles_many_segments_in_several_dispatches() {
    let Some(gpu) = gpu_or_skip() else { return };
    let spec = grid();
    let tool = CutTool {
        radius: 0.6,
        corner: 0.0,
    };
    let cuts = random_cuts(70_000);
    let mut expected = vec![10.0_f32; spec.columns * spec.rows];
    cut_capsules_cpu(&spec, &mut expected, &cuts, tool);
    let mut actual = vec![10.0_f32; spec.columns * spec.rows];
    gpu.cut_capsules(&spec, &mut actual, &cuts, tool).unwrap();
    let worst = expected
        .iter()
        .zip(&actual)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0_f32, f32::max);
    assert!(worst < 2e-3, "largest height difference {worst}");
}

#[test]
fn a_vertical_plunge_cuts_down_to_its_lowest_point_on_cpu_and_gpu() {
    let spec = grid();
    // 1.01 keeps every grid node clear of the exact edge of the cutter, where
    // the CPU's f64 and the GPU's f32 may legitimately disagree.
    let tool = CutTool {
        radius: 1.01,
        corner: 0.0,
    };
    let cuts = [[[10.0, 9.8, 12.0], [10.0, 9.8, 3.0]]];
    let mut cpu = vec![10.0_f32; spec.columns * spec.rows];
    cut_capsules_cpu(&spec, &mut cpu, &cuts, tool);
    let column = ((10.0 - spec.origin[0]) / spec.cell[0]).round() as usize;
    let row = ((9.8 - spec.origin[1]) / spec.cell[1]).round() as usize;
    assert_eq!(cpu[row * spec.columns + column], 3.0);
    if let Some(gpu) = gpu_or_skip() {
        let mut on_gpu = vec![10.0_f32; spec.columns * spec.rows];
        gpu.cut_capsules(&spec, &mut on_gpu, &cuts, tool).unwrap();
        let differing: Vec<(usize, f32, f32)> = cpu
            .iter()
            .zip(&on_gpu)
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, (a, b))| (i, *a, *b))
            .collect();
        assert!(
            differing.is_empty(),
            "{} nodes differ, e.g. {:?}",
            differing.len(),
            &differing[..differing.len().min(6)]
        );
    }
}

/// Timing comparison, run by hand:
/// `cargo test --release --lib bench_gpu_vs_cpu -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_gpu_vs_cpu() {
    use std::time::Instant;
    let Some(gpu) = gpu_or_skip() else { return };
    let model = terrain(400);
    let query = MeshQuery::new(&model);
    let probes = random_probes(1_500_000, 200.0);

    let started = Instant::now();
    let cpu = exact_fits_parallel(&query, &probes);
    let cpu_time = started.elapsed();

    let started = Instant::now();
    let engine = ProbeEngine::with_gpu(&query, Some(gpu.clone()));
    let build_time = started.elapsed();
    let started = Instant::now();
    let assisted = engine.fits(&probes);
    let gpu_time = started.elapsed();
    assert_eq!(cpu, assisted);
    let stats = engine.last_stats();
    eprintln!(
        "probes: {} triangles, {} probes | CPU (all cores) {:.2?} | GPU bounds {:.2?} + queries {:.2?} | {:.1}% decided on GPU",
        model.triangles.len(),
        probes.len(),
        cpu_time,
        build_time,
        gpu_time,
        100.0 * stats.decided_on_gpu as f64 / stats.total as f64
    );

    let spec = GridSpec {
        origin: [0.0, 0.0],
        cell: [0.2, 0.2],
        columns: 640,
        rows: 640,
        floor: -5.0,
    };
    let mut random = Lcg(9);
    let cuts: Vec<Cut> = (0..300_000)
        .map(|_| {
            let a = [
                random.next() as f32 * 120.0,
                random.next() as f32 * 120.0,
                random.next() as f32 * 8.0 - 6.0,
            ];
            let b = [
                a[0] + random.next() as f32 * 3.0,
                a[1] + random.next() as f32 * 3.0,
                a[2],
            ];
            [a, b]
        })
        .collect();
    let tool = CutTool {
        radius: 1.5,
        corner: 0.0,
    };
    let mut on_cpu = vec![10.0_f32; spec.columns * spec.rows];
    let started = Instant::now();
    cut_capsules_cpu(&spec, &mut on_cpu, &cuts, tool);
    let cpu_cut = started.elapsed();
    let mut on_gpu = vec![10.0_f32; spec.columns * spec.rows];
    let started = Instant::now();
    gpu.cut_capsules(&spec, &mut on_gpu, &cuts, tool).unwrap();
    let gpu_cut = started.elapsed();
    eprintln!(
        "stock cut: {} cuts on a {}x{} grid | CPU (1 core) {:.2?} | GPU {:.2?}",
        cuts.len(),
        spec.columns,
        spec.rows,
        cpu_cut,
        gpu_cut
    );
}
