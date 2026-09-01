//! 标量三角形光栅化 + premultiplied alpha 混合。
//!
//! 这是软渲染后端的心脏：把 egui tessellate 出来的三角形网格逐像素画进内存位图。
//! 为了「界面不变」，混合必须与 egui 的约定一致 —— 顶点颜色与纹理都是 premultiplied
//! alpha，混合公式是 `dst = src + dst * (1 - src.a)`（`ONE, ONE_MINUS_SRC_ALPHA`）。
//!
//! 先求正确、可控，再求快：当前是标准的 barycentric 标量实现，后续可加
//! 「三角形对 → 矩形」和 SIMD 分批混合来提帧率（内存收益不受影响）。

use std::collections::HashMap;

use egui::{ClippedPrimitive, Color32, Mesh, Rect, TextureId, Vec2};

use crate::texture::{swizzle_rgba_bgra, EguiTexture};

pub struct Renderer {
    textures: HashMap<TextureId, EguiTexture>,
}

/// 光栅化用的顶点：坐标已换算成物理像素，颜色已 swizzle 成 BGRA。
#[derive(Clone, Copy)]
struct PxVertex {
    pos: Vec2,
    uv: Vec2,
    color: [u8; 4],
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            textures: HashMap::new(),
        }
    }

    /// 把 egui 这一帧的图元画进 `fb`（BGRX，premultiplied）。
    ///
    /// `fb` 是 `width × height` 个 `[u8; 4]`，由 softbuffer 的 `Buffer` 直接透传。
    pub fn render(
        &mut self,
        fb: &mut [[u8; 4]],
        width: usize,
        height: usize,
        paint_jobs: &[ClippedPrimitive],
        textures_delta: &egui::TexturesDelta,
        pixels_per_point: f32,
    ) {
        self.set_textures(textures_delta);

        for job in paint_jobs {
            let mesh = match &job.primitive {
                egui::epaint::Primitive::Mesh(m) => m,
                // 自定义绘制回调（如插件里的特殊画笔）软渲染不支持，跳过 ——
                // Ferric 自身没有这类回调，插件不装时不会走到这里。
                egui::epaint::Primitive::Callback(_) => continue,
            };
            if mesh.vertices.is_empty() || mesh.indices.is_empty() {
                continue;
            }
            let Some(texture) = self.textures.get(&mesh.texture_id) else {
                continue;
            };
            let clip = Rect {
                min: job.clip_rect.min * pixels_per_point,
                max: job.clip_rect.max * pixels_per_point,
            };
            self.draw_mesh(fb, width, height, clip, mesh, texture, pixels_per_point);
        }

        self.free_textures(textures_delta);
    }

    fn draw_mesh(
        &self,
        fb: &mut [[u8; 4]],
        width: usize,
        height: usize,
        clip: Rect,
        mesh: &Mesh,
        texture: &EguiTexture,
        pixels_per_point: f32,
    ) {
        let verts: Vec<PxVertex> = mesh
            .vertices
            .iter()
            .map(|v| PxVertex {
                pos: v.pos.to_vec2() * pixels_per_point,
                uv: v.uv.to_vec2(),
                color: swizzle_rgba_bgra(v.color.to_array()),
            })
            .collect();

        for tri in mesh.indices.chunks_exact(3) {
            let a = verts[tri[0] as usize];
            let b = verts[tri[1] as usize];
            let c = verts[tri[2] as usize];
            rasterize_tri(fb, width, height, clip, a, b, c, texture);
        }
    }

    fn set_textures(&mut self, textures_delta: &egui::TexturesDelta) {
        for (id, deltas) in &textures_delta.set {
            for delta in deltas {
                let size = delta.image.size();
                let pixels: &[Color32] = match &delta.image {
                    egui::ImageData::Color(image) => &image.pixels,
                };
                if let Some(pos) = delta.pos {
                    // 部分更新：往已有纹理里覆盖一块。
                    if let Some(texture) = self.textures.get_mut(id) {
                        texture.write_region(pos, size, pixels);
                    }
                } else {
                    self.textures
                        .insert(*id, EguiTexture::new(delta.options, size, pixels));
                }
            }
        }
    }

    fn free_textures(&mut self, textures_delta: &egui::TexturesDelta) {
        for id in &textures_delta.free {
            self.textures.remove(id);
        }
    }
}

/// 三角形有向面积的 2 倍。
#[inline(always)]
fn orient2d(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)
}

