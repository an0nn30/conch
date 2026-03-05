use iced::widget::shader;
use iced::wgpu;
use iced::wgpu::util::DeviceExt;
use iced::Rectangle;

use super::glyph_cache::{CachedGlyph, GlyphKey};
use super::pipeline::{RectVertex, TerminalPipeline, TextVertex, Uniforms};
use crate::size_info::SizeInfo;

/// A renderable cell extracted from alacritty_terminal.
#[derive(Debug, Clone)]
pub struct RenderCell {
    pub col: usize,
    pub row: usize,
    pub c: char,
    pub fg: [f32; 4],
    pub bg: [f32; 4],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
}

/// Cursor info for rendering.
#[derive(Debug, Clone)]
pub struct RenderCursor {
    pub col: usize,
    pub row: usize,
    pub color: [f32; 4],
    pub visible: bool,
}

/// Snapshot of terminal content for one frame.
#[derive(Debug)]
pub struct TerminalPrimitive {
    pub cells: Vec<RenderCell>,
    pub cursor: Option<RenderCursor>,
    pub size_info: SizeInfo,
    pub bg_color: [f32; 4],
}

impl shader::Primitive for TerminalPrimitive {
    type Pipeline = TerminalPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        let viewport_w = bounds.width;
        let viewport_h = bounds.height;

