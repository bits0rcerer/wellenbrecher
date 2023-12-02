use std::iter;

use bytemuck_derive::{Pod, Zeroable};
use tracing::{error, warn};
use wgpu::util::DeviceExt;
use wgpu::{
    Backends, BindGroup, CompositeAlphaMode, ImageDataLayout, PresentMode, PushConstantRange,
    ShaderStages, TextureFormat,
};
use winit::{
    event::*,
    event_loop::{ControlFlow, EventLoop},
    window::Window,
};

use wellenbrecher_canvas::{Bgra, Canvas, UserID};

use crate::texture::Texture;

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
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    size: winit::dpi::PhysicalSize<u32>,
    render_pipeline: wgpu::RenderPipeline,
    vertex_buffer: wgpu::Buffer,
    canvas_texture: Texture,
    uid_map_texture: Texture,
    window: Window,
    bind_group: BindGroup,
    canvas: Canvas,
}

impl State {
    async fn new(window: Window, gpu_index: usize, canvas: Canvas) -> eyre::Result<Self> {
        let size = window.inner_size();

        let instance = wgpu::Instance::default();

        let surface = unsafe { instance.create_surface(&window) }.unwrap();
        let adapter = instance
            .enumerate_adapters(Backends::all())
            .filter(|a| a.is_surface_supported(&surface))
            .nth(gpu_index)
            .expect("Failed to find an appropriate adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    features: wgpu::Features::PUSH_CONSTANTS,
                    limits: wgpu::Limits {
                        max_push_constant_size: std::mem::size_of::<Push>() as u32,
                        ..wgpu::Limits::default().using_resolution(adapter.limits())
                    },
                },
                None,
            )
            .await
            .expect("Failed to create device");

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
                    ty: wgpu::BindingType::Texture {
                        multisampled: false,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        sample_type: wgpu::TextureSampleType::Uint,
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

        let uid_map_texture = Texture::new(
            &device,
            TextureFormat::R8Uint,
            canvas.width(),
            canvas.height(),
            Some("uid_map_texture"),
        )?;

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

    #[allow(unused_variables)]
    fn input(&mut self, event: &WindowEvent) -> bool {
        false
    }

    fn update(&mut self) {}

    fn render(&mut self) -> Result<(), wgpu::SurfaceError> {
        let output = self.surface.get_current_texture()?;
        let view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Render Encoder"),
            });

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
                bytemuck::bytes_of(&Push {
                    blend_to: [0.0; 4],
                    user_id_filter: 0,
                    blending: 0.0,
                }),
            );
            render_pass.draw(0..4, 0..1);
        }

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

        self.queue.submit(iter::once(encoder.finish()));
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
