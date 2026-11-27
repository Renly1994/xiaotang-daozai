// glass.hlsl - D3D11 port of dao zai's liquid-glass WGSL (gpu_glass.rs).
// Full port of dao zai's liquid-glass WGSL: rounded-rect SDF, notch SDF,
// 5-tap chromatic dispersion, Snell-style refraction (-g*k*strength),
// directional Schlick-like rim highlight, bevel dark band, light bleed,
// Beer-Lambert absorption, dither, plus per-control inner glass lenses.

#define MAX_CTL 13

cbuffer U : register(b0) {
    float4 bounds;          // xy = top-left (device px), zw = size (device px)
    float4 params;          // x=chroma, y=unused, z=morph, w=material(0..1)
    float4 extra;           // x=radius, y=texW, z=texH, w=dock(0=top,1=left,2=right)
    float4 tint;            // rgb tint, a=strength
    float4 quad;            // unused (fullscreen triangle)
    float4 misc;            // xy=pointer(px), z=refract strength, w=flat mode(0/1/2)
    float4 crect[MAX_CTL];  // (x, y, w, h) per control, capture-device px
    float4 cprm[MAX_CTL];   // (radius, enabled, magnify, refract) per control
    float4 cstyle[MAX_CTL]; // (style, -, -, -) per control: 0=普通 1=分栏容器(磨砂) 2=分栏指示器(亮)
    float4 look;            // x=blur, y=band, z=edge width, w=edge opacity
    float4 notch;           // x=flare, y=fh, z=isNotch, w=isDComp
    float4 lightcfg;        // x=follow strength
    float4 meta;            // 融球水滴: xy=中心(capture px, 顶原点), zw=半宽半高
    float4 meta2;           // x=smooth-min 半径 k, y=锚点 y(capture px), z=开关(0=关), w=保留
};

Texture2D<float4> bg : register(t0);
SamplerState samp : register(s0);

struct VSOut {
    float4 pos : SV_POSITION;
    float2 uv  : TEXCOORD0;
};

VSOut vs(uint id : SV_VertexID) {
    // Fullscreen triangle: no vertex buffer / input layout needed.
    float2 xy[3] = { float2(-1,-1), float2(3,-1), float2(-1,3) };
    float2 uv[3] = { float2(0,0),  float2(2,0),  float2(0,2)  };
    VSOut o;
    o.pos = float4(xy[id], 0.0, 1.0);
    o.uv  = uv[id];
    return o;
}

float rr_sdf(float2 p, float2 b, float r) {
    float2 q = abs(p) - b + r;
    return min(max(q.x, q.y), 0.0) + length(max(q, float2(0,0))) - r;
}

// 梯度感知 smooth-union (Apple QuartzCore sdf_union / IQ 多项式 smin)。
// a=vec3(d, gx, gy), b 同理。k 越大颈部越粗。返回一个新的 (d, g)。
float3 sdf_union3(float3 a, float3 b, float k) {
    float2 ga = normalize(a.yz + float2(1e-6, 1e-6));
    float2 gb = normalize(b.yz + float2(1e-6, 1e-6));
    float kEff = k * saturate(0.5 - 0.5 * dot(ga, gb)) + 1e-4;
    float h = saturate(0.5 + 0.5 * (b.x - a.x) / kEff);
    float d = lerp(b.x, a.x, h) - kEff * h * (1.0 - h);
    float2 g = normalize(lerp(b.yz, a.yz, h) + float2(1e-6, 1e-6));
    return float3(d, g);
}