fn rasterize_tri(
    fb: &mut [[u8; 4]],
    width: usize,
    height: usize,
    clip: Rect,
    a: PxVertex,
    b: PxVertex,
    c: PxVertex,
    texture: &EguiTexture,
) {
    // 统一成逆时针（面积为正），后续重心坐标用 `w >= 0` 判内。
    let area = orient2d(a.pos, b.pos, c.pos);
    if area.abs() < 1e-6 {
        return;
    }
    let (a, b, c, area) = if area < 0.0 {
        (a, c, b, -area)
    } else {
        (a, b, c, area)
    };
    let inv_area = 1.0 / area;

    let min_x = a.pos.x.min(b.pos.x).min(c.pos.x).max(clip.min.x).floor() as i32;
    let max_x = (a.pos.x.max(b.pos.x).max(c.pos.x).min(clip.max.x).ceil() as i32).max(min_x);
    let min_y = a.pos.y.min(b.pos.y).min(c.pos.y).max(clip.min.y).floor() as i32;
    let max_y = (a.pos.y.max(b.pos.y).max(c.pos.y).min(clip.max.y).ceil() as i32).max(min_y);

    let min_x = min_x.max(0);
    let min_y = min_y.max(0);
    let max_x = (max_x as usize).min(width) as i32;
    let max_y = (max_y as usize).min(height) as i32;
    if max_x <= min_x || max_y <= min_y {
        return;
    }

    for y in min_y..max_y {
        let py = y as f32 + 0.5;
        for x in min_x..max_x {
            let px = x as f32 + 0.5;
            let p = Vec2::new(px, py);
            let w0 = orient2d(b.pos, c.pos, p);
            let w1 = orient2d(c.pos, a.pos, p);
            let w2 = orient2d(a.pos, b.pos, p);
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let b0 = w0 * inv_area;
            let b1 = w1 * inv_area;
            let b2 = w2 * inv_area;

            let uv = Vec2::new(
                a.uv.x * b0 + b.uv.x * b1 + c.uv.x * b2,
                a.uv.y * b0 + b.uv.y * b1 + c.uv.y * b2,
            );
            let tex = texture.sample_bilinear(uv);

            // 顶点颜色（premultiplied BGRA）重心插值后与纹理 unorm 相乘。
            let mut src = [0u8; 4];
            for i in 0..4 {
                let vc = a.color[i] as f32 * b0 + b.color[i] as f32 * b1 + c.color[i] as f32 * b2;
                src[i] = unorm_mult(vc.round().clamp(0.0, 255.0) as u32, tex[i] as u32) as u8;
            }

            let dst_idx = (x as usize) + (y as usize) * width;
            fb[dst_idx] = egui_blend_u8(src, fb[dst_idx]);
        }
    }
}

/// Jerry R. Van Aken「无除法 alpha 混合」里的 unorm 乘法：把 0..255 当 0..1 相乘。
#[inline(always)]
fn unorm_mult(a: u32, b: u32) -> u32 {
    let mut a = a * b;
    a += 0x80;
    a += a >> 8;
    a >> 8
}

/// egui 的 premultiplied alpha 混合：`dst = src + dst * (1 - src.a)`。
#[inline(always)]
fn egui_blend_u8(src: [u8; 4], mut dst: [u8; 4]) -> [u8; 4] {
    let a = src[3];
    if a == 255 {
        return src;
    }
    if a != 0 {
        let alpha = a as u64;
        let alpha_compl = 0xFF ^ alpha;
        let dst64 = as_color16(u32::from_le_bytes(dst));

        let res16 = dst64 * alpha_compl + 0x0080008000800080;
        let res8 = res16 + ((res16 >> 8) & 0x00FF00FF00FF00FF);

        let res = (res8 >> 8) & 0x00FF00FF00FF00FF;
        let res = (res | (res >> 8)) & 0x0000FFFF0000FFFF;
        let res = res | (res >> 16);
        dst = u32::to_le_bytes((res & 0x00000000FFFF_FFFF) as u32);
    }

    [
        dst[0].saturating_add(src[0]),
        dst[1].saturating_add(src[1]),
        dst[2].saturating_add(src[2]),
        dst[3].saturating_add(src[3]),
    ]
}

/// 把 4 字节 RGBA 展开成 8 字节 0R0G0B0A，供 64 位混合一次性算完。
#[inline(always)]
fn as_color16(color: u32) -> u64 {
    let x = color as u64;
    let x = ((x & 0xFFFF_0000) << 16) | (x & 0xFFFF);
    ((x & 0x0000_FF00_0000_FF00) << 8) | (x & 0x0000_00FF_0000_00FF)
}
