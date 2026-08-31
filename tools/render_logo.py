#!/usr/bin/env python
"""FluxReader logo 栅格化器 —— logo.svg 的纯 Python 像素级实现。

几何与渐变与 SVG 定义一一对应：
  512 画布，rx=112 圆角矩形底 + 45% 蓝色环境辉光 + 三条磨砂玻璃横条 + 橙色圆点。
输出带 Alpha 的 RGBA PNG（源 SVG 无透明背景需求，底即不透明，alpha 恒 255，
但保留 RGBA 模式便于未来透明版）。
"""
import math
from PIL import Image, ImageDraw

# ---- 渐变采样（SVG 定义） ----

def lerp(a, b, t):
    return tuple(a[i] + (b[i] - a[i]) * t for i in range(len(a)))

def hexc(s):
    s = s.lstrip('#')
    return (int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16))

BG_STOPS = [(0.0, hexc('#050b17')), (0.5, hexc('#0a1936')), (1.0, hexc('#040a18'))]
GLOW_STOPS = [(0.0, (14, 165, 233, 0.45)), (0.5, (59, 130, 246, 0.2)), (1.0, (10, 25, 54, 0.0))]

def sample_stops(stops, t):
    t = min(1.0, max(0.0, t))
    for i in range(len(stops) - 1):
        o0, c0 = stops[i]
        o1, c1 = stops[i + 1]
        if t <= o1 or i == len(stops) - 2:
            span = o1 - o0
            lt = 0.0 if span <= 0 else (t - o0) / span
            return lerp(c0, c1, min(1.0, max(0.0, lt)))
    return stops[-1][1]

def in_rounded_rect(x, y, rect, rx):
    x0, y0, x1, y1 = rect
    if x < x0 or x > x1 or y < y0 or y > y1:
        return False
    # 四角圆角判定
    for cx, cy in ((x0 + rx, y0 + rx), (x1 - rx, y0 + rx), (x0 + rx, y1 - rx), (x1 - rx, y1 - rx)):
        # 判断点是否落在该角的外部方区
        in_corner_x = (x < cx - rx if cx == x0 + rx else x > cx)
        # 更简单：仅当点位于四角 9 宫格外圈角区时才需圆判定
    # 直接圆判定：找最近的角圆心
    cxs = [x0 + rx, x1 - rx]
    cys = [y0 + rx, y1 - rx]
    if (x0 + rx) <= x <= (x1 - rx) or (y0 + rx) <= y <= (y1 - rx):
        return True
    # 角区：找最近角圆心
    cx = cxs[0] if x < cxs[0] else cxs[1]
    cy = cys[0] if y < cys[0] else cys[1]
    return (x - cx) ** 2 + (y - cy) ** 2 <= rx * rx

def rounded_rect_alpha(x, y, rect, rx):
    """像素级抗锯齿覆盖度 0..1（对 1px 边界做 4 采样平均）"""
    total = 0
    for dx, dy in ((0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)):
        if in_rounded_rect(x + dx - 0.5 + 0.5, y + dy - 0.5 + 0.5, rect, rx):
            total += 0.25
    return total

def circle_alpha(x, y, cx, cy, r):
    total = 0
    for dx, dy in ((0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)):
        px_, py_ = x + dx - 0.5, y + dy - 0.5
        if (px_ - cx) ** 2 + (py_ - cy) ** 2 <= r * r:
            total += 0.25
    return total