// Notch (Liu Hai) outline SDF: shoulders flare outward near the top, matching
// shape::sections. Bottom corners rounded, top edge straight where shoulders extend.
// p 是 D3D11 局部坐标：+b.y 是岛体顶、-b.y 是底（bounds 已做 h-by-bh 翻转）。
// egui::sections 肩部 t=0 在顶、wing=flare*(1-t)^2.4 顶部最宽——t=0 也必须在顶。
float notch_sdf(float2 p, float2 b, float r, float flare, float fh) {
    float t = saturate((b.y - p.y) / max(fh, 1.0));
    float wing = flare * pow(1.0 - t, 2.4);
    float hx = b.x + wing;
    // 圆角在底部（-b.y 侧），不是顶部。迁移前 bounds 是顶原点、写 p.y > b.y-r；
    // 迁移后 bounds 做了 h-by-bh 翻转，底原点系里底部是 p.y=-b.y，故改这里。
    if (p.y < -(b.y - r)) {
        float qx = max(abs(p.x) - (b.x - r), 0.0);
        // qy = 底部圆角圆心(p.y=-(b.y-r)) 到当前点的距离，必须是减号；
        // 写成 abs(p.y)+(b.y-r) 会让底部整条 SDF 变正 → 判成岛体外 → 玻璃铺不满。
        float qy = -(p.y) - (b.y - r);
        return length(float2(qx, max(qy, 0.0))) - r;
    }
    return max(abs(p.x) - hx, abs(p.y) - b.y);
}

// 岛体 SDF：把屏幕局部坐标按停靠方向旋到「贴顶坐标系」再算形状。
// extra.w = dock（0=Top 不旋转；1=Left 肩部朝屏幕左；2=Right 肩部朝屏幕右）。
// 左右停靠时岛体 rect 是竖向窄条（36 x 长），half_size 需交换宽高，局部坐标也旋转 90°。
float island_sdf(float2 local, float2 b, float r) {
    if (extra.w < 0.5) {
        return (notch.z > 0.5)
            ? notch_sdf(local, b, r, notch.x, notch.y)
            : rr_sdf(local, b, r);
    }
    // 映射经完整链路数值验证（egui y 顶原点 → bounds 的 h-y 翻转 → shader local → q）：
    // Left  贴左边缘 local.x=-b.x → q=(local.y, -local.x) → q.y=+b.y → 肩部落在贴边侧 ✓
    // Right 贴右边缘 local.x=+b.x → q=(-local.y, local.x) → q.y=+b.y → 肩部落在贴边侧 ✓
    float2 q;
    float2 hb = b.yx;
    if (extra.w < 1.5) {
        q = float2(local.y, -local.x);   // Left
    } else {
        q = float2(-local.y, local.x);   // Right
    }
    return (notch.z > 0.5)
        ? notch_sdf(q, hb, r, notch.x, notch.y)
        : rr_sdf(q, hb, r);
}

float hash21(float2 p) {
    float2 q = frac(p * float2(0.1031, 0.1030));
    q += dot(q, q.yx + float2(33.33, 33.33));
    return frac((q.x + q.y) * q.x);
}

float3 sample_px(float2 px) {
    if (misc.w > 1.5) {
        // Anti-aliased grid (FLAT=2): makes refraction/dispersion/overflow visible.
        float2 q = px / 20.0;
        float dl = min(abs(frac(q.x) - 0.5), abs(frac(q.y) - 0.5)) * 20.0;
        float ln = 1.0 - smoothstep(0.6, 1.8, dl);
        return lerp(float3(0.80,0.84,0.90), float3(0.18,0.22,0.30), ln);
    }
    // px is window-local physical pixels, but y counts from the BOTTOM (D3D11 fullscreen
    // triangle: uv.y=0 at bottom). Flip it against window height (extra.z) so it counts
    // from the top, matching the desktop/window origin in quad.yz.
    // Without this flip the capture is mirrored vertically (island at top -> sample at bottom).
    uint w, h;
    bg.GetDimensions(w, h);
    float2 screen_px = float2(px.x + quad.y, (extra.z - px.y) + quad.z);
    float2 uv = saturate(screen_px / float2(w, h));
    // WGC frames are top-down; D3D11 SRV samples top-left origin -> flip Y.
    // quad.x is the flip flag (set when the live WGC path is active).
    if (quad.x > 0.5) uv.y = 1.0 - uv.y;
    // 采样 BGRA 背景纹理：D3D11 SRV 已把 B8G8R8A8 映射成正确的 RGBA 语义（.r=红），
    // 直接取 .rgb 即可。切勿照搬独立工程的 .bgr 交换——那套 pipeline 手动上传 BGRA
    // 字节才需要交换；WGC CopyResource 路径再交换会把 RGB 翻一次，暖色壁纸变青蓝。
    return bg.SampleLevel(samp, uv, 0.0).rgb;
}

