# 图标生成脚本（图标的「源文件」，改设计改这里再重跑）。
#
#   python3 generate.py        # 依赖 Pillow：pip install pillow
#
# 设计：元素周期表的 Fe（铁，26 号）格子 —— ferric 即三价铁。
# 深色底 + 品牌绿描边/字样，颜色取自 crates/ferric-ui/src/theme.rs；
# 字体用仓库内嵌的 Plus Jakarta Sans（与 UI 同族）。
# <64px 的档位换简化母版：去掉原子序数、加粗描边、放大 Fe，保证小图可辨。
#
# 产物（全部提交进仓库，打包与运行时直接引用，不在构建期重新生成）：
#   32x32.png / 128x128.png / 128x128@2x.png / icon.png   cargo-packager（deb/appimage）
#   icon.ico                                              Windows（exe 资源 + NSIS）
#   icon.icns                                             macOS（app bundle / dmg）

import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

HERE = Path(__file__).parent
FONTS = HERE / "../../ferric-ui/assets/fonts"

# 品牌色（theme.rs）
BG_TOP = (0x14, 0x1A, 0x22, 255)  # 深色画布，略亮做渐变顶
BG_BOT = (0x0B, 0x0E, 0x13, 255)  # canvas #0e1116 压深做渐变底
ACCENT = (0x2B, 0xB5, 0x6A, 255)  # dark accent（描边）
ACCENT_HI = (0x39, 0xC1, 0x76, 255)  # accent_strong（Fe 字样）
MUTED = (0x8B, 0x97, 0xA3, 255)  # 原子序数

S = 1024  # 尺寸基准：以下所有长度参数都按 1024 边长给
SS = 4  # 超采样倍率


def rounded_mask(size, radius):
    m = Image.new("L", (size * SS, size * SS), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([0, 0, size * SS - 1, size * SS - 1], radius=radius * SS, fill=255)
    return m.resize((size, size), Image.LANCZOS)


def vgrad(size, top, bot):
    g = Image.new("RGBA", (1, size))
    for y in range(size):
        t = y / (size - 1)
        g.putpixel((0, y), tuple(int(a + (b - a) * t) for a, b in zip(top, bot)))
    return g.resize((size, size))


def draw_master(size, *, keyline_w, fe_scale, with_number):
    k = size / S
    ss = size * SS
    img = vgrad(ss, BG_TOP, BG_BOT)
    d = ImageDraw.Draw(img)

    # 周期表格子：内缩圆角描边
    inset = int(72 * k * SS)
    d.rounded_rectangle(
        [inset, inset, ss - 1 - inset, ss - 1 - inset],
        radius=int(120 * k * SS),
        outline=ACCENT,
        width=max(int(keyline_w * k * SS), SS),
    )

    fe_font = ImageFont.truetype(str(FONTS / "PlusJakartaSans-Bold.ttf"), int(fe_scale * k * SS))
    bb = d.textbbox((0, 0), "Fe", font=fe_font)
    fx = (ss - (bb[2] - bb[0])) // 2 - bb[0]
    # 光学居中：带序数时字形下压一点，给左上的 26 留视觉空间
    fy = (ss - (bb[3] - bb[1])) // 2 - bb[1] + int((0.045 if with_number else 0.0) * ss)
    d.text((fx, fy), "Fe", font=fe_font, fill=ACCENT_HI)

    if with_number:
        num_font = ImageFont.truetype(
            str(FONTS / "PlusJakartaSans-SemiBold.ttf"), int(118 * k * SS)
        )
        d.text((int(150 * k * SS), int(128 * k * SS)), "26", font=num_font, fill=MUTED)

    img = img.resize((size, size), Image.LANCZOS)
    img.putalpha(rounded_mask(size, int(226 * k)))
    return img


def render(size):
    if size < 64:
        return draw_master(size, keyline_w=42, fe_scale=520, with_number=False)
    return draw_master(size, keyline_w=14, fe_scale=430, with_number=True)


def main():
    pngs = {s: render(s) for s in (16, 24, 32, 48, 64, 128, 256, 512, 1024)}
    pngs[32].save(HERE / "32x32.png")
    pngs[128].save(HERE / "128x128.png")
    pngs[256].save(HERE / "128x128@2x.png")
    pngs[512].save(HERE / "icon.png")

    # ICO：逐档塞独立绘制的图，避免 Pillow 从单图缩出糊的 16px
    ico_sizes = [16, 24, 32, 48, 64, 128, 256]
    pngs[256].save(
        HERE / "icon.ico",
        format="ICO",
        sizes=[(s, s) for s in ico_sizes],
        append_images=[pngs[s] for s in ico_sizes if s != 256],
    )

    # ICNS：手写容器，chunk 负载直接用 PNG（macOS 10.7+）
    icns_types = {
        b"icp4": 16, b"icp5": 32, b"ic07": 128, b"ic08": 256, b"ic09": 512,
        b"ic10": 1024, b"ic11": 32, b"ic12": 64, b"ic13": 256, b"ic14": 512,
    }
    chunks = b""
    for typ, s in icns_types.items():
        buf = io.BytesIO()
        pngs[s].save(buf, format="PNG")
        data = buf.getvalue()
        chunks += typ + struct.pack(">I", len(data) + 8) + data
    (HERE / "icon.icns").write_bytes(b"icns" + struct.pack(">I", len(chunks) + 8) + chunks)


if __name__ == "__main__":
    main()