def build(size):
    S = 512.0
    sc = size / S
    im = Image.new('RGBA', (size, size), (0, 0, 0, 0))
    px = im.load()
    # 局部坐标 -> SVG 坐标
    diag = math.hypot(S, S)
    cx_g, cy_g, r_g = 0.45 * S, 0.50 * S, 0.55 * S
    bars = [
        (106, 146, 300, 48),   # 顶条
        (106, 232, 186, 48),   # 中条
        (106, 318, 112, 48),   # 底条
    ]
    dot_c = (354, 256)
    halo_r = 42.0
    dot_r = 28.0
    for y in range(size):
        for x in range(size):
            sx, sy = (x + 0.5) / sc, (y + 0.5) / sc
            # 底圆角矩形：rx=112（占满 512 画布）
            base_cov = rounded_rect_alpha(sx - 0.5, sy - 0.5, (0, 0, S, S), 112)
            if base_cov <= 0:
                continue
            # 底色：对角线性渐变
            gt = (sx + sy) / (2 * S)
            c = sample_stops(BG_STOPS, gt)
            r_, g_, b_ = c
            # 辉光：径向，45%,50% r=55%（SVG radialGradient 默认 userSpaceOnUse 的近似：以中心百分比计）
            dx, dy = sx - cx_g, sy - cy_g
            dist = math.hypot(dx, dy)
            gk = min(1.0, dist / r_g)
            gc = sample_stops(GLOW_STOPS, gk)
            gr, gg, gb, ga = int(gc[0]), int(gc[1]), int(gc[2]), gc[3] * 255
            # over 合成：glow 半透明叠在 bg 上
            a = ga / 255.0
            r_ = int(round(gr * a + r_ * (1 - a)))
            g_ = int(round(gg * a + g_ * (1 - a)))
            b_ = int(round(gb * a + b_ * (1 - a)))
            # 玻璃条：fill 白色 0.28→0.05 对角渐变 + stroke 白色系渐变（1.5px，向内外各半）
            covered = False
            for (bx, by, bw, bh) in bars:
                rx_bar = 24.0
                outer = (bx - 0.75, by - 0.75, bx + bw + 0.75, by + bh + 0.75)
                cov_out = rounded_rect_alpha(sx - 0.5, sy - 0.5, outer, rx_bar + 0.75)
                if cov_out <= 0:
                    continue
                inner = (bx + 0.75, by + 0.75, bx + bw - 0.75, by + bh - 0.75)
                cov_in = rounded_rect_alpha(sx - 0.5, sy - 0.5, inner, rx_bar - 0.75)
                if cov_in > 0:
                    # fill：白色对角渐变 0.28 → 0.05（以条内局部对角比例）
                    lt = (sx + sy) % 512 / 512.0  # 近似全局对角
                    # 更准确：以整幅画布对角比例
                    lt = (sx + sy) / (2 * S)
                    fa = 0.28 + (0.05 - 0.28) * lt
                    a_ = cov_in * fa
                    r_ = int(round(255 * a_ + r_ * (1 - a_)))
                    g_ = int(round(255 * a_ + g_ * (1 - a_)))
                    b_ = int(round(255 * a_ + b_ * (1 - a_)))
                # stroke（fill 之上，覆盖 fill 区）：cov_out - cov_in 即描边带
                scov = cov_out - cov_in
                if scov > 0:
                    st0, st40, st100 = (255, 255, 255, 0.8), (147, 197, 253, 0.4), (255, 255, 255, 0.1)
                    lt = (sx + sy) / (2 * S)
                    if lt <= 0.4:
                        stc = lerp(st0, st40, lt / 0.4)
                    else:
                        stc = lerp(st40, st100, (lt - 0.4) / 0.6)
                    a_ = scov * stc[3]
                    r_ = int(round(stc[0] * a_ + r_ * (1 - a_)))
                    g_ = int(round(stc[1] * a_ + g_ * (1 - a_)))
                    b_ = int(round(stc[2] * a_ + b_ * (1 - a_)))
            # 橙点：halo（#f97316 25%）→ dot（径向 #ffedd5→#f97316→#ea580c + #ffedd5 0.8 stroke 2px）
            halo_cov = circle_alpha(sx - 0.5, sy - 0.5, dot_c[0], dot_c[1], halo_r)
            if halo_cov > 0:
                a_ = halo_cov * 0.25
                r_ = int(round(249 * a_ + r_ * (1 - a_)))
                g_ = int(round(115 * a_ + g_ * (1 - a_)))
                b_ = int(round(22 * a_ + b_ * (1 - a_)))
            dot_cov = circle_alpha(sx - 0.5, sy - 0.5, dot_c[0], dot_c[1], dot_r)
            if dot_cov > 0:
                dxn, dyn = sx - dot_c[0], sy - dot_c[1]
                dd = math.hypot(dxn, dyn)
                kt = dd / dot_r
                # radial 35%,35% r=65%（相对点 bounding box）：亮点在左上
                lx = (dxn / dot_r + 1) / 2
                ly = (dyn / dot_r + 1) / 2
                rad_t = min(1.0, math.hypot(lx - 0.35, ly - 0.35) / 0.65)
                if rad_t <= 0.4:
                    dc = lerp(hexc('#ffedd5'), hexc('#f97316'), rad_t / 0.4)
                else:
                    dc = lerp(hexc('#f97316'), hexc('#ea580c'), (rad_t - 0.4) / 0.6)
                a_ = dot_cov
                r_ = int(round(dc[0] * a_ + r_ * (1 - a_)))
                g_ = int(round(dc[1] * a_ + g_ * (1 - a_)))
                b_ = int(round(dc[2] * a_ + b_ * (1 - a_)))
            # dot stroke：r=28 外扩 1px（stroke-width 2）
            ring_out = circle_alpha(sx - 0.5, sy - 0.5, dot_c[0], dot_c[1], dot_r + 1)
            ring_in = circle_alpha(sx - 0.5, sy - 0.5, dot_c[0], dot_c[1], dot_r - 1)
            rc = ring_out - ring_in
            if rc > 0:
                a_ = rc * 0.8
                fc = hexc('#ffedd5')
                r_ = int(round(fc[0] * a_ + r_ * (1 - a_)))
                g_ = int(round(fc[1] * a_ + g_ * (1 - a_)))
                b_ = int(round(fc[2] * a_ + b_ * (1 - a_)))
            px[x, y] = (r_, g_, b_, 255 if base_cov >= 1 else int(base_cov * 255))
    return im

if __name__ == '__main__':
    import sys, os
    outdir = sys.argv[1] if len(sys.argv) > 1 else '.'
    for sz in (32, 64, 128, 256, 512):
        im = build(sz)
        im.save(os.path.join(outdir, f'logo-{sz}.png'))
        print('saved', sz)
