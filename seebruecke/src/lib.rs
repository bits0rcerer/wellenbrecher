use std::sync::Arc;
use std::time::Instant;

use bytemuck_derive::{Pod, Zeroable};
use egui::ahash::{HashMap, HashMapExt};
use egui::mutex::RwLock;
use egui::{Align2, ViewportId};
use egui_winit::EventResponse;
use tracing::{error, info, warn};
use wgpu::util::{BufferInitDescriptor, DeviceExt};
use wgpu::{
    Backends, BindGroup, BufferBindingType, BufferUsages, CompositeAlphaMode, ImageDataLayout,
    PresentMode, PushConstantRange, ShaderStages, StorageTextureAccess, TextureFormat,
};
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::Window,
};

use wellenbrecher_canvas::{Bgra, Canvas, UserID};

use crate::texture::{StorageTexture, Texture};

mod texture;

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    tex_coords: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Pod, Zeroable)]
struct Push {
    blend_to: [f32; 4],
    user_id_filter: u32,
    blending: f32,
}

#[repr(C)]
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
struct FragmentShaderState {
    last_highlighted_uid: u32,
}

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        use std::mem;
        wgpu::VertexBufferLayout {
            array_stride: mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute {
                    offset: 0,
                    shader_location: 0,
                    format: wgpu::VertexFormat::Float32x2,
                },
                wgpu::VertexAttribute {
                    offset: mem::size_of::<[f32; 2]>() as wgpu::BufferAddress,
                    shader_location: 1,
                    format: wgpu::VertexFormat::Float32x2,
                },
            ],
        }
    }
}

struct State {
    surface: wgpu::Surface,
    device: Arc<wgpu::Device>,
    queue: Arc<wgpu::Queue>,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    canvas_texture: Texture,
    uid_map_texture: StorageTexture,
    window: Window,
    bind_group: BindGroup,
    canvas: Canvas,
    push_constants: Push,
    egui_state: egui_winit::State,
    egui_context: egui::Context,
    egui_render_state: egui_wgpu::RenderState,
    last_rx_bytes: (Instant, u64, f64),
}

impl State {
    async fn new(window: Window, gpu_index: usize, canvas: Canvas) -> eyre::Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();

