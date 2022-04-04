use std::cell::RefCell;
use std::collections::HashMap;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use arc_swap::ArcSwap;
use cgmath::{Matrix4, SquareMatrix};
use futures::StreamExt;
use once_cell::sync::OnceCell;
use wgpu::{BindGroupDescriptor, BindGroupEntry, PipelineLayoutDescriptor, RenderPass, RenderPipeline, VertexState};
use wgpu::util::{BufferInitDescriptor, DeviceExt};

use wgpu_mc::{wgpu, WmRenderer};
use wgpu_mc::camera::UniformMatrixHelper;
use wgpu_mc::model::BindableTexture;
use wgpu_mc::render::pipeline::WmPipeline;
use wgpu_mc::render::shader::{GlslShader, WgslShader, WmShader};
use wgpu_mc::texture::TextureSamplerView;
use wgpu_mc::util::WmArena;
use wgpu_mc::wgpu::PipelineLayout;

use crate::{Extent3d, gl, GlTexture};
use crate::wgpu::{BindGroup, BlendComponent, BlendState, Label};

// #[rustfmt::skip]
// pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
//     1.0, 0.0, 0.0, 0.0,
//     0.0, 1.0, 0.0, 0.0,
//     0.0, 0.0, 0.5, 0.0,
//     0.0, 0.0, 0.5, 1.0,
// );

#[rustfmt::skip]
pub const OPENGL_TO_WGPU_MATRIX: cgmath::Matrix4<f32> = cgmath::Matrix4::new(
    1.0, 0.0, 0.0, 0.0,
    0.0, 1.0, 0.0, 0.0,
    0.0, 0.0, 0.5, 0.5,
    0.0, 0.0, 0.0, 1.0,
);

#[derive(Clone, Debug)]
pub enum GLCommand {
    SetMatrix(Matrix4<f32>),
    ClearColor(f32, f32, f32),
    UsePipeline(usize),
    SetVertexBuffer(Vec<u8>),
    SetIndexBuffer(Vec<u32>),
    DrawIndexed(u32),
    Draw(u32),
    AttachTexture(i32)
}

#[derive(Debug)]
pub struct TextureUnit {
    pub target_tex_2d: i32,
    // target_tex_3d: i32
}

#[derive(Debug)]
pub struct GlPipeline {
    pub commands: ArcSwap<Vec<GLCommand>>,
    pub black_texture: OnceCell<Arc<BindableTexture>>
}

fn byte_buffer_to_short(bytes: &[u8]) -> Vec<u16> {
    bytes.iter().map(|byte| *byte as u16).collect()
}

