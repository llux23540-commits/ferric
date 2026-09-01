//! 纹理：持有 egui 上传的像素，提供双线性采样。
//!
//! 采样逻辑与 `egui_software_backend` 对齐：像素在入库时就 swizzle 成 BGRA，
//! 这样光栅化阶段算出的颜色可以直接写进 softbuffer 的 BGRX 帧缓冲，无需二次转换。

use egui::{vec2, Color32, TextureFilter, TextureOptions, Vec2};

pub struct EguiTexture {
    /// 已 swizzle 成 BGRA 的像素（premultiplied alpha），行优先。
    data: Vec<[u8; 4]>,
    /// uv == (0, 0) 时的采样值：egui 的默认纹理左上角恒为全白，这是字体图集外采样的兜底。
    uv_zero_val: [u8; 4],
    width_extent: i32,
    height_extent: i32,
    width: usize,
    fsize: Vec2,
    options: TextureOptions,
}

impl EguiTexture {
    pub fn new(options: TextureOptions, size: [usize; 2], pixels: &[Color32]) -> Self {
        let data = pixels
            .iter()
            .map(|p| swizzle_rgba_bgra(p.to_array()))
            .collect::<Vec<_>>();
        // 空纹理退化成一像素白，避免 `data[0]` 越界 —— egui 不会上传空纹理，防御而已。
        let uv_zero_val = data.first().copied().unwrap_or([255; 4]);
        Self {
            data,
            width_extent: size[0] as i32 - 1,
            height_extent: size[1] as i32 - 1,
            width: size[0],
            fsize: vec2(size[0] as f32, size[1] as f32),
            options,
            uv_zero_val,
        }
    }

    /// 写入一个子区域（`ImageDelta::partial`）。软渲染后端自己维护这块纹理，
    /// 因此直接按坐标覆盖即可。
    pub fn write_region(&mut self, pos: [usize; 2], size: [usize; 2], pixels: &[Color32]) {
        for y in 0..size[1] {
            for x in 0..size[0] {
                let src = swizzle_rgba_bgra(pixels[x + y * size[0]].to_array());
                let dst = (x + pos[0]) + (y + pos[1]) * self.width;
                if dst < self.data.len() {
                    self.data[dst] = src;
                }
            }
        }
    }

    #[inline]
    pub fn sample_bilinear(&self, uv: Vec2) -> [u8; 4] {
        if uv == Vec2::ZERO {
            return self.uv_zero_val;
        }

        let w = self.fsize.x;
        let h = self.fsize.y;

        #[inline(always)]
        fn mirror(v: f32) -> f32 {
            ((v * 0.5 + 0.5).fract() - 0.5) * 2.0
        }

        let uv = match self.options.wrap_mode {
            egui::TextureWrapMode::ClampToEdge => uv,
            egui::TextureWrapMode::Repeat => vec2(uv.x.fract(), uv.y.fract()),
            egui::TextureWrapMode::MirroredRepeat => vec2(mirror(uv.x), mirror(uv.y)),
        };

        let sx = uv.x * w - 0.5;
        let sy = uv.y * h - 0.5;

        let x0 = sx.floor() as i32;
        let y0 = sy.floor() as i32;
        let x1 = x0 + 1;
        let y1 = y0 + 1;

        let fx = sx - x0 as f32;
        let fy = sy - y0 as f32;

        let x0c = x0.max(0).min(self.width_extent);
        let y0c = y0.max(0).min(self.height_extent);
        let x1c = x1.max(0).min(self.width_extent);
        let y1c = y1.max(0).min(self.height_extent);

        let c00 = self.data[(x0c as usize) + (y0c as usize) * self.width];

        if self.options.magnification == TextureFilter::Nearest || (fx == 0.0 && fy == 0.0) {
            return c00;
        }

        let c10 = self.data[(x1c as usize) + (y0c as usize) * self.width];
        let c01 = self.data[(x0c as usize) + (y1c as usize) * self.width];
        let c11 = self.data[(x1c as usize) + (y1c as usize) * self.width];

        let w00 = (1.0 - fx) * (1.0 - fy);
        let w10 = fx * (1.0 - fy);
        let w01 = (1.0 - fx) * fy;
        let w11 = fx * fy;

        let mut out = [0u8; 4];
        for (i, o) in out.iter_mut().enumerate() {
            let v = c00[i] as f32 * w00
                + c01[i] as f32 * w01
                + c10[i] as f32 * w10
                + c11[i] as f32 * w11;
            *o = (v + 0.5) as u8;
        }
        out
    }
}

/// RGBA → BGRA（premultiplied 值不变，只换通道位置）。
#[inline(always)]
pub fn swizzle_rgba_bgra(a: [u8; 4]) -> [u8; 4] {
    [a[2], a[1], a[0], a[3]]
}