// Per-channel 5-point cross average: R samples +ca side, B samples -ca side.
float3 tap5(float2 px, float2 ca, float blur) {
    float2 off[5] = {
        float2(0,0), float2(blur,0), float2(-blur,0), float2(0,blur), float2(0,-blur)
    };
    float3 s = 0;
    for (int i = 0; i < 5; i++) {
        float2 q = px + off[i];
        s += float3(sample_px(q + ca).r, sample_px(q).g, sample_px(q - ca).b);
    }
    return s / 5.0;
}

// 高斯模糊采样（容器磨砂玻璃用）：中心 + 内环4点 + 外环8点，共13点，
// 高斯权重 exp(-r²/2σ²)。radius 为模糊半径(device px)。
float3 gauss_blur(float2 px, float radius) {
    float sigma = max(radius * 0.5, 1.0);
    float w2 = exp(-1.0 / (2.0 * sigma * sigma));
    float w4 = exp(-2.0 / (2.0 * sigma * sigma));
    float3 s = sample_px(px);                                  // 中心 w=1
    float2 n[4] = { float2(1,0), float2(-1,0), float2(0,1), float2(0,-1) };
    float2 d[4] = { float2(1,1), float2(-1,1), float2(1,-1), float2(-1,-1) };
    for (int i = 0; i < 4; i++) {
        s += sample_px(px + n[i] * radius) * w2;              // 内环 (距离 r)
        s += sample_px(px + d[i] * radius * 0.707) * w2;      // 对角 (距离 r)
        s += sample_px(px + n[i] * radius * 2.0) * w4;        // 外环 (距离 2r)
    }
    float wsum = 1.0 + 8.0 * w2 + 4.0 * w4;
    return s / wsum;
}

// Control hit test: later control overwrites earlier (volume overlay over lyrics).
// Returns idx = MAX_CTL when nothing hit.
struct CtlSel { float d; float2 hs; float r; int idx; float mag; float refr; };
CtlSel pick_ctl(float2 p) {
    CtlSel outc = { 0.0, float2(1,1), 0.0, MAX_CTL, 1.0, 1.0 };
    for (int ci = 0; ci < MAX_CTL; ci++) {
        if (cprm[ci].y < 0.5) continue;
        float4 cr = crect[ci];
        float2 hs = max(cr.zw * 0.5 - float2(0.5,0.5), float2(1,1));
        float rr = max(min(cprm[ci].x - 0.5, min(hs.x, hs.y)), 0.0);
        float dd = rr_sdf(p - cr.xy - cr.zw * 0.5, hs, rr);
        if (dd < 0.0) {
            outc.d = dd; outc.hs = hs; outc.r = rr;
            outc.idx = ci; outc.mag = cprm[ci].z; outc.refr = cprm[ci].w;
        }
    }
    return outc;
}