        // Update uniforms
        queue.write_buffer(
            &pipeline.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport_size: [viewport_w, viewport_h],
            }),
        );

        let si = &self.size_info;

        // --- Build rect vertices (backgrounds) ---
        let mut rect_verts: Vec<RectVertex> = Vec::with_capacity(self.cells.len() * 6 + 6);

        // Full terminal background rect
        push_quad(&mut rect_verts, 0.0, 0.0, viewport_w, viewport_h, self.bg_color);

        for cell in &self.cells {
            if cell.bg != self.bg_color {
                let (x, y) = si.cell_position(cell.col, cell.row);
                let w = si.cell_width;
                let h = si.cell_height;
                push_quad(&mut rect_verts, x, y, w, h, cell.bg);
            }
        }

        // Cursor
        if let Some(cursor) = &self.cursor {
            if cursor.visible {
                let (x, y) = si.cell_position(cursor.col, cursor.row);
                push_quad(
                    &mut rect_verts,
                    x,
                    y,
                    si.cell_width,
                    si.cell_height,
                    cursor.color,
                );
            }
        }

        // Underlines & strikethrough
        for cell in &self.cells {
            let (x, y) = si.cell_position(cell.col, cell.row);
            if cell.underline {
                let uy = y + si.cell_height - 2.0;
                push_quad(&mut rect_verts, x, uy, si.cell_width, 1.0, cell.fg);
            }
            if cell.strikeout {
                let sy = y + si.cell_height * 0.5;
                push_quad(&mut rect_verts, x, sy, si.cell_width, 1.0, cell.fg);
            }
        }

        // Upload rect vertices
        if !rect_verts.is_empty() {
            pipeline.rect_vertex_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("rect_vertices"),
                    contents: bytemuck::cast_slice(&rect_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
        } else {
            pipeline.rect_vertex_buffer = None;
        }
        pipeline.rect_vertex_count = rect_verts.len() as u32;

        // --- Build text vertices (glyphs) ---
        let mut text_verts: Vec<TextVertex> = Vec::with_capacity(self.cells.len() * 6);

        for cell in &self.cells {
            if cell.c == ' ' || cell.c == '\0' {
                continue;
            }

            let glyph_key = GlyphKey {
                c: cell.c,
                bold: cell.bold,
                italic: cell.italic,
            };

            // Check cache
            let cached = if let Some(cached) = pipeline.glyph_cache.get(&glyph_key) {
                *cached
            } else {
                // Rasterize and cache
                let rasterized =
                    pipeline
                        .font_context
                        .rasterize_char(cell.c, cell.bold, cell.italic);

                let cached = if let Some(glyph) = rasterized {
                    if glyph.width > 0 && glyph.height > 0 {
                        if let Some(region) = pipeline.atlas.allocate(glyph.width, glyph.height) {
                            // Upload to atlas texture
                            queue.write_texture(
                                wgpu::TexelCopyTextureInfo {
                                    texture: &pipeline.atlas_texture,
                                    mip_level: 0,
                                    origin: wgpu::Origin3d {
                                        x: region.x,
                                        y: region.y,
                                        z: 0,
                                    },
                                    aspect: wgpu::TextureAspect::All,
                                },
                                &glyph.data,
                                wgpu::TexelCopyBufferLayout {
                                    offset: 0,
                                    bytes_per_row: Some(glyph.width * 4),
                                    rows_per_image: None,
                                },
                                wgpu::Extent3d {
                                    width: glyph.width,
                                    height: glyph.height,
                                    depth_or_array_layers: 1,
                                },
                            );

                            let (uv_x, uv_y, uv_w, uv_h) =
                                region.uv(pipeline.atlas.width, pipeline.atlas.height);

                            Some(CachedGlyph {
                                uv_x,
                                uv_y,
                                uv_w,
                                uv_h,
                                width: glyph.width,
                                height: glyph.height,
                                left: glyph.left,
                                top: glyph.top,
                                is_color: glyph.is_color,
                            })
                        } else {
                            None // Atlas full
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

                pipeline.glyph_cache.insert(glyph_key, cached);
                cached
            };

            if let Some(glyph) = cached {
                let (cx, cy) = si.cell_position(cell.col, cell.row);
                let gx = cx + glyph.left as f32;
                let gy = cy + (si.cell_height - glyph.top as f32);
                let gw = glyph.width as f32;
                let gh = glyph.height as f32;

                let fg = cell.fg;
                let is_color = if glyph.is_color { 1.0 } else { 0.0 };

                let u0 = glyph.uv_x;
                let v0 = glyph.uv_y;
                let u1 = glyph.uv_x + glyph.uv_w;
                let v1 = glyph.uv_y + glyph.uv_h;

                // Triangle 1: TL, TR, BL
                text_verts.push(TextVertex {
                    position: [gx, gy],
                    uv: [u0, v0],
                    fg_color: fg,
                    is_color,
                });
                text_verts.push(TextVertex {
                    position: [gx + gw, gy],
                    uv: [u1, v0],
                    fg_color: fg,
                    is_color,
                });
                text_verts.push(TextVertex {
                    position: [gx, gy + gh],
                    uv: [u0, v1],
                    fg_color: fg,
                    is_color,
                });
                // Triangle 2: TR, BR, BL
                text_verts.push(TextVertex {
                    position: [gx + gw, gy],
                    uv: [u1, v0],
                    fg_color: fg,
                    is_color,
                });
                text_verts.push(TextVertex {
                    position: [gx + gw, gy + gh],
                    uv: [u1, v1],
                    fg_color: fg,
                    is_color,
                });
                text_verts.push(TextVertex {
                    position: [gx, gy + gh],
                    uv: [u0, v1],
                    fg_color: fg,
                    is_color,
                });
            }
        }

        // Upload text vertices
        if !text_verts.is_empty() {
            pipeline.text_vertex_buffer = Some(device.create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: Some("text_vertices"),
                    contents: bytemuck::cast_slice(&text_verts),
                    usage: wgpu::BufferUsages::VERTEX,
                },
            ));
        } else {
            pipeline.text_vertex_buffer = None;
        }
        pipeline.text_vertex_count = text_verts.len() as u32;

        // --- Create bind groups ---
        pipeline.rect_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("rect_bind_group"),
            layout: &pipeline.rect_bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: pipeline.uniform_buffer.as_entire_binding(),
            }],
        }));

        pipeline.text_bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("text_bind_group"),
            layout: &pipeline.text_bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: pipeline.uniform_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&pipeline.atlas_texture_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&pipeline.atlas_sampler),
                },
            ],
        }));
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("terminal_render_pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
                depth_slice: None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );

        // Pass 1: Background rects
        if let (Some(bind_group), Some(vertex_buf)) =
            (&pipeline.rect_bind_group, &pipeline.rect_vertex_buffer)
        {
            pass.set_pipeline(&pipeline.rect_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buf.slice(..));
            pass.draw(0..pipeline.rect_vertex_count, 0..1);
        }

        // Pass 2: Text glyphs
        if let (Some(bind_group), Some(vertex_buf)) =
            (&pipeline.text_bind_group, &pipeline.text_vertex_buffer)
        {
            pass.set_pipeline(&pipeline.text_pipeline);
            pass.set_bind_group(0, bind_group, &[]);
            pass.set_vertex_buffer(0, vertex_buf.slice(..));
            pass.draw(0..pipeline.text_vertex_count, 0..1);
        }
    }
}

/// Push 6 vertices (2 triangles) for a quad.
fn push_quad(verts: &mut Vec<RectVertex>, x: f32, y: f32, w: f32, h: f32, color: [f32; 4]) {
    verts.push(RectVertex { position: [x, y], color });
    verts.push(RectVertex { position: [x + w, y], color });
    verts.push(RectVertex { position: [x, y + h], color });
    verts.push(RectVertex { position: [x + w, y], color });
    verts.push(RectVertex { position: [x + w, y + h], color });
    verts.push(RectVertex { position: [x, y + h], color });
}