        let surface = unsafe { instance.create_surface(&window) }.unwrap();
        let adapter = Arc::new(
            instance
                .enumerate_adapters(Backends::all())
                .filter(|a| a.is_surface_supported(&surface))
                .nth(gpu_index)
                .expect("Failed to find an appropriate adapter"),
        );

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    features: wgpu::Features::PUSH_CONSTANTS
                        | wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES,
                    limits: wgpu::Limits {
                        max_push_constant_size: std::mem::size_of::<Push>() as u32,
                        ..wgpu::Limits::default().using_resolution(adapter.limits())
                    },
                },
                None,
            )
            .await
            .expect("Failed to create device");
        let device = Arc::new(device);
        let queue = Arc::new(queue);

        let surface_format = TextureFormat::Bgra8UnormSrgb;
        let present_mode = PresentMode::AutoVsync;
        let alpha_mode = CompositeAlphaMode::Auto;
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: size.width,
            height: size.height,
            present_mode,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::StorageTexture {
                        access: StorageTextureAccess::ReadOnly,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        format: TextureFormat::R32Uint,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::StorageTexture {
                        access: StorageTextureAccess::ReadWrite,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        format: TextureFormat::Rgba8Unorm,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::StorageTexture {
                        access: StorageTextureAccess::ReadWrite,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        format: TextureFormat::R32Uint,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
            label: Some("bind_group_layout"),
        });

        let canvas_texture = Texture::new(
            &device,
            TextureFormat::Bgra8UnormSrgb,
            canvas.width(),
            canvas.height(),
            Some("canvas_texture"),
        )?;

        let uid_map_texture = StorageTexture::new(
            &device,
            TextureFormat::R32Uint,
            canvas.width(),
            canvas.height(),
            Some("user_id_map"),
        )?;

        let secondary_canvas_texture = StorageTexture::new(
            &device,
            TextureFormat::Rgba8Unorm,
            canvas.width(),
            canvas.height(),
            Some("secondary_canvas_texture"),
        )?;
        queue.write_texture(
            secondary_canvas_texture.texture.as_image_copy(),
            vec![0u8; (canvas.width() * canvas.height()) as usize * std::mem::size_of::<Bgra>()]
                .as_slice(),
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(canvas.width() * std::mem::size_of::<Bgra>() as u32),
                rows_per_image: Some(canvas.height()),
            },
            canvas_texture.texture.size(),
        );

        let secondary_uid_map_texture = StorageTexture::new(
            &device,
            TextureFormat::R32Uint,
            canvas.width(),
            canvas.height(),
            Some("secondary_user_id_map"),
        )?;
        queue.write_texture(
            secondary_uid_map_texture.texture.as_image_copy(),
            vec![0u8; (canvas.width() * canvas.height()) as usize * std::mem::size_of::<UserID>()]
                .as_slice(),
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(canvas.width() * std::mem::size_of::<UserID>() as u32),
                rows_per_image: Some(canvas.height()),
            },
            canvas_texture.texture.size(),
        );

        let fragment_shader_state = device.create_buffer_init(&BufferInitDescriptor {
            contents: bytemuck::bytes_of(&FragmentShaderState::default()),
            usage: BufferUsages::STORAGE,
            label: Some("fragment_shader_state"),
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&canvas_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&canvas_texture.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&uid_map_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&secondary_canvas_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(&secondary_uid_map_texture.view),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Buffer(
                        fragment_shader_state.as_entire_buffer_binding(),
                    ),
                },
            ],
            label: Some("bind_group"),
        });

        let vertex_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("vertex_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("vertex.wgsl").into()),
        });

        let fragment_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fragment_shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fragment.wgsl").into()),
        });

        let push_constant_range = PushConstantRange {
            stages: ShaderStages::FRAGMENT,
            range: (0u32..std::mem::size_of::<Push>() as u32),
        };

        let render_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("render_pipeline_layout"),
                bind_group_layouts: &[&bind_group_layout],
                push_constant_ranges: &[push_constant_range],
            });

        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("render_pipeline"),
            layout: Some(&render_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &vertex_shader,
                entry_point: "main",
                buffers: &[Vertex::desc()],
            },
            fragment: Some(wgpu::FragmentState {
                module: &fragment_shader,
                entry_point: "main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: config.format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent::REPLACE,
                        alpha: wgpu::BlendComponent::REPLACE,
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState {
                count: 1,
                mask: !0,
                alpha_to_coverage_enabled: false,
            },
            multiview: None,
        });

        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("vertex_buffer"),
            contents: bytemuck::cast_slice(&[
                Vertex {
                    position: [1.0, 0.0],
                    tex_coords: [1.0, 1.0],
                },
                Vertex {
                    position: [1.0, 1.0],
                    tex_coords: [1.0, 0.0],
                },
                Vertex {
                    position: [0.0, 0.0],
                    tex_coords: [0.0, 1.0],
                },
                Vertex {
                    position: [0.0, 1.0],
                    tex_coords: [0.0, 0.0],
                },
            ]),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });

        let push_constants = Push {
            blend_to: [0.0; 4],
            user_id_filter: 0,
            blending: 0.9,
        };

        let egui_state = egui_winit::State::new(
            ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
        );
        let egui_context = egui::Context::default();
        let egui_render_state = egui_wgpu::RenderState {
            adapter,
            device: device.clone(),
            queue: queue.clone(),
            target_format: TextureFormat::R8Unorm,
            renderer: Arc::new(RwLock::new(egui_wgpu::Renderer::new(
                device.as_ref(),
                surface_format,
                None,
                1,
            ))),
        };

        Ok(Self {
            surface,
            device,
            queue,
            config,
            size,
            render_pipeline,
            vertex_buffer,
            canvas_texture,
            uid_map_texture,
            bind_group,
            window,
            canvas,
            push_constants,
            egui_state,
            egui_context,
            egui_render_state,
            last_rx_bytes: (Instant::now(), 0, 0.0f64),
        })
    }

    pub fn window(&self) -> &Window {
        &self.window
    }

    pub fn resize(&mut self, new_size: winit::dpi::PhysicalSize<u32>) {
        if new_size.width > 0 && new_size.height > 0 {
            self.size = new_size;
            self.config.width = new_size.width;
            self.config.height = new_size.height;

            let canvas_ratio = self.canvas.width() as f32 / self.canvas.height() as f32;
            let (width, height) = match self.size.height as f32 * canvas_ratio {
                draw_width if draw_width <= self.size.width as f32 => {
                    (draw_width, self.size.height as f32)
                }
                _ => (
                    self.size.width as f32,
                    self.size.width as f32 * (1.0 / canvas_ratio),
                ),
            };
            let x = width / self.size.width as f32;
            let x_offset = (1.0 - x) / 2.0;
            let y = height / self.size.height as f32;
            let y_offset = (1.0 - y) / 2.0;

            self.queue.write_buffer(
                &self.vertex_buffer,
                0,
                bytemuck::cast_slice(&[
                    Vertex {
                        position: [x_offset + x, y_offset],
                        tex_coords: [1.0, 1.0],
                    },
                    Vertex {
                        position: [x_offset + x, y_offset + y],
                        tex_coords: [1.0, 0.0],
                    },
                    Vertex {
                        position: [x_offset, y_offset],
                        tex_coords: [0.0, 1.0],
                    },
                    Vertex {
                        position: [x_offset, y_offset + y],
                        tex_coords: [0.0, 0.0],
                    },
                ]),
            );

            self.surface.configure(&self.device, &self.config);
        }
    }

    fn input(&mut self, event: &WindowEvent) -> bool {
        let EventResponse { consumed, .. } =
            self.egui_state.on_window_event(&self.egui_context, event);
        if consumed {
            return true;
        }

        match event {
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::Left),
                        ..
                    },
                ..
            } => {
                self.push_constants.blending = f32::max(0.0, self.push_constants.blending - 0.1);
                true
            }
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::Right),
                        ..
                    },
                ..
            } => {
                self.push_constants.blending = f32::min(1.0, self.push_constants.blending + 0.1);
                true
            }
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::Up),
                        ..
                    },
                ..
            } => {
                self.push_constants.user_id_filter =
                    self.push_constants.user_id_filter.saturating_add(1);
                info!("Highlighting User: {}", self.push_constants.user_id_filter);
                true
            }
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::Down),
                        ..
                    },
                ..
            } => {
                self.push_constants.user_id_filter =
                    self.push_constants.user_id_filter.saturating_sub(1);
                info!("Highlighting User: {}", self.push_constants.user_id_filter);
                true
            }
            WindowEvent::KeyboardInput {
                input:
                    KeyboardInput {
                        state: ElementState::Pressed,
                        virtual_keycode: Some(VirtualKeyCode::R),
                        ..
                    },
                ..
            } => {
                self.push_constants.user_id_filter = 0;
                info!("Highlighting cleared");
                true
            }
            _ => false,
        }
    }

    fn update(&mut self) {}

    fn build_egui(&self, ctx: &egui::Context, mut rx_bits_per_secs: f64) {
        let pixel_user_map = self
            .canvas
            .user_id_slice()
            .iter()
            .filter(|&&uid| uid > 0)
            .fold(HashMap::new(), |mut map, uid| {
                match map.get_mut(uid) {
                    Some(pixels) => *pixels += 1,
                    None => {
                        map.insert(uid, 1);
                    }
                }
                map
            });

        let mut traffic = String::default();
        for unit in ["Bit", "kBit", "MBit", "GBit", "PBit"] {
            traffic = format!("{rx_bits_per_secs:.1} {unit}");
            if rx_bits_per_secs < 1000.0 {
                break;
            }
            rx_bits_per_secs /= 1024.0;
        }

        egui::Window::new("Stats")
            .anchor(Align2::RIGHT_TOP, [-50.0, 50.0])
            .default_size([120.0, 30.0])
            .resizable(true)
            .movable(true)
            .default_open(true)
            .title_bar(false)
            .show(ctx, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(ui.available_height());
                ui.colored_label(
                    egui::Color32::WHITE,
                    format!("Players: {}", pixel_user_map.len()),
                );
                ui.colored_label(egui::Color32::WHITE, format!("Traffic: {traffic}"));
            });
    }

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        self.queue.write_texture(
            self.canvas_texture.texture.as_image_copy(),
            self.canvas.pixel_byte_slice(),
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.canvas.width() * std::mem::size_of::<Bgra>() as u32),
                rows_per_image: Some(self.canvas.height()),
            },
            self.canvas_texture.texture.size(),
        );
        self.queue.write_texture(
            self.uid_map_texture.texture.as_image_copy(),
            self.canvas.user_id_byte_slice(),
            ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(self.canvas.width() * std::mem::size_of::<UserID>() as u32),
                rows_per_image: Some(self.canvas.height()),
            },
            self.uid_map_texture.texture.size(),
        );

        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

        let rx_bytes_total: u64 = sys_metrics::network::get_ionets()
            .expect("unable to get network info")
            .into_iter()
            .map(|m| m.rx_bytes)
            .sum();

        let secs = self.last_rx_bytes.0.elapsed().as_secs_f64();
        let rx_bits_per_secs = if secs.is_normal() {
            let rx_bits_per_secs = 8.0 * (rx_bytes_total - self.last_rx_bytes.1) as f64 / secs;
            self.last_rx_bytes = (Instant::now(), rx_bytes_total, rx_bits_per_secs);
            rx_bits_per_secs
        } else {
            self.last_rx_bytes.2
        };

        let egui::FullOutput {
            platform_output,
            textures_delta,
            shapes,
            pixels_per_point,
            ..
        } = self
            .egui_context
            .run(self.egui_state.take_egui_input(&self.window), |ctx| {
                self.build_egui(ctx, rx_bits_per_secs)
            });

        self.egui_state
            .handle_platform_output(&self.window, &self.egui_context, platform_output);

        let clipped_primitives = self.egui_context.tessellate(shapes, pixels_per_point);

        let scale_factor = self.window.scale_factor() as f32;
        let screen_descriptor = egui_wgpu::renderer::ScreenDescriptor {
            size_in_pixels: [self.size.width, self.size.height],
            pixels_per_point: scale_factor,
        };

        let mut egui_renderer = self.egui_render_state.renderer.write();

        for (id, image_delta) in &textures_delta.set {
            egui_renderer.update_texture(&self.device, &self.queue, *id, image_delta);
        }

        let mut command_buffers = egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Render Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            render_pass.set_pipeline(&self.render_pipeline);
            render_pass.set_bind_group(0, &self.bind_group, &[]);
            render_pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
            render_pass.set_push_constants(
                ShaderStages::FRAGMENT,
                0,
                bytemuck::bytes_of(&self.push_constants),
            );
            render_pass.draw(0..4, 0..1);
            egui_renderer.render(&mut render_pass, &clipped_primitives, &screen_descriptor);
        }

        for id in &textures_delta.free {
            egui_renderer.free_texture(id);
        }

        command_buffers.push(encoder.finish());

        self.queue.submit(command_buffers);
        output.present();

        Ok(())
    }
}