// Small lens drawn for one control (same material as island body, tighter band).
float4 ctrl_glass(float2 p, float2 local, float2 hs, float radius, float d,
                  float mat, float2 Ld, float3 edge_bg, float mag, float refr) {
    float e = 1.0;
    float gx = rr_sdf(local + float2(e,0), hs, radius) - rr_sdf(local - float2(e,0), hs, radius);
    float gy = rr_sdf(local + float2(0,e), hs, radius) - rr_sdf(local - float2(0,e), hs, radius);
    float2 g = normalize(float2(gx,gy) + float2(1e-6,1e-6));

    float short_side = max(min(hs.x, hs.y), 1.0);
    float band = clamp(short_side * 1.30, 10.0, 72.0);
    float t = saturate(-d / band);
    float k = pow(1.0 - pow(t, 0.30), 2.0);
    float strength = max(misc.z, 0.0) * 0.55 * max(refr, 0.05);
    float cxn = saturate((abs(local.x) - (hs.x - radius)) / max(radius,1));
    float cyn = saturate((abs(local.y) - (hs.y - radius)) / max(radius,1));
    float chroma = max(params.x,0.0) * 0.55 * max(refr,0.05) * lerp(0.60,1.0,cxn*cyn);
    float m = max(mag, 1.0);
    float2 q = p - local * (1.0 - 1.0 / m);
    float2 bp = q - g * k * strength;

    float3 col = tap5(bp, -g * k * chroma, look.x * 0.80 + k * 1.2);
    // Material compensation identical to the island body.
    float lum_c = dot(col, float3(0.299,0.587,0.114));
    col = col * (1.0 + lerp(0.045,-0.035,smoothstep(0.42,0.88,lum_c)));
    float bg_l = dot(edge_bg, float3(0.299,0.587,0.114));
    float scrim = smoothstep(0.48,0.96,bg_l) * 0.16 * mat;
    col = lerp(col, float3(0.30,0.30,0.30), scrim);
    col = col * 1.010 + float3(0.004,0.006,0.010);
    float matdark = (1.0 - mat) * 0.70;
    col = lerp(col, col * 0.58 + float3(0.010,0.014,0.020), float3(matdark,matdark,matdark));
    float topk = 1.0 - smoothstep(0.0, max(hs.y * 2.0, 1.0), local.y + hs.y);
    col += float3(0.06,0.08,0.11) * topk * 0.16;
    float tq = (d + 1.7) / 0.80;
    float bevel = exp(-tq*tq);
    col *= (1.0 - bevel * 0.10);
    float t_in = max(-d, 0.0);
    float bleed = exp(-t_in / clamp(short_side*0.45,5.0,22.0)) * (1.0 - bevel*0.5);
    float lobe = pow(max(dot(g,Ld),0.0),1.5);
    col += float3(0.62,0.86,0.96) * bleed * (lobe*0.15 + 0.055);
    float sigma_c = max(look.z, 0.5) * 0.42;
    float lq = (d + sigma_c*0.65) / sigma_c;
    float rim = exp(-lq*lq);
    float dirw = lerp(0.55,1.0,pow(max(dot(g,Ld),0.0),1.3));
    float line_v = rim * dirw;
    float edge_a = saturate(look.w);
    col = lerp(col, float3(1,1,1), saturate(line_v*edge_a));
    float aa = 1.0 - smoothstep(-1.0,1.0,d);
    return float4(col, saturate(max(aa, line_v*edge_a)));
}

// 液态玻璃「水滴」：直接复用其他控件的玻璃管线 ctrl_glass，材质天然一致（无特殊底色）。
// meta.xy=水滴中心(device px,底原点), zw=半宽半高; meta2.z=开关。
float4 metaball_glass(float2 p, float mat, float2 Ld, float3 edge_bg) {
    float2 dropB = max(meta.zw, float2(1.0,1.0));
    float dropR = min(dropB.x, dropB.y);
    float2 dl = p - meta.xy;
    float d = rr_sdf(dl, dropB, dropR);
    // 和其他控件一模一样的液态玻璃：折射/色散/压暗/白边全走 ctrl_glass。
    // mag=1 不放大, refr=1 标准折射强度（与多数控件默认值一致）。
    return ctrl_glass(p, dl, dropB, dropR, d, mat, Ld, edge_bg, 1.0, 1.0);
}

