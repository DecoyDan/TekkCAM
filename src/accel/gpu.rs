//! wgpu compute backend: mesh height bounds, probe classification and stock
//! cutting. Every call is wrapped in a wgpu error scope so a driver or shader
//! problem becomes an `Err` (and the caller falls back to the CPU) instead of
//! taking the application down.

use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use egui_wgpu::wgpu::{self, util::DeviceExt};

use super::{Cut, CutTool, GridSpec, Probe};

const BOUNDS_SHADER: &str = include_str!("bounds.wgsl");
const CLASSIFY_SHADER: &str = include_str!("classify.wgsl");
const CUT_SHADER: &str = include_str!("cut.wgsl");

/// Most probes sent in one dispatch.
const PROBE_CHUNK: usize = 1 << 20;
/// Largest dispatch (workgroups) along one dimension.
const MAX_GROUPS: u32 = 65_535;
/// Vertical/horizontal slack (mm) the height bounds are widened by to absorb
/// the difference between the GPU's f32 and the CPU's f64 arithmetic.
const BOUND_MARGIN_MM: f32 = 2e-3;
/// The mesh bound grid never gets finer than this many cells on a side.
const MAX_GRID_SIDE: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GridParams {
    origin: [f32; 2],
    cell: f32,
    margin: f32,
    columns: u32,
    rows: u32,
    count: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GpuProbe {
    x: f32,
    y: f32,
    limit: f32,
    radius: f32,
    ring: u32,
    center: u32,
    _p0: u32,
    _p1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct CutParams {
    origin: [f32; 2],
    cell: [f32; 2],
    radius: f32,
    floor_z: f32,
    columns: u32,
    rows: u32,
    count: u32,
    ball: u32,
}

struct Kernel {
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

pub struct GpuContext {
    device: wgpu::Device,
    queue: wgpu::Queue,
    adapter_name: String,
    /// wgpu error scopes are shared by every thread using the device, so GPU
    /// jobs (planner thread, viewport) take turns instead of interleaving them.
    gate: std::sync::Mutex<()>,
    bounds: Kernel,
    classify: Kernel,
    cut: Kernel,
}

/// Conservative height bounds of one mesh on the GPU.
pub struct GpuBounds {
    gpu: Arc<GpuContext>,
    params: GridParams,
    upper: wgpu::Buffer,
    lower: wgpu::Buffer,
}

fn encode(value: f32) -> u32 {
    let bits = value.to_bits();
    if bits & 0x8000_0000 == 0 {
        bits | 0x8000_0000
    } else {
        !bits
    }
}

fn decode(key: u32) -> f32 {
    if key & 0x8000_0000 != 0 {
        f32::from_bits(key & 0x7fff_ffff)
    } else {
        f32::from_bits(!key)
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn make_kernel(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    entries: &[wgpu::BindGroupLayoutEntry],
) -> Kernel {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(label),
        entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[&layout],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    Kernel { pipeline, layout }
}

impl GpuContext {
    /// Own device on the best available adapter (CLI, tests).
    pub fn headless() -> Result<Arc<Self>, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }))
        .ok_or_else(|| "no GPU adapter available".to_string())?;
        let info = adapter.get_info();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default(), None))
                .map_err(|error| format!("cannot open the GPU: {error}"))?;
        Self::from_device(device, queue, format!("{} ({:?})", info.name, info.backend))
    }

    /// Use the device the application already renders with.
    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_name: String,
    ) -> Result<Arc<Self>, String> {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bounds = make_kernel(
            &device,
            "tekkcam_bounds",
            BOUNDS_SHADER,
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, false),
                storage_entry(4, false),
            ],
        );
        let classify = make_kernel(
            &device,
            "tekkcam_classify",
            CLASSIFY_SHADER,
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, true),
                storage_entry(3, true),
                storage_entry(4, false),
            ],
        );
        let cut = make_kernel(
            &device,
            "tekkcam_cut",
            CUT_SHADER,
            &[
                uniform_entry(0),
                storage_entry(1, true),
                storage_entry(2, false),
            ],
        );
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("GPU kernels failed to compile: {error}"));
        }
        Ok(Arc::new(Self {
            device,
            queue,
            adapter_name,
            gate: std::sync::Mutex::new(()),
            bounds,
            classify,
            cut,
        }))
    }

    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// Run `work` inside validation and out-of-memory error scopes.
    fn guarded<T>(&self, work: impl FnOnce() -> T) -> Result<T, String> {
        let _turn = self
            .gate
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let output = work();
        let validation = pollster::block_on(self.device.pop_error_scope());
        let memory = pollster::block_on(self.device.pop_error_scope());
        match (validation, memory) {
            (Some(error), _) => Err(format!("GPU validation error: {error}")),
            (None, Some(error)) => Err(format!("GPU out of memory: {error}")),
            (None, None) => Ok(output),
        }
    }

    fn buffer_init(&self, label: &str, contents: &[u8], usage: wgpu::BufferUsages) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage,
            })
    }

    /// Copy a storage buffer back to the CPU.
    fn read_back(&self, source: &wgpu::Buffer, bytes: u64) -> Result<Vec<u8>, String> {
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tekkcam_readback"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("tekkcam_readback"),
            });
        encoder.copy_buffer_to_buffer(source, 0, &staging, 0, bytes);
        self.queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device.poll(wgpu::Maintain::Wait);
        receiver
            .recv()
            .map_err(|_| "GPU readback was interrupted".to_string())?
            .map_err(|error| format!("GPU readback failed: {error}"))?;
        let data = staging.slice(..).get_mapped_range().to_vec();
        staging.unmap();
        Ok(data)
    }

    fn dispatch(
        &self,
        kernel: &Kernel,
        entries: &[wgpu::BindGroupEntry],
        groups: (u32, u32),
        label: &str,
    ) {
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &kernel.layout,
            entries,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some(label) });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(label),
                timestamp_writes: None,
            });
            pass.set_pipeline(&kernel.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(groups.0, groups.1, 1);
        }
        self.queue.submit([encoder.finish()]);
    }

    /// Build conservative height bounds for a mesh given as transformed
    /// vertices and triangle indices.
    pub fn build_bounds(
        self: &Arc<Self>,
        vertices: &[[f64; 3]],
        triangles: &[[u32; 3]],
    ) -> Result<GpuBounds, String> {
        if triangles.is_empty() || vertices.is_empty() {
            return Err("empty mesh".into());
        }
        if triangles.len() > (MAX_GROUPS as usize) * 64 {
            return Err("mesh too large for one GPU dispatch".into());
        }
        let mut low = [f64::INFINITY; 2];
        let mut high = [f64::NEG_INFINITY; 2];
        for vertex in vertices {
            for axis in 0..2 {
                low[axis] = low[axis].min(vertex[axis]);
                high[axis] = high[axis].max(vertex[axis]);
            }
        }
        if low
            .iter()
            .chain(high.iter())
            .any(|value| !value.is_finite())
        {
            return Err("mesh has non-finite vertices".into());
        }
        let extent = (high[0] - low[0]).max(high[1] - low[1]).max(1e-3);
        let cell = (extent / (MAX_GRID_SIDE as f64 * 0.75)).clamp(0.05, 4.0);
        let columns = (((high[0] - low[0]) / cell).ceil() as usize + 3).min(MAX_GRID_SIDE * 2);
        let rows = (((high[1] - low[1]) / cell).ceil() as usize + 3).min(MAX_GRID_SIDE * 2);
        let params = GridParams {
            origin: [(low[0] - 1.5 * cell) as f32, (low[1] - 1.5 * cell) as f32],
            cell: cell as f32,
            margin: BOUND_MARGIN_MM,
            columns: columns as u32,
            rows: rows as u32,
            count: triangles.len() as u32,
            _pad: 0,
        };
        let vertex_data: Vec<[f32; 4]> = vertices
            .iter()
            .map(|v| [v[0] as f32, v[1] as f32, v[2] as f32, 0.0])
            .collect();
        let triangle_data: Vec<[u32; 4]> =
            triangles.iter().map(|t| [t[0], t[1], t[2], 0]).collect();
        let cells = columns * rows;
        let gpu = self.clone();
        self.guarded(|| {
            let uniform = gpu.buffer_init(
                "bounds_params",
                bytemuck::bytes_of(&params),
                wgpu::BufferUsages::UNIFORM,
            );
            let vertex_buffer = gpu.buffer_init(
                "bounds_vertices",
                bytemuck::cast_slice(&vertex_data),
                wgpu::BufferUsages::STORAGE,
            );
            let triangle_buffer = gpu.buffer_init(
                "bounds_triangles",
                bytemuck::cast_slice(&triangle_data),
                wgpu::BufferUsages::STORAGE,
            );
            let zeros = vec![0u32; cells];
            let upper = gpu.buffer_init(
                "bounds_upper",
                bytemuck::cast_slice(&zeros),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            let lower = gpu.buffer_init(
                "bounds_lower",
                bytemuck::cast_slice(&zeros),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            );
            gpu.dispatch(
                &gpu.bounds,
                &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: vertex_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: triangle_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: upper.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: lower.as_entire_binding(),
                    },
                ],
                (triangles.len().div_ceil(64) as u32, 1),
                "bounds",
            );
            GpuBounds {
                gpu: gpu.clone(),
                params,
                upper,
                lower,
            }
        })
    }

    /// Lower `heights` (a `columns × rows` grid of node heights) with cuts.
    pub fn cut_capsules(
        &self,
        spec: &GridSpec,
        heights: &mut [f32],
        cuts: &[Cut],
        tool: CutTool,
    ) -> Result<(), String> {
        let nodes = spec.columns * spec.rows;
        if heights.len() != nodes || nodes == 0 {
            return Err("height grid size does not match its spec".into());
        }
        let mut offset = 0;
        // Very long cut lists are processed in slices so a dispatch stays short.
        while offset < cuts.len() {
            let slice = &cuts[offset..(offset + (MAX_GROUPS as usize) * 8).min(cuts.len())];
            self.cut_slice(spec, heights, slice, tool)?;
            offset += slice.len();
        }
        Ok(())
    }

    fn cut_slice(
        &self,
        spec: &GridSpec,
        heights: &mut [f32],
        cuts: &[Cut],
        tool: CutTool,
    ) -> Result<(), String> {
        let params = CutParams {
            origin: [spec.origin[0] as f32, spec.origin[1] as f32],
            cell: [spec.cell[0] as f32, spec.cell[1] as f32],
            radius: tool.radius,
            floor_z: spec.floor,
            columns: spec.columns as u32,
            rows: spec.rows as u32,
            count: cuts.len() as u32,
            ball: u32::from(tool.ball),
        };
        let cut_data: Vec<[f32; 4]> = cuts
            .iter()
            .flat_map(|cut| {
                [
                    [cut[0][0], cut[0][1], cut[0][2], 0.0],
                    [cut[1][0], cut[1][1], cut[1][2], 0.0],
                ]
            })
            .collect();
        let keys: Vec<u32> = heights.iter().map(|height| encode(*height)).collect();
        let bytes = (keys.len() * 4) as u64;
        let groups = {
            let count = cuts.len() as u32;
            (count.min(MAX_GROUPS), count.div_ceil(MAX_GROUPS).max(1))
        };
        let data = self.guarded(|| {
            let uniform = self.buffer_init(
                "cut_params",
                bytemuck::bytes_of(&params),
                wgpu::BufferUsages::UNIFORM,
            );
            let cut_buffer = self.buffer_init(
                "cut_segments",
                bytemuck::cast_slice(&cut_data),
                wgpu::BufferUsages::STORAGE,
            );
            let height_buffer = self.buffer_init(
                "cut_heights",
                bytemuck::cast_slice(&keys),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            );
            self.dispatch(
                &self.cut,
                &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: cut_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: height_buffer.as_entire_binding(),
                    },
                ],
                groups,
                "cut",
            );
            self.read_back(&height_buffer, bytes)
        })??;
        let results: &[u32] = bytemuck::cast_slice(&data);
        for (height, key) in heights.iter_mut().zip(results) {
            *height = decode(*key);
        }
        Ok(())
    }
}