impl WmPipeline for GlPipeline {
    fn name(&self) -> &'static str {
        "OpenGL"
    }

    fn provide_shaders(&self, wm: &WmRenderer) -> HashMap<String, Box<dyn WmShader>> {
        [
            (
                "wgpu_mc_ogl:shaders/pos_col_float3".into(),
                Box::new(WgslShader::init(
                    &("wgpu_mc", "shaders/gui_col_pos.wgsl").into(),
                    &*wm.mc.resource_provider,
                    &wm.wgpu_state.device,
                    "fs_main".into(),
                    "vs_main".into()
                )) as Box<dyn WmShader>
            ),
            (
                "wgpu_mc_ogl:shaders/pos_col_uint".into(),
                Box::new(WgslShader::init(
                    &("wgpu_mc", "shaders/gui_col_pos_uint.wgsl").into(),
                    &*wm.mc.resource_provider,
                    &wm.wgpu_state.device,
                    "fs_main".into(),
                    "vs_main".into()
                )) as Box<dyn WmShader>
            ),
            (
                "wgpu_mc_ogl:shaders/pos_tex".into(),
                Box::new(WgslShader::init(
                    &("wgpu_mc", "shaders/gui_uv_pos.wgsl").into(),
                    &*wm.mc.resource_provider,
                    &wm.wgpu_state.device,
                    "fs_main".into(),
                    "vs_main".into()
                )) as Box<dyn WmShader>
            ),
            (
                "wgpu_mc_ogl:shaders/clearcolor".into(),
                Box::new(WgslShader::init(
                    &("wgpu_mc", "shaders/clearcolor.wgsl").into(),
                    &*wm.mc.resource_provider,
                    &wm.wgpu_state.device,
                    "fs_main".into(),
                    "vs_main".into()
                )) as Box<dyn WmShader>
            )
        ].into_iter().collect()
    }

    fn atlases(&self) -> &'static [&'static str] {
        &[]
    }

    fn build_wgpu_pipeline_layouts(&self, wm: &WmRenderer) -> HashMap<String, PipelineLayout> {
        let pipeline_manager = wm.render_pipeline_manager.load();
        let layouts = pipeline_manager.bind_group_layouts.read();

        [
            (
                "wgpu_mc_ogl:layouts/pos_col".into(),
                wm.wgpu_state.device.create_pipeline_layout(
                    &wgpu::PipelineLayoutDescriptor {
                        label: Some("pos_col"),
                        bind_group_layouts: &[
                            &layouts.get("matrix4").unwrap()
                        ],
                        push_constant_ranges: &[]
                    }
                )
            ),
            (
                "wgpu_mc_ogl:layouts/pos_tex".into(),
                wm.wgpu_state.device.create_pipeline_layout(
                    &wgpu::PipelineLayoutDescriptor {
                        label: Some("pos_tex"),
                        bind_group_layouts: &[
                            layouts.get("matrix4").unwrap(),
                            layouts.get("texture").unwrap()
                        ],
                        push_constant_ranges: &[]
                    }
                )
            ),
            (
                "wgpu_mc_ogl:layouts/clearcolor".into(),
                wm.wgpu_state.device.create_pipeline_layout(
                    &wgpu::PipelineLayoutDescriptor {
                        label: Some("clearcolor"),
                        bind_group_layouts: &[],
                        push_constant_ranges: &[]
                    }
                )
            )
        ].into_iter().collect()
    }

    fn build_wgpu_pipelines(&self, wm: &WmRenderer) -> HashMap<String, RenderPipeline> {
        let pipeline_manager = wm.render_pipeline_manager.load();
        let layouts = pipeline_manager.pipeline_layouts.load();
        let shaders = pipeline_manager.shader_map.read();

        let black_tsv = TextureSamplerView::from_rgb_bytes(
            &wm.wgpu_state,
            &[0u8; 4],
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1
            },
            Some("Black Texture"),
            wgpu::TextureFormat::Bgra8Unorm
        ).unwrap();

        self.black_texture.set(
            Arc::new(BindableTexture::from_tsv(&wm.wgpu_state, &*pipeline_manager, black_tsv))
        );

        let pos_col_float3_shader = shaders.get("wgpu_mc_ogl:shaders/pos_col_float3").unwrap();
        let pos_col_uint_shader = shaders.get("wgpu_mc_ogl:shaders/pos_col_uint").unwrap();
        let pos_tex_shader = shaders.get("wgpu_mc_ogl:shaders/pos_tex").unwrap();
        let clearcolor_shader = shaders.get("wgpu_mc_ogl:shaders/clearcolor").unwrap();

        [
            (
                "wgpu_mc_ogl:pipelines/pos_col_float3".into(),
                wm.wgpu_state.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: None,
                        layout: Some(&layouts.get("wgpu_mc_ogl:layouts/pos_col").unwrap()),
                        vertex: VertexState {
                            module: &pos_col_float3_shader.get_vert().0,
                            entry_point: &pos_col_float3_shader.get_vert().1,
                            buffers: &[
                                wgpu::VertexBufferLayout {
                                    array_stride: 24,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &[
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x3,
                                            offset: 0,
                                            shader_location: 0
                                        },
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x3,
                                            offset: 12,
                                            shader_location: 1
                                        }
                                    ]
                                }
                            ]
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: None,
                            unclipped_depth: false,
                            polygon_mode: wgpu::PolygonMode::Fill,
                            conservative: false
                        },
                        depth_stencil: Some(
                            wgpu::DepthStencilState {
                                format: wgpu::TextureFormat::Depth32Float,
                                depth_write_enabled: false,
                                depth_compare: wgpu::CompareFunction::Always,
                                stencil: Default::default(),
                                bias: Default::default()
                            }
                        ),
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &pos_col_float3_shader.get_frag().0,
                            entry_point: &pos_col_float3_shader.get_frag().1,
                            targets: &[
                                wgpu::ColorTargetState {
                                    format: wgpu::TextureFormat::Bgra8Unorm,
                                    blend: None,
                                    write_mask: Default::default()
                                }
                            ]
                        }),
                        multiview: None
                    }
                )
            ),
            (
                "pos_tex".into(),
                wm.wgpu_state.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: None,
                        layout: Some(&layouts.get("wgpu_mc_ogl:layouts/pos_tex").unwrap()),
                        vertex: VertexState {
                            module: &pos_tex_shader.get_vert().0,
                            entry_point: &pos_tex_shader.get_vert().1,
                            buffers: &[
                                wgpu::VertexBufferLayout {
                                    array_stride: 20,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &[
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x3,
                                            offset: 0,
                                            shader_location: 0
                                        },
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x2,
                                            offset: 12,
                                            shader_location: 1
                                        }
                                    ]
                                }
                            ]
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: None,
                            unclipped_depth: false,
                            polygon_mode: wgpu::PolygonMode::Fill,
                            conservative: false
                        },
                        depth_stencil: Some(
                            wgpu::DepthStencilState {
                                format: wgpu::TextureFormat::Depth32Float,
                                depth_write_enabled: false,
                                depth_compare: wgpu::CompareFunction::Always,
                                stencil: Default::default(),
                                bias: Default::default()
                            }
                        ),
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &pos_tex_shader.get_frag().0,
                            entry_point: &pos_tex_shader.get_frag().1,
                            targets: &[
                                wgpu::ColorTargetState {
                                    format: wgpu::TextureFormat::Bgra8Unorm,
                                    blend: Some(BlendState {
                                        color: BlendComponent::OVER,
                                        alpha: BlendComponent::OVER
                                    }),
                                    write_mask: Default::default()
                                }
                            ]
                        }),
                        multiview: None
                    }
                )
            ),
            (
                "pos_col_uint".into(),
                wm.wgpu_state.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: None,
                        layout: Some(&layouts.get("wgpu_mc_ogl:layouts/pos_col").unwrap()),
                        vertex: VertexState {
                            module: &pos_col_uint_shader.get_vert().0,
                            entry_point: &pos_col_uint_shader.get_vert().1,
                            buffers: &[
                                wgpu::VertexBufferLayout {
                                    array_stride: 16,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &[
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x3,
                                            offset: 0,
                                            shader_location: 0
                                        },
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Uint32,
                                            offset: 12,
                                            shader_location: 1
                                        }
                                    ]
                                }
                            ]
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: None,
                            unclipped_depth: false,
                            polygon_mode: wgpu::PolygonMode::Fill,
                            conservative: false
                        },
                        depth_stencil: Some(
                            wgpu::DepthStencilState {
                                format: wgpu::TextureFormat::Depth32Float,
                                depth_write_enabled: false,
                                depth_compare: wgpu::CompareFunction::Always,
                                stencil: Default::default(),
                                bias: Default::default()
                            }
                        ),
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &pos_col_uint_shader.get_frag().0,
                            entry_point: &pos_col_uint_shader.get_frag().1,
                            targets: &[
                                wgpu::ColorTargetState {
                                    format: wgpu::TextureFormat::Bgra8Unorm,
                                    blend: Some(BlendState {
                                        color: BlendComponent::OVER,
                                        alpha: BlendComponent::OVER
                                    }),
                                    write_mask: Default::default()
                                }
                            ]
                        }),
                        multiview: None
                    }
                )
            ),
            (
                "clearcolor".into(),
                wm.wgpu_state.device.create_render_pipeline(
                    &wgpu::RenderPipelineDescriptor {
                        label: None,
                        layout: Some(&layouts.get("wgpu_mc_ogl:layouts/clearcolor").unwrap()),
                        vertex: VertexState {
                            module: &clearcolor_shader.get_vert().0,
                            entry_point: &clearcolor_shader.get_vert().1,
                            buffers: &[
                                wgpu::VertexBufferLayout {
                                    array_stride: 20,
                                    step_mode: wgpu::VertexStepMode::Vertex,
                                    attributes: &[
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x2,
                                            offset: 0,
                                            shader_location: 0
                                        },
                                        wgpu::VertexAttribute {
                                            format: wgpu::VertexFormat::Float32x3,
                                            offset: 8,
                                            shader_location: 1
                                        },
                                    ]
                                }
                            ]
                        },
                        primitive: wgpu::PrimitiveState {
                            topology: wgpu::PrimitiveTopology::TriangleList,
                            strip_index_format: None,
                            front_face: wgpu::FrontFace::Ccw,
                            cull_mode: None,
                            unclipped_depth: false,
                            polygon_mode: wgpu::PolygonMode::Fill,
                            conservative: false
                        },
                        depth_stencil: Some(
                            wgpu::DepthStencilState {
                                format: wgpu::TextureFormat::Depth32Float,
                                depth_write_enabled: false,
                                depth_compare: wgpu::CompareFunction::Always,
                                stencil: Default::default(),
                                bias: Default::default()
                            }
                        ),
                        multisample: Default::default(),
                        fragment: Some(wgpu::FragmentState {
                            module: &clearcolor_shader.get_frag().0,
                            entry_point: &clearcolor_shader.get_frag().1,
                            targets: &[
                                wgpu::ColorTargetState {
                                    format: wgpu::TextureFormat::Bgra8Unorm,
                                    blend: None,
                                    write_mask: Default::default()
                                }
                            ]
                        }),
                        multiview: None
                    }
                )
            )
        ].into()
    }

    fn render<'a: 'd, 'b, 'c, 'd: 'c, 'e: 'c + 'd>(&'a self, wm: &'b WmRenderer, render_pass: &'c mut RenderPass<'d>, arena: &'c mut WmArena<'e>) {
        let pipeline_manager = wm.render_pipeline_manager.load();
        let gl_alloc = gl::GL_ALLOC.get().unwrap().read();

        let commands = self.commands.load();

        commands.iter().for_each(|command| {
            match command {
                GLCommand::UsePipeline(pipeline) => {
                    render_pass.set_pipeline(
                        arena.alloc(match pipeline {
                            0 => pipeline_manager.render_pipelines.load().get("pos_col_uint").unwrap().clone(),
                            1 => pipeline_manager.render_pipelines.load().get("pos_tex").unwrap().clone(),
                            2 => pipeline_manager.render_pipelines.load().get("wgpu_mc_ogl:pipelines/pos_col_float3").unwrap().clone(),
                            _ => unimplemented!()
                        })
                    )
                },
                GLCommand::SetVertexBuffer(buf) => {
                    let buffer = wm.wgpu_state.device.create_buffer_init(
                        &BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::cast_slice(&buf),
                            usage: wgpu::BufferUsages::VERTEX
                        }
                    );

                    render_pass.set_vertex_buffer(0, arena.alloc(buffer).slice(..));
                },
                GLCommand::SetIndexBuffer(buf) => {
                    let buffer = wm.wgpu_state.device.create_buffer_init(
                        &BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::cast_slice(&buf),
                            usage: wgpu::BufferUsages::INDEX
                        }
                    );

                    render_pass.set_index_buffer(arena.alloc(buffer).slice(..), wgpu::IndexFormat::Uint32);
                },
                GLCommand::Draw(count) => {
                    render_pass.draw(0..*count, 0..1);
                },
                GLCommand::DrawIndexed(count) => {
                    render_pass.draw_indexed(0..*count, 0, 0..1);
                },
                GLCommand::ClearColor(r, g, b) => {
                    let (r, g, b) = (*r, *g, *b);

                    let vertex_buffer = arena.alloc(
                        wm.wgpu_state.device.create_buffer_init(
                            &BufferInitDescriptor {
                                label: None,
                                contents: bytemuck::cast_slice(&[
                                    -1.0, -1.0, r, g, b,
                                    -1.0, 1.0, r, g, b,
                                    1.0, 1.0, r, g, b,
                                    -1.0, -1.0, r, g, b,
                                    1.0, 1.0, r, g, b,
                                    1.0, -1.0, r, g, b
                                ]),
                                usage: wgpu::BufferUsages::VERTEX
                            }
                        )
                    );

                    render_pass.set_pipeline(
                        arena.alloc(
                            pipeline_manager.render_pipelines.load().get("clearcolor").unwrap().clone()
                        )
                    );

                    render_pass.set_vertex_buffer(0, vertex_buffer.slice(..));
                    render_pass.draw(0..6, 0..1);
                },
                GLCommand::AttachTexture(texture) => {
                    let texture = match gl_alloc.get(texture) {
                        None => self.black_texture.get().unwrap().clone(),
                        Some(tx) => tx.bindable_texture.as_ref().unwrap().clone()
                    };

                    render_pass.set_bind_group(1, &arena.alloc(texture).bind_group, &[]);
                },
                GLCommand::SetMatrix(mat) => {
                    let buffer = arena.alloc(wm.wgpu_state.device.create_buffer_init(
                        &BufferInitDescriptor {
                            label: None,
                            contents: bytemuck::bytes_of(
                                &UniformMatrixHelper { view_proj: (*mat).into() }
                            ),
                            usage: wgpu::BufferUsages::UNIFORM
                        }
                    ));

                    let bg = arena.alloc(wm.wgpu_state.device.create_bind_group(
                        &BindGroupDescriptor {
                            label: None,
                            layout: &pipeline_manager.bind_group_layouts.read().get("matrix4").unwrap(),
                            entries: &[
                                BindGroupEntry {
                                    binding: 0,
                                    resource: buffer.as_entire_binding()
                                }
                            ]
                        }
                    ));

                    render_pass.set_bind_group(0, bg, &[]);
                }
                _ => {}
            };
        });
    }
}