pub async fn run(
    canvas: Canvas,
    event_loop: EventLoop<()>,
    window: Window,
    gpu_index: usize,
) -> eyre::Result<()> {
    let mut state = State::new(window, gpu_index, canvas).await?;

    event_loop.run(move |event, _, control_flow| {
        match event {
            Event::WindowEvent {
                ref event,
                window_id,
            } if window_id == state.window().id() => {
                if !state.input(event) {
                    match event {
                        WindowEvent::CloseRequested
                        | WindowEvent::KeyboardInput {
                            input:
                                KeyboardInput {
                                    state: ElementState::Pressed,
                                    virtual_keycode: Some(VirtualKeyCode::Escape),
                                    ..
                                },
                            ..
                        } => *control_flow = ControlFlow::Exit,
                        WindowEvent::Resized(physical_size) => {
                            state.resize(*physical_size);
                        }
                        WindowEvent::ScaleFactorChanged { new_inner_size, .. } => {
                            state.resize(**new_inner_size);
                        }
                        _ => {}
                    }
                }
            }
            Event::RedrawRequested(window_id) if window_id == state.window().id() => {
                state.update();
                match state.render() {
                    Ok(_) => {}
                    // Reconfigure the surface if it's lost or outdated
                    Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                        state.resize(state.size)
                    }
                    Err(wgpu::SurfaceError::OutOfMemory) => {
                        error!("wgpu::SurfaceError::OutOfMemory");
                        *control_flow = ControlFlow::Exit
                    }
                    // We're ignoring timeouts
                    Err(wgpu::SurfaceError::Timeout) => warn!("Surface timeout"),
                }
            }
            Event::MainEventsCleared => {
                state.window().request_redraw();
            }
            _ => {}
        }
    });
}