impl GpuBounds {
    /// Classify probes: `0` fits for certain, `1` blocked for certain, `2`
    /// undecided (needs the exact CPU test).
    pub fn classify(&self, probes: &[Probe]) -> Result<Vec<u8>, String> {
        let mut out = Vec::with_capacity(probes.len());
        for chunk in probes.chunks(PROBE_CHUNK) {
            out.extend(self.classify_chunk(chunk)?);
        }
        Ok(out)
    }

    fn classify_chunk(&self, probes: &[Probe]) -> Result<Vec<u8>, String> {
        let gpu = &self.gpu;
        let mut params = self.params;
        params.count = probes.len() as u32;
        let data: Vec<GpuProbe> = probes
            .iter()
            .map(|probe| GpuProbe {
                x: probe.x as f32,
                y: probe.y as f32,
                limit: probe.limit as f32,
                radius: probe.radius as f32,
                ring: probe.ring,
                center: u32::from(probe.center),
                _p0: 0,
                _p1: 0,
            })
            .collect();
        let bytes = (probes.len() * 4) as u64;
        let raw = gpu.guarded(|| {
            let uniform = gpu.buffer_init(
                "classify_params",
                bytemuck::bytes_of(&params),
                wgpu::BufferUsages::UNIFORM,
            );
            let probe_buffer = gpu.buffer_init(
                "classify_probes",
                bytemuck::cast_slice(&data),
                wgpu::BufferUsages::STORAGE,
            );
            let result = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("classify_result"),
                size: bytes,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            });
            gpu.dispatch(
                &gpu.classify,
                &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: uniform.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: probe_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.upper.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.lower.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: result.as_entire_binding(),
                    },
                ],
                (probes.len().div_ceil(64) as u32, 1),
                "classify",
            );
            gpu.read_back(&result, bytes)
        })??;
        let classes: &[u32] = bytemuck::cast_slice(&raw);
        Ok(classes.iter().map(|class| *class as u8).collect())
    }
}