float4 fs(VSOut i) : SV_TARGET {
    float2 tex = float2(extra.y, extra.z);
    float2 p = i.uv * tex;

    float inset = (notch.w > 0.5) ? 0.0 : 3.0;
    float2 half_size = max(bounds.zw * 0.5 - inset, float2(2,2));
    float2 local = p - bounds.xy - bounds.zw * 0.5;
    float radius = max(min(extra.x - inset, min(half_size.x, half_size.y)), 0.0);
    float d = island_sdf(local, half_size, radius);
    float mat = saturate(params.w);

    // Outside the island: paint the desktop back (or stay transparent under DComp).
    // 液态玻璃的投影就画在这里——D3D11 是「先玻璃、后 egui」，egui 侧阴影会盖住
    // 折射玻璃（迁移前 wgpu 顺序相反才没事）。shader 用岛体 SDF 的高斯衰减画外阴影，
    // 形状/位置/方向自动跟随刘海或胶囊、任意停靠方向都对，且只画在 d>0 的外围。
    // 资料包 8 件套同款：shadow = 0.5*exp(-sd²/2σ²)，再用 SDF 把内部裁掉。
    float thresh = (notch.w > 0.5) ? 1.0 : (inset - 0.25);
    if (d > thresh) {
        if (notch.w > 0.5) {
            // DComp 逐像素 alpha：输出预乘的黑色阴影，越靠近岛体越浓、向外衰减到透明。
            float sigma = 5.0;
            float shadow = exp(-pow(max(d - 1.0, 0.0), 2.0) / (2.0 * sigma * sigma));
            shadow *= lightcfg.z;   // 岛体投影开关（0=关，1=开）
            // 内边界抗锯齿：这里原本 d 刚过 thresh 就把 alpha 从岛体侧的 ~0.05
            // 硬跳到 0.28，沿轮廓一圈无过渡 → 展开态四边锯齿，开投影尤其明显。
            // 用屏幕导数 fwidth(d) 做 1~2 像素的平滑淡入，和岛体边缘 aa_w 同宽，
            // 使阴影从岛体边缘连续长出，不出现台阶。
            float sh_aa = max(fwidth(d), 1.0) * 1.4;
            float edge_fade = smoothstep(thresh, thresh + sh_aa, d);
            return float4(0, 0, 0, saturate(shadow) * 0.28 * edge_fade);
        }
        float3 bgcol = sample_px(p);
        float sh = (1.0 - smoothstep(0.0, 7.0, d - 1.0)) * lerp(0.06, 0.02, mat);
        return float4(bgcol * (1.0 - sh) + float3(0.05,0.06,0.07) * sh, 1.0);
    }

    float aa_w = (notch.w > 0.5) ? max(fwidth(d) * 1.1, 1.4) : 1.2;
    float aa = 1.0 - smoothstep(-aa_w, aa_w, d);

    // Virtual light source follows pointer horizontally; light always from above.
    float2 center = bounds.xy + bounds.zw * 0.5;
    float2 to_ptr = misc.xy - center;
    float plen = length(to_ptr);
    float2 swung = to_ptr / max(plen, 1e-3);
    float2 up_bias = float2(swung.x, min(swung.y, -0.18));
    float follow = saturate(lightcfg.x);
    float2 Ld = normalize(lerp(normalize(float2(-0.45,-0.65)), up_bias,
                               smoothstep(3.0, 48.0, plen) * follow)
                        + float2(1e-6, 1e-6));

    float3 edge_bg = sample_px(p);

    // FLAT diagnostic: show raw captured background (grid seam = alignment check).
    if (misc.w > 0.5) {
        float3 flat_col = sample_px(p);
        CtlSel cs = pick_ctl(p);
        if (cs.idx < MAX_CTL) {
            float4 cr = crect[cs.idx];
            float4 ce = ctrl_glass(p, p - cr.xy - cr.zw*0.5, cs.hs, cs.r, cs.d,
                                   mat, Ld, edge_bg, cs.mag, cs.refr);
            flat_col = lerp(flat_col, ce.rgb, ce.a);
        }
        float a = (notch.w > 0.5) ? aa : 1.0;
        return float4(flat_col * a, a);
    }

    // SDF gradient points outward. 用 island_sdf 保证左右停靠时法线跟着形状一起旋转。
    float e = 1.0;
    float gx = island_sdf(local + float2(e,0), half_size, radius)
             - island_sdf(local - float2(e,0), half_size, radius);
    float gy = island_sdf(local + float2(0,e), half_size, radius)
             - island_sdf(local - float2(0,e), half_size, radius);
    float2 g = normalize(float2(gx, gy) + float2(1e-6, 1e-6));

    // Refraction band: strongest at the edge, decays inward.
    // 角部判定用旋转后的局部坐标，左右停靠（竖向岛体）才能正确识别四个圆角。
    float2 edge_local = local;
    float2 edge_half = half_size;
    if (extra.w > 0.5) {
        edge_local = (extra.w < 1.5) ? float2(local.y, -local.x) : float2(-local.y, local.x);
        edge_half = half_size.yx;
    }
    float band = max(look.y, 1.0);
    float t = saturate(-d / band);
    float k = pow(1.0 - pow(t, 0.30), 2.0);
    float cxn = saturate((abs(edge_local.x) - (edge_half.x - radius)) / max(radius,1));
    float cyn = saturate((abs(edge_local.y) - (edge_half.y - radius)) / max(radius,1));
    float chroma = max(params.x, 0.0) * lerp(0.60, 1.0, cxn * cyn);
    float strength = max(misc.z, 0.0);
    float2 refr = -g * k * strength;
    float2 ca = -g * k * chroma;
    float2 bp = p + refr;

    float3 col = tap5(bp, ca, max(look.x, 0.0) + k * 1.6);
    col = col * 1.004 + float3(0.002, 0.003, 0.006);

    // Top cold light -> glass thickness.
    float topk = 1.0 - smoothstep(0.0, max(bounds.w, 1.0), p.y - bounds.y);
    col += float3(0.05, 0.07, 0.10) * topk * 0.10;

    // Inverse luminance compensation (keeps contrast on bright wallpapers).
    float lum_m = dot(col, float3(0.299, 0.587, 0.114));
    float lift = lerp(0.045, -0.035, smoothstep(0.42, 0.88, lum_m));
    col *= (1.0 + lift);

    // Material: dark variant must stay visibly below background.
    float matdark = (1.0 - mat) * 0.70;
    col = lerp(col, col * 0.58 + float3(0.010,0.014,0.020), float3(matdark,matdark,matdark));
    float matfog = mat * 0.02;
    col = lerp(col, float3(0.96,0.97,1.0), float3(matfog,matfog,matfog));

    // Per-pixel scrim for text legibility (only on light material).
    float bg_l = dot(edge_bg, float3(0.299, 0.587, 0.114));
    float scrim = smoothstep(0.48, 0.96, bg_l) * 0.16 * mat;
    col = lerp(col, float3(0.30,0.30,0.30), scrim);

    // Restore a little saturation washed out by the scrim.
    float lum0 = dot(col, float3(0.299, 0.587, 0.114));
    col = lerp(float3(lum0,lum0,lum0), col, 1.06);

    // Bevel dark band.
    float tq = (d + 2.2) / 0.9;
    float bevel = exp(-tq * tq);
    col *= (1.0 - bevel * 0.08);

    // Internal light bleed (suppressed by bevel) + Beer-Lambert edge absorption.
    float t_in = max(-d, 0.0);
    float bleed = exp(-t_in / 16.0) * (1.0 - bevel * 0.55);
    float path = exp(-t_in / 7.0);
    col *= lerp(float3(1,1,1), float3(0.90,1.0,0.96), path * 0.65);
    float absorb = 1.0 - exp(-t_in / 10.0);
    float3 lcol = lerp(float3(0.88,0.93,1.0), float3(0.52,0.90,0.80), absorb);
    float lobe_a = pow(max(dot(g, Ld), 0.0), 1.6);
    float lobe_b = pow(max(dot(g, -Ld), 0.0), 1.2);
    col += lcol * bleed * (lobe_a * 0.16 + lobe_b * 0.085 + 0.050);
    col += tint.rgb * tint.a * 0.05 * bleed;

    // Sharp rim highlight, directionally modulated (the "liquid" feel).
    float sigma = max(look.z, 0.3) * 0.365;
    float lq = (d + sigma * 0.55) / sigma;
    float rim = exp(-lq * lq);
    float gq = (d + 1.0) / max(3.2 * look.z / 1.5, 0.8);
    float glow = exp(-gq * gq) * 0.04;
    float dirw = lerp(0.40, 1.0, pow(max(dot(g, Ld), 0.0), 1.5));
    float line_v = rim * dirw;
    float edge_a = saturate(look.w);
    float line_w = edge_a * lerp(0.69, 1.02, smoothstep(0.60, 0.92, lum_m));
    col = lerp(col, float3(1,1,1), saturate(line_v * line_w + glow));

    // Per-control inner glass lenses (same capture, multiple small lenses).
    CtlSel cs = pick_ctl(p);
    if (cs.idx < MAX_CTL) {
        float4 cr = crect[cs.idx];
        float4 ce = ctrl_glass(p, p - cr.xy - cr.zw*0.5, cs.hs, cs.r, cs.d,
                               mat, Ld, edge_bg, cs.mag, cs.refr);
        // 分栏标签栏层次（资料包）：容器高斯模糊磨砂+细白边（不压暗），
        // 指示器清晰折射的亮玻璃+强白边。样式由 cstyle[槽位].x 传入
        // （槽位是 pick_ctl 命中的紧凑索引，不是 Ctl 枚举 idx）。
        float style = cstyle[cs.idx].x;
        if (style > 0.5 && style < 1.5) {
            // 容器：13 点高斯模糊背景；边缘保留 ctrl_glass 的白边/倒角，内部换成磨砂底。
            // 用户明确不要暗底——不做 ×0.55 压暗、不加深色 tint。
            float inner = smoothstep(0.0, 7.0, -cs.d);
            float3 frost = gauss_blur(p, 6.0);
            // 与 ctrl_glass 相同的浅材质补偿，避免模糊底偏色；不叠加 scrim 压暗。
            float fl = dot(frost, float3(0.299,0.587,0.114));
            frost = frost * (1.0 + lerp(0.045,-0.035,smoothstep(0.42,0.88,fl))) * 1.010
                  + float3(0.004,0.006,0.010);
            ce.rgb = lerp(ce.rgb, frost, inner * 0.85);
        } else if (style > 1.5) {
            // 指示器更亮：提亮主体 + 按 SDF 边缘加强白边高光。
            ce.rgb = ce.rgb * 1.10 + float3(0.02,0.02,0.03);
            float tabRim = exp(-pow((cs.d + 1.4) / 1.6, 2.0));
            ce.rgb = lerp(ce.rgb, float3(1,1,1), saturate(tabRim * 0.75));
            ce.a = saturate(max(ce.a, tabRim * 0.9));
        }
        col = lerp(col, ce.rgb, ce.a);
    }

    // Blend edge with the (unrefracted) desktop so no black halo; dither.
    float cov = saturate(max(aa, line_v * edge_a));

    // 液态玻璃融球水滴：作为独立玻璃层浮在岛体之上（不合并进岛体 SDF）。
    if (meta2.z > 0.5) {
        float4 mb = metaball_glass(p, mat, Ld, edge_bg);
        col = lerp(col, mb.rgb, mb.a);
        cov = saturate(max(cov, mb.a));
    }

    float dth = (hash21(p) - 0.5) * (1.7 / 255.0);
    float3 outc = lerp(edge_bg, col, float3(cov,cov,cov)) + float3(dth,dth,dth);
    if (notch.w > 0.5) return float4(outc * cov, cov);
    return float4(outc, 1.0);
}
