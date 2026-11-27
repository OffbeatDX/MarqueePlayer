//! Pure-Rust macOS Metal + Core Animation visualizer underlay.
//!
//! Composites the visualizer directly beneath Slint's transparent foreground layer using a
//! stage-sized opaque `CAMetalLayer` inside a Core Animation scene layer (`scene` + optional
//! `panel` + `viz`). While active, Slint's Skia renderer draws zero visualizer frames per
//! second; Core Animation composites the Metal drawable with the static Slint chrome in
//! hardware while keeping `NSWindow` opaque so WindowServer uses its fast opaque path.
//!
//! Anti-aliasing without MSAA memory or tile-bandwidth overhead:
//! - Every style renders at 1× sample count with analytic per-pixel SDF / edge-coverage AA
//!   (bars with rounded tops, circular dots/sparks, miter-clamped stroke ribbons, feathered
//!   curve/corona silhouettes, circular/rounded-box cover clip and drop shadow).

use std::ptr::NonNull;
use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{ClassType, msg_send};
use objc2_app_kit::{NSColor, NSView};
use objc2_core_foundation::{CFData, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{
    CGBitmapInfo, CGColor, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
    CGImageAlphaInfo, kCGColorSpaceSRGB,
};
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLBuffer, MTLClearColor, MTLCommandBuffer,
    MTLCommandEncoder, MTLCommandQueue, MTLCompareFunction, MTLCreateSystemDefaultDevice,
    MTLDepthStencilDescriptor, MTLDepthStencilState, MTLDevice, MTLGPUFamily, MTLLibrary,
    MTLLoadAction, MTLOrigin, MTLPixelFormat, MTLPrimitiveType, MTLRegion,
    MTLRenderCommandEncoder, MTLRenderPassDescriptor, MTLRenderPipelineDescriptor,
    MTLRenderPipelineState, MTLResourceOptions, MTLSamplerAddressMode, MTLSamplerDescriptor,
    MTLSamplerMinMagFilter, MTLSamplerState, MTLSize, MTLStencilDescriptor, MTLStencilOperation,
    MTLStorageMode, MTLStoreAction, MTLTexture, MTLTextureDescriptor, MTLTextureUsage, MTLViewport,
};
use objc2_quartz_core::{
    CALayer, CAMetalDrawable, CAMetalLayer, CATransaction, kCAGravityResize,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::winit_030::WinitWindowAccessor;
use slint::{Brush, Color, ComponentHandle, Image, Timer};

use crate::visualizer::Where;
use crate::visualizer::draw::{
    self, BarQuad, Brushes, CoronaInner, Dot, NativeFrame, OpenCurve, ReflCurve, STRIPS, Stage,
};
use crate::visualizer::look::{Look, Style};
use crate::{AppWindow, Backdrop, Player, Theme, Visualizer};

const MAX_STOPS: usize = 16;
const RING_BUFFERS: usize = 3;

const MSL_SOURCE: &str = r#"
#include <metal_stdlib>
using namespace metal;

struct ViewportUniforms {
    float2 stage_offset;
    float2 layer_size;
    float scale;
    float _pad;
};

struct BrushUniforms {
    uint kind;         // 0=solid, 1=linear, 2=radial, 3=conic
    uint stop_count;
    float alpha_mul;
    float _pad0;
    float4 box;        // (x, y, w, h) in stage logical px
    float4 geom;       // linear: (sx, sy, ex, ey) in box uv; radial: (cx, cy, rx, ry) stage px; conic: (cx, cy, 0, 0)
    float4 colors[16]; // straight sRGB (r, g, b, a)
    float4 stops[4];   // 16 packed stop positions
};

static inline float stop_pos(constant BrushUniforms &b, uint i) {
    return b.stops[i >> 2][i & 3];
}

static inline float4 eval_brush(constant BrushUniforms &b, float2 stage_pos) {
    if (b.stop_count == 0) {
        return float4(0.0);
    }
    if (b.kind == 0 || b.stop_count == 1) {
        float4 c = b.colors[0];
        c.a = clamp(c.a * b.alpha_mul, 0.0, 1.0);
        return c;
    }

    float t = 0.0;
    if (b.kind == 1) {
        float2 uv = (stage_pos - b.box.xy) / max(b.box.zw, float2(1.0));
        float2 s = b.geom.xy;
        float2 d = b.geom.zw - s;
        float denom = max(dot(d, d), 1e-6);
        t = dot(uv - s, d) / denom;
    } else if (b.kind == 2) {
        float2 d = (stage_pos - b.geom.xy) / max(b.geom.zw, float2(1e-4));
        t = length(d);
    } else {
        float2 d = stage_pos - b.geom.xy;
        float angle = atan2(d.x, -d.y); // 0 at 12 o'clock, clockwise
        t = fract(angle / (2.0 * M_PI_F));
    }
    t = clamp(t, 0.0, 1.0);

    float p0 = stop_pos(b, 0);
    if (t <= p0) {
        float4 c = b.colors[0];
        c.a = clamp(c.a * b.alpha_mul, 0.0, 1.0);
        return c;
    }
    uint last = b.stop_count - 1;
    for (uint i = 1; i <= last; ++i) {
        float p1 = stop_pos(b, i);
        if (t <= p1) {
            float k = (p1 > p0) ? (t - p0) / (p1 - p0) : 1.0;
            float4 c = mix(b.colors[i - 1], b.colors[i], clamp(k, 0.0, 1.0));
            c.a = clamp(c.a * b.alpha_mul, 0.0, 1.0);
            return c;
        }
        p0 = p1;
    }
    float4 c = b.colors[last];
    c.a = clamp(c.a * b.alpha_mul, 0.0, 1.0);
    return c;
}

static inline float4 to_ndc(float2 stage_pos, constant ViewportUniforms &vp) {
    float2 p = stage_pos + vp.stage_offset;
    float2 ndc = float2(p.x / vp.layer_size.x * 2.0 - 1.0, 1.0 - p.y / vp.layer_size.y * 2.0);
    return float4(ndc, 0.0, 1.0);
}

// ---- 0. Opaque backdrop (all float4 fields first so Rust & MSL layouts match identically) ----
struct BackdropUniforms {
    float4 bg;
    float4 tint;
    float4 panel_rect;
    float2 layer_origin;
    float2 layer_size;
    float2 win_size;
    float2 art_origin;
    float2 art_size;
    float opacity;
    float visible;
    float has_art;
    float panel_radius;
};

struct BackdropOut {
    float4 pos [[position]];
    float2 win_pos;
};

vertex BackdropOut vs_backdrop(
    uint vid [[vertex_id]],
    constant BackdropUniforms &bd [[buffer(0)]]
) {
    const float2 uv[3] = {
        float2(0.0, 0.0),
        float2(2.0, 0.0),
        float2(0.0, 2.0),
    };
    float2 p = uv[vid];
    BackdropOut out;
    out.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.win_pos = bd.layer_origin + p * bd.layer_size;
    return out;
}

fragment float4 fs_backdrop(
    BackdropOut in [[stage_in]],
    constant BackdropUniforms &bd [[buffer(0)]],
    texture2d<float> art_tex [[texture(0)]],
    sampler samp [[sampler(0)]]
) {
    float3 rgb = bd.bg.rgb;
    if (bd.visible > 0.5) {
        if (bd.has_art > 0.5) {
            float2 uv = clamp((in.win_pos - bd.art_origin) / max(bd.art_size, float2(1.0)), float2(0.0), float2(1.0));
            float3 art = art_tex.sample(samp, uv).rgb;
            rgb = mix(rgb, art, clamp(bd.opacity, 0.0, 1.0));
        }
        float k = clamp((in.win_pos.y / max(bd.win_size.y, 1.0)) / 0.7, 0.0, 1.0);
        float4 t = mix(bd.tint, bd.bg, k);
        rgb = mix(rgb, t.rgb, clamp(t.a, 0.0, 1.0));
    }
    if (bd.panel_rect.z > 0.0 && bd.panel_rect.w > 0.0) {
        float2 half_size = bd.panel_rect.zw * 0.5;
        float2 center = bd.panel_rect.xy + half_size;
        float r = min(bd.panel_radius, min(half_size.x, half_size.y));
        float2 q = abs(in.win_pos - center) - (half_size - float2(r));
        float dist = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - r;
        if (dist <= 0.0) {
            rgb *= (1.0 - 64.0 / 255.0);
        }
    }
    return float4(rgb, 1.0);
}

// ---- 1. Instanced quads (bars, caps, reflections, bloom) ----
struct QuadInstance {
    float4 rect;       // x, y, w, h in stage logical px
    float top_radius;
    float crisp;
    float2 _pad;
};

struct QuadOut {
    float4 pos [[position]];
    float2 stage_pos;
    float2 local_pos;
    float2 size;
    float top_radius;
    float crisp;
};

vertex QuadOut vs_quad(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    const device QuadInstance *insts [[buffer(1)]]
) {
    const float2 corners[6] = {
        float2(0.0, 0.0), float2(1.0, 0.0), float2(0.0, 1.0),
        float2(1.0, 0.0), float2(1.0, 1.0), float2(0.0, 1.0),
    };
    QuadInstance q = insts[iid];
    float2 c = corners[vid];
    float2 local = c * q.rect.zw;
    float2 sp = q.rect.xy + local;
    QuadOut out;
    out.pos = to_ndc(sp, vp);
    out.stage_pos = sp;
    out.local_pos = local;
    out.size = q.rect.zw;
    out.top_radius = q.top_radius;
    out.crisp = q.crisp;
    return out;
}

fragment float4 fs_quad(
    QuadOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float cov = 1.0;
    if (in.top_radius > 0.0) {
        float r = min(in.top_radius, min(in.size.x * 0.5, in.size.y * 0.5));
        if (in.local_pos.y < r) {
            float dx = 0.0;
            if (in.local_pos.x < r) {
                dx = r - in.local_pos.x;
            } else if (in.local_pos.x > in.size.x - r) {
                dx = in.local_pos.x - (in.size.x - r);
            }
            if (dx > 0.0) {
                float dy = r - in.local_pos.y;
                float dist = length(float2(dx, dy)) - r;
                float aa = max(0.5 / vp.scale, 0.25);
                cov = 1.0 - smoothstep(-aa, aa, dist);
                if (cov <= 0.001) {
                    discard_fragment();
                }
            }
        }
    }
    float4 c = eval_brush(brush, in.stage_pos);
    float a = c.a * cov;
    return float4(c.rgb * a, a);
}

// ---- 2. Instanced circular dots (sparks, Saturn dust, glow discs) ----
struct DotInstance {
    float2 center;
    float radius;
    float _pad;
};

struct DotOut {
    float4 pos [[position]];
    float2 stage_pos;
    float2 offset;
    float radius;
};

vertex DotOut vs_dot(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    const device DotInstance *insts [[buffer(1)]]
) {
    const float2 corners[6] = {
        float2(-1.0, -1.0), float2(1.0, -1.0), float2(-1.0, 1.0),
        float2(1.0, -1.0),  float2(1.0, 1.0),  float2(-1.0, 1.0),
    };
    DotInstance d = insts[iid];
    float pad = max(1.0 / vp.scale, 0.75);
    float ext = d.radius + pad;
    float2 off = corners[vid] * ext;
    float2 sp = d.center + off;
    DotOut out;
    out.pos = to_ndc(sp, vp);
    out.stage_pos = sp;
    out.offset = off;
    out.radius = d.radius;
    return out;
}

fragment float4 fs_dot(
    DotOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float dist = length(in.offset) - in.radius;
    float aa = max(0.55 / vp.scale, 0.25);
    float cov = 1.0 - smoothstep(-aa, aa, dist);
    if (cov <= 0.001) {
        discard_fragment();
    }
    float4 c = eval_brush(brush, in.stage_pos);
    float a = c.a * cov;
    return float4(c.rgb * a, a);
}

// ---- 3. Feathered 2D fills (curve fills, reflections, corona annulus) ----
struct PolyVertex {
    float2 pos;
    float cov;
    float _pad;
};

struct PolyOut {
    float4 pos [[position]];
    float2 stage_pos;
    float cov;
};

vertex PolyOut vs_poly(
    uint vid [[vertex_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    const device PolyVertex *verts [[buffer(1)]]
) {
    PolyVertex v = verts[vid];
    PolyOut out;
    out.pos = to_ndc(v.pos, vp);
    out.stage_pos = v.pos;
    out.cov = v.cov;
    return out;
}

fragment float4 fs_poly(
    PolyOut in [[stage_in]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float4 c = eval_brush(brush, in.stage_pos);
    float a = c.a * clamp(in.cov, 0.0, 1.0);
    return float4(c.rgb * a, a);
}

// ---- 4. Analytic-AA stroke ribbons (line, glow, ripples, snare rings) ----
struct StrokeVertex {
    float2 pos;
    float edge_dist;
    float half_width;
};

struct StrokeOut {
    float4 pos [[position]];
    float2 stage_pos;
    float edge_dist;
    float half_width;
};

vertex StrokeOut vs_stroke(
    uint vid [[vertex_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    const device StrokeVertex *verts [[buffer(1)]]
) {
    StrokeVertex v = verts[vid];
    StrokeOut out;
    out.pos = to_ndc(v.pos, vp);
    out.stage_pos = v.pos;
    out.edge_dist = v.edge_dist;
    out.half_width = v.half_width;
    return out;
}

fragment float4 fs_stroke(
    StrokeOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float dist = abs(in.edge_dist) - in.half_width;
    float aa = max(0.55 / vp.scale, 0.25);
    float cov = 1.0 - smoothstep(-aa, aa, dist);
    if (cov <= 0.001) {
        discard_fragment();
    }
    float4 c = eval_brush(brush, in.stage_pos);
    float a = c.a * cov;
    return float4(c.rgb * a, a);
}

// ---- 4b. Wide translucent strokes as stencil-deduplicated analytic capsules (zero bowties, zero self-overlap) ----
struct CapsuleInstance {
    float2 p0;
    float2 p1;
    float half_width;
    float _pad;
};

struct CapsuleOut {
    float4 pos [[position]];
    float2 stage_pos;
    float2 p0;
    float2 p1;
    float half_width;
};

vertex CapsuleOut vs_capsule(
    uint vid [[vertex_id]],
    uint iid [[instance_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    const device CapsuleInstance *insts [[buffer(1)]]
) {
    const float2 corners[6] = {
        float2(0.0, -1.0), float2(1.0, -1.0), float2(0.0, 1.0),
        float2(1.0, -1.0), float2(1.0,  1.0), float2(0.0, 1.0),
    };
    CapsuleInstance c = insts[iid];
    float2 ba = c.p1 - c.p0;
    float len = length(ba);
    float2 dir = len > 1e-5 ? ba / len : float2(1.0, 0.0);
    float2 perp = float2(-dir.y, dir.x);
    float ext = c.half_width + max(1.0 / vp.scale, 0.75);
    float2 corner = corners[vid];
    float2 along = mix(c.p0 - dir * ext, c.p1 + dir * ext, corner.x);
    float2 sp = along + perp * (corner.y * ext);
    CapsuleOut out;
    out.pos = to_ndc(sp, vp);
    out.stage_pos = sp;
    out.p0 = c.p0;
    out.p1 = c.p1;
    out.half_width = c.half_width;
    return out;
}

static inline float capsule_cov(CapsuleOut in, float scale) {
    float2 pa = in.stage_pos - in.p0;
    float2 ba = in.p1 - in.p0;
    float h = clamp(dot(pa, ba) / max(dot(ba, ba), 1e-6), 0.0, 1.0);
    float d = length(pa - ba * h) - in.half_width;
    float aa = max(0.55 / scale, 0.25);
    return 1.0 - smoothstep(-aa, aa, d);
}

fragment float4 fs_capsule_core(
    CapsuleOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float cov = capsule_cov(in, vp.scale);
    if (cov < 0.99) {
        discard_fragment();
    }
    float4 c = eval_brush(brush, in.stage_pos);
    return float4(c.rgb * c.a, c.a);
}

fragment float4 fs_capsule_aa(
    CapsuleOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant BrushUniforms &brush [[buffer(1)]]
) {
    float cov = capsule_cov(in, vp.scale);
    if (cov <= 0.005 || cov >= 0.99) {
        discard_fragment();
    }
    float4 c = eval_brush(brush, in.stage_pos);
    float a = c.a * cov;
    return float4(c.rgb * a, a);
}

// ---- 5. Analytic rounded-rect / circle drop shadow (ring cover) ----
struct ShadowUniforms {
    float2 center;
    float2 half_size;
    float radius;
    float sigma;
    float alpha;
    float _pad;
};

struct ShadowOut {
    float4 pos [[position]];
    float2 local_pos;
};

static inline float erf_approx(float x) {
    float s = sign(x);
    float a = abs(x);
    float t = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    float t2 = t * t;
    return s - s / (t2 * t2);
}

static inline float rounded_box_shadow_x(float x, float y, float sigma, float corner, float2 half_size) {
    float delta = min(half_size.y - corner - abs(y), 0.0);
    float curved = half_size.x - corner + sqrt(max(0.0, corner * corner - delta * delta));
    float2 integral = 0.5 + 0.5 * float2(erf_approx((x + curved) * (sqrt(0.5) / sigma)),
                                         erf_approx((x - curved) * (sqrt(0.5) / sigma)));
    return integral.x - integral.y;
}

vertex ShadowOut vs_shadow(
    uint vid [[vertex_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant ShadowUniforms &sh [[buffer(1)]]
) {
    const float2 corners[6] = {
        float2(-1.0, -1.0), float2(1.0, -1.0), float2(-1.0, 1.0),
        float2(1.0, -1.0),  float2(1.0, 1.0),  float2(-1.0, 1.0),
    };
    float2 ext = sh.half_size + float2(sh.sigma * 3.0);
    float2 local = corners[vid] * ext;
    ShadowOut out;
    out.pos = to_ndc(sh.center + local, vp);
    out.local_pos = local;
    return out;
}

fragment float4 fs_shadow(
    ShadowOut in [[stage_in]],
    constant ShadowUniforms &sh [[buffer(1)]]
) {
    float sigma = max(sh.sigma, 0.5);
    float val = 0.0;
    if (sh.radius >= min(sh.half_size.x, sh.half_size.y) - 0.5) {
        // Fast exact radial Gaussian falloff for circular cover.
        float d = length(in.local_pos) - sh.radius;
        val = 0.5 * (1.0 - erf_approx(d * (sqrt(0.5) / sigma)));
    } else {
        float low = in.local_pos.y - sh.half_size.y;
        float high = in.local_pos.y + sh.half_size.y;
        float start = clamp(-3.0 * sigma, low, high);
        float end = clamp(3.0 * sigma, low, high);
        float step = (end - start) / 4.0;
        float y = start + step * 0.5;
        for (int i = 0; i < 4; ++i) {
            float g = exp(-(y * y) / (2.0 * sigma * sigma)) / (sqrt(2.0 * M_PI_F) * sigma);
            val += rounded_box_shadow_x(in.local_pos.x, in.local_pos.y - y, sigma, sh.radius, sh.half_size) * g * step;
            y += step;
        }
    }
    float a = clamp(val * sh.alpha, 0.0, 1.0);
    return float4(0.0, 0.0, 0.0, a);
}

// ---- 6. Ring cover image with analytic rounded-rect / circular clip ----
struct CoverUniforms {
    float2 center;
    float2 half_size;
    float radius;
    float has_cover;
    float2 _pad;
};

struct CoverOut {
    float4 pos [[position]];
    float2 local_pos;
    float2 uv;
};

vertex CoverOut vs_cover(
    uint vid [[vertex_id]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant CoverUniforms &cv [[buffer(1)]]
) {
    const float2 corners[6] = {
        float2(-1.0, -1.0), float2(1.0, -1.0), float2(-1.0, 1.0),
        float2(1.0, -1.0),  float2(1.0, 1.0),  float2(-1.0, 1.0),
    };
    float2 c = corners[vid];
    float2 local = c * cv.half_size;
    CoverOut out;
    out.pos = to_ndc(cv.center + local, vp);
    out.local_pos = local;
    out.uv = c * 0.5 + 0.5;
    return out;
}

fragment float4 fs_cover(
    CoverOut in [[stage_in]],
    constant ViewportUniforms &vp [[buffer(0)]],
    constant CoverUniforms &cv [[buffer(1)]],
    texture2d<float> tex [[texture(0)]],
    sampler samp [[sampler(0)]]
) {
    float r = min(cv.radius, min(cv.half_size.x, cv.half_size.y));
    float2 q = abs(in.local_pos) - (cv.half_size - float2(r));
    float dist = length(max(q, float2(0.0))) + min(max(q.x, q.y), 0.0) - r;
    float aa = max(0.55 / vp.scale, 0.25);
    float cov = 1.0 - smoothstep(-aa, aa, dist);
    if (cov <= 0.001) {
        discard_fragment();
    }
    float4 col = cv.has_cover > 0.5 ? tex.sample(samp, in.uv) : float4(0.12, 0.12, 0.14, 1.0);
    float a = col.a * cov;
    return float4(col.rgb * a, a);
}
"#;

#[repr(C)]
#[derive(Clone, Copy)]
struct ViewportUniforms {
    stage_offset: [f32; 2],
    layer_size: [f32; 2],
    scale: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BrushUniforms {
    kind: u32,
    stop_count: u32,
    alpha_mul: f32,
    _pad0: f32,
    box_rect: [f32; 4],
    geom: [f32; 4],
    colors: [[f32; 4]; MAX_STOPS],
    stops: [[f32; 4]; 4],
}

impl Default for BrushUniforms {
    fn default() -> Self {
        Self {
            kind: 0,
            stop_count: 0,
            alpha_mul: 1.0,
            _pad0: 0.0,
            box_rect: [0.0, 0.0, 1.0, 1.0],
            geom: [0.0; 4],
            colors: [[0.0; 4]; MAX_STOPS],
            stops: [[0.0; 4]; 4],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct BackdropUniforms {
    bg: [f32; 4],
    tint: [f32; 4],
    panel_rect: [f32; 4],
    layer_origin: [f32; 2],
    layer_size: [f32; 2],
    win_size: [f32; 2],
    art_origin: [f32; 2],
    art_size: [f32; 2],
    opacity: f32,
    visible: f32,
    has_art: f32,
    panel_radius: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct QuadInstance {
    rect: [f32; 4],
    top_radius: f32,
    crisp: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DotInstance {
    center: [f32; 2],
    radius: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct PolyVertex {
    pos: [f32; 2],
    cov: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StrokeVertex {
    pos: [f32; 2],
    edge_dist: f32,
    half_width: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CapsuleInstance {
    p0: [f32; 2],
    p1: [f32; 2],
    half_width: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ShadowUniforms {
    center: [f32; 2],
    half_size: [f32; 2],
    radius: f32,
    sigma: f32,
    alpha: f32,
    _pad: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CoverUniforms {
    center: [f32; 2],
    half_size: [f32; 2],
    radius: f32,
    has_cover: f32,
    _pad: [f32; 2],
}

struct Pipelines {
    backdrop: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    quad: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    dot: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    poly: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    stroke: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    capsule_core: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    capsule_aa: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    shadow: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
    cover: Retained<ProtocolObject<dyn MTLRenderPipelineState>>,
}

struct GpuBrushes {
    fill: BrushUniforms,
    caps: BrushUniforms,
    line: BrushUniforms,
    glow: BrushUniforms,
    refl: [BrushUniforms; STRIPS],
    bloom: BrushUniforms,
    ripples: BrushUniforms,
    rings: BrushUniforms,
    sparks: BrushUniforms,
    sparks_old: BrushUniforms,
    sparks_glow: BrushUniforms,
    line_width: f32,
    glow_width: f32,
    ripple_width: f32,
    ring_width: f32,
}

impl Default for GpuBrushes {
    fn default() -> Self {
        Self {
            fill: BrushUniforms::default(),
            caps: BrushUniforms::default(),
            line: BrushUniforms::default(),
            glow: BrushUniforms::default(),
            refl: [BrushUniforms::default(); STRIPS],
            bloom: BrushUniforms::default(),
            ripples: BrushUniforms::default(),
            rings: BrushUniforms::default(),
            sparks: BrushUniforms::default(),
            sparks_old: BrushUniforms::default(),
            sparks_glow: BrushUniforms::default(),
            line_width: 2.0,
            glow_width: 0.0,
            ripple_width: 1.5,
            ring_width: 1.5,
        }
    }
}

#[derive(Clone, PartialEq)]
struct BackdropState {
    bg: Color,
    tint: Color,
    opacity: f32,
    visible: bool,
    image: Option<Image>,
    win_aspect_milli: u32,
}

#[derive(Clone, Copy, PartialEq)]
struct LayoutState {
    root_w: f64,
    root_h: f64,
    points: f64,
    scale: f32,
    vx: f32,
    vy: f32,
    vw: f32,
    vh: f32,
    dw: u32,
    dh: u32,
    panel_rect: [f32; 4],
    panel_radius: f32,
}

#[cfg(feature = "bench")]
#[derive(Clone)]
struct LastRender {
    frame: NativeFrame,
    look: Look,
    stage: Stage,
    at: Where,
    stage_x: f32,
    stage_y: f32,
    vx: f32,
    vy: f32,
    vw: f32,
    vh: f32,
    win_w: f32,
    win_h: f32,
    dw: u32,
    dh: u32,
    scale: f32,
    square: bool,
    radius_card: f32,
    has_cover: bool,
    refl_opacity: f32,
    bg: Color,
    tint: Color,
    backdrop_opacity: f32,
    backdrop_visible: bool,
    panel_rect: [f32; 4],
    panel_radius: f32,
}

/// Reusable staging buffers for one Metal frame (zero per-frame heap churn).
#[derive(Default)]
struct Staging {
    bytes: Vec<u8>,
    poly_pts: Vec<(f32, f32)>,
    inner_pts: Vec<(f32, f32)>,
    poly_verts: Vec<PolyVertex>,
    stroke_verts: Vec<StrokeVertex>,
}

impl Staging {
    fn reset(&mut self) {
        self.bytes.clear();
    }

    fn push_slice<T: Copy>(&mut self, items: &[T]) -> usize {
        push_bytes(&mut self.bytes, items)
    }

    fn push_poly_verts(&mut self) -> usize {
        push_bytes(&mut self.bytes, &self.poly_verts)
    }

    fn push_stroke_verts(&mut self) -> usize {
        push_bytes(&mut self.bytes, &self.stroke_verts)
    }
}

fn push_bytes<T: Copy>(bytes: &mut Vec<u8>, items: &[T]) -> usize {
    let align = std::mem::align_of::<T>().max(16);
    let pad = (align - (bytes.len() % align)) % align;
    bytes.resize(bytes.len() + pad, 0);
    let offset = bytes.len();
    let byte_len = std::mem::size_of_val(items);
    if byte_len > 0 {
        // SAFETY: T is Copy/repr(C) POD; reading its raw bytes is sound.
        let raw = unsafe { std::slice::from_raw_parts(items.as_ptr().cast::<u8>(), byte_len) };
        bytes.extend_from_slice(raw);
    }
    offset
}

struct MetalCtx {
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    queue: Retained<ProtocolObject<dyn MTLCommandQueue>>,
    sampler: Retained<ProtocolObject<dyn MTLSamplerState>>,
    pipes: Pipelines,
    ds_none: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    ds_once: Retained<ProtocolObject<dyn MTLDepthStencilState>>,
    memoryless_stencil: bool,
}

type SizedTexture = (u32, u32, Retained<ProtocolObject<dyn MTLTexture>>);

#[derive(Default)]
pub(crate) struct Presenter {
    ctx: Option<MetalCtx>,
    init_failed: bool,
    scene: Option<Retained<CALayer>>,
    foreground: Option<Retained<CALayer>>,
    panel_layer: Option<Retained<CALayer>>,
    viz_layer: Option<Retained<CAMetalLayer>>,
    stencil_tex: Option<SizedTexture>,
    ring: [Option<Retained<ProtocolObject<dyn MTLBuffer>>>; RING_BUFFERS],
    ring_idx: usize,
    staging: Staging,
    brushes: GpuBrushes,
    backdrop_state: Option<BackdropState>,
    art_tex: Option<SizedTexture>,
    layout_state: Option<LayoutState>,
    cover_image: Option<Image>,
    cover_tex: Option<Retained<ProtocolObject<dyn MTLTexture>>>,
    active: bool,
    #[cfg(feature = "bench")]
    last: Option<LastRender>,
}

pub(crate) fn enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MP_VIZ_METAL").as_deref() != Ok("0"))
}

impl Presenter {
    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    pub(crate) fn set_brushes(&mut self, b: &Brushes, stage: &Stage, top_h: f32, refl_h: f32) {
        let top_box = [0.0, 0.0, stage.w.max(1.0), top_h.max(1.0)];
        let full_box = [0.0, 0.0, stage.w.max(1.0), stage.h.max(1.0)];
        let refl_box = [0.0, top_h, stage.w.max(1.0), refl_h.max(1.0)];

        self.brushes.fill = convert_brush(&b.fill, top_box);
        self.brushes.caps = convert_brush(&b.caps, top_box);
        self.brushes.line = convert_brush(&b.line, top_box);
        self.brushes.glow = convert_brush(&b.glow, top_box);
        for (dst, src) in self.brushes.refl.iter_mut().zip(&b.refl) {
            *dst = convert_brush(src, refl_box);
        }
        self.brushes.bloom = convert_brush(&b.bloom, full_box);
        self.brushes.ripples = convert_brush(&b.ripples, full_box);
        self.brushes.rings = convert_brush(&b.rings, full_box);
        self.brushes.sparks = convert_brush(&b.sparks, full_box);
        self.brushes.sparks_old = convert_brush(&b.sparks_old, full_box);
        self.brushes.sparks_glow = convert_brush(&b.sparks_glow, full_box);
        self.brushes.line_width = b.line_width;
        self.brushes.glow_width = b.glow_width;
        self.brushes.ripple_width = b.ripple_width;
        self.brushes.ring_width = b.ring_width;
    }

    pub(crate) fn remove(&mut self, app: &AppWindow) {
        let was_active = self.active;
        if self.active {
            self.active = false;
            app.global::<Visualizer>().set_native_underlay(false);
        }
        if let Some(scene) = self.scene.take() {
            if was_active {
                // Keep the opaque backdrop/visualizer scene mounted briefly while Slint paints
                // its opaque replacement frame, preventing a 1-frame transparent/black flash.
                Timer::single_shot(Duration::from_millis(40), move || {
                    CATransaction::begin();
                    CATransaction::setDisableActions(true);
                    scene.removeFromSuperlayer();
                    CATransaction::commit();
                });
            } else {
                CATransaction::begin();
                CATransaction::setDisableActions(true);
                scene.removeFromSuperlayer();
                CATransaction::commit();
            }
        }
        self.foreground = None;
        self.panel_layer = None;
        self.viz_layer = None;
        self.stencil_tex = None;
        self.backdrop_state = None;
        self.art_tex = None;
        self.layout_state = None;
        self.cover_image = None;
        self.cover_tex = None;
        #[cfg(feature = "bench")]
        {
            self.last = None;
        }
    }

    /// Render one visualizer frame into the macOS Metal underlay. Returns `true` if presented
    /// natively, `false` if the caller should fall back to Slint's Skia path renderer.
    pub(crate) fn present(
        &mut self,
        app: &AppWindow,
        look: &Look,
        stage: &Stage,
        at: Where,
        frame: &NativeFrame,
    ) -> bool {
        if !enabled() || self.init_failed {
            self.remove(app);
            return false;
        }
        if !self.ensure_ctx() {
            self.remove(app);
            return false;
        }
        if !self.ensure_attached(app) {
            self.remove(app);
            return false;
        }

        let g = app.global::<Visualizer>();
        let t = app.global::<Theme>();
        let p = app.global::<Player>();

        let scale = app.window().scale_factor().max(0.5);
        let win_px = app.window().size();
        if win_px.width == 0 || win_px.height == 0 {
            return false;
        }
        let win_w = win_px.width as f32 / scale;
        let win_h = win_px.height as f32 / scale;
        let stage_x = g.get_stage_x();
        let stage_y = g.get_stage_y();
        let open = matches!(at, Where::Behind | Where::Page);

        let (vx, vy, vw, vh) = if look.style.around() && open {
            let h = draw::halo(stage);
            let max_r = h.hub + h.gap + h.travel + h.glow * 0.5 + 8.0;
            let cx = stage_x + h.cx;
            let cy = stage_y + h.cy;
            let left = (cx - max_r).floor().max(0.0);
            let top = (cy - max_r).floor().max(0.0);
            let right = (cx + max_r).ceil().min(win_w);
            let bottom = (cy + max_r).ceil().min(win_h);
            (left, top, (right - left).max(1.0), (bottom - top).max(1.0))
        } else {
            let left = stage_x.floor().max(0.0);
            let top = stage_y.floor().max(0.0);
            let right = (stage_x + stage.w).ceil().min(win_w);
            let bottom = (stage_y + stage.h).ceil().min(win_h);
            (left, top, (right - left).max(1.0), (bottom - top).max(1.0))
        };

        let dw = (vw * scale).round().max(1.0) as u32;
        let dh = (vh * scale).round().max(1.0) as u32;

        let panel_frame = g.get_panel_frame();
        let panel_rect = if at == Where::Panel && panel_frame.width > 0.0 && panel_frame.height > 0.0 {
            [panel_frame.x, panel_frame.y, panel_frame.width, panel_frame.height]
        } else {
            [0.0; 4]
        };
        let panel_radius = t.get_radius_card();

        self.sync_backdrop(app, win_w, win_h);
        self.sync_layout(win_px.width, scale, vx, vy, vw, vh, dw, dh, panel_rect, panel_radius);
        // Keep the NSWindow opaque while the Metal underlay is active: `scene` and `viz_layer`
        // are opaque, so WindowServer stays on its opaque-window fast path instead of blending
        // the desktop behind us or recomputing dynamic alpha window shadows every frame.
        keep_window_opaque(app);

        let bd = app.global::<Backdrop>();
        let bg = t.get_bg();
        let tint = t.get_backdrop_tint();
        let backdrop_opacity = t.get_backdrop_opacity();
        let backdrop_visible = bd.get_visible();

        let has_cover = p.get_has_cover();
        if look.style.around() {
            if has_cover {
                let img = p.get_full();
                self.sync_cover_texture(img);
            } else {
                self.cover_image = None;
                self.cover_tex = None;
            }
        }

        let refl_opacity = if stage.refl_room > 0.0 && t.get_glass() && !p.get_bar_up() {
            (1.0 - f32::from(t.get_glass_tint().alpha()) / 255.0).max(0.2)
        } else {
            1.0
        };
        let square = g.get_square();
        let radius_card = t.get_radius_card();

        let drawable = self.viz_layer.as_ref().and_then(|l| l.nextDrawable());
        let Some(drawable) = drawable else {
            return self.active;
        };
        let target = drawable.texture();

        let render = RenderParams {
            look,
            stage,
            at,
            frame,
            stage_x,
            stage_y,
            vx,
            vy,
            vw,
            vh,
            win_w,
            win_h,
            dw,
            dh,
            scale,
            square,
            radius_card,
            has_cover,
            refl_opacity,
            bg,
            tint,
            backdrop_opacity,
            backdrop_visible,
            panel_rect,
            panel_radius,
        };
        if !self.encode_and_submit(&target, Some(&drawable), &render) {
            return false;
        }

        if !self.active {
            self.active = true;
            g.set_native_underlay(true);
            let weak = app.as_weak();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(app) = weak.upgrade() {
                    keep_window_opaque(&app);
                }
            });
        }

        #[cfg(feature = "bench")]
        {
            self.last = Some(LastRender {
                frame: frame.clone(),
                look: look.clone(),
                stage: *stage,
                at,
                stage_x,
                stage_y,
                vx,
                vy,
                vw,
                vh,
                win_w,
                win_h,
                dw,
                dh,
                scale,
                square,
                radius_card,
                has_cover,
                refl_opacity,
                bg,
                tint,
                backdrop_opacity,
                backdrop_visible,
                panel_rect,
                panel_radius,
            });
        }

        true
    }

    /// Bench helper: composite backdrop + Metal visualizer underlay behind Slint's transparent
    /// foreground snapshot so `MP_BENCH` PNG captures match what Core Animation shows on screen.
    #[cfg(feature = "bench")]
    pub(crate) fn composite_snapshot(&mut self, app: &AppWindow, fg: &mut image::RgbaImage) {
        let Some(last) = self.last.clone() else { return };
        let (w, h) = (fg.width(), fg.height());
        if w == 0 || h == 0 {
            return;
        }

        let t = app.global::<Theme>();
        let bd = app.global::<Backdrop>();
        let mut out = render_backdrop_rgba(
            w,
            h,
            t.get_bg(),
            t.get_backdrop_tint(),
            t.get_backdrop_opacity(),
            bd.get_visible(),
            bd.get_visible().then(|| bd.get_image()),
            last.panel_rect,
            last.panel_radius,
            last.scale,
        );

        // Render the exact opaque Metal frame into a CPU-readable Shared BGRA8 texture.
        if let Some(viz_bgra) = self.render_offscreen(&last) {
            let ox = (last.vx * last.scale).round() as i32;
            let oy = (last.vy * last.scale).round() as i32;
            let vw = last.dw as i32;
            let vh = last.dh as i32;
            for y in 0..vh {
                let dy = oy + y;
                if dy < 0 || dy >= h as i32 {
                    continue;
                }
                for x in 0..vw {
                    let dx = ox + x;
                    if dx < 0 || dx >= w as i32 {
                        continue;
                    }
                    let si = ((y as u32 * last.dw + x as u32) * 4) as usize;
                    let dst = out.get_pixel_mut(dx as u32, dy as u32);
                    dst[0] = viz_bgra[si + 2];
                    dst[1] = viz_bgra[si + 1];
                    dst[2] = viz_bgra[si];
                    dst[3] = 255;
                }
            }
        }

        // Alpha-composite Slint's straight RGBA8 foreground over backdrop + Metal viz.
        for (dst, src) in out.pixels_mut().zip(fg.pixels()) {
            let sa = f32::from(src[3]) / 255.0;
            if sa <= 0.0 {
                continue;
            }
            if sa >= 1.0 {
                *dst = *src;
                continue;
            }
            let inv = 1.0 - sa;
            dst[0] = (f32::from(src[0]) * sa + f32::from(dst[0]) * inv).round().clamp(0.0, 255.0) as u8;
            dst[1] = (f32::from(src[1]) * sa + f32::from(dst[1]) * inv).round().clamp(0.0, 255.0) as u8;
            dst[2] = (f32::from(src[2]) * sa + f32::from(dst[2]) * inv).round().clamp(0.0, 255.0) as u8;
            dst[3] = 255;
        }
        *fg = out;
    }

    #[cfg(feature = "bench")]
    fn render_offscreen(&mut self, last: &LastRender) -> Option<Vec<u8>> {
        let ctx = self.ctx.as_ref()?;
        // SAFETY: valid 2D BGRA8Unorm texture descriptor.
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::BGRA8Unorm,
                last.dw as usize,
                last.dh as usize,
                false,
            )
        };
        desc.setStorageMode(MTLStorageMode::Shared);
        desc.setUsage(MTLTextureUsage::RenderTarget);
        let tex = ctx.device.newTextureWithDescriptor(&desc)?;

        let render = RenderParams {
            look: &last.look,
            stage: &last.stage,
            at: last.at,
            frame: &last.frame,
            stage_x: last.stage_x,
            stage_y: last.stage_y,
            vx: last.vx,
            vy: last.vy,
            vw: last.vw,
            vh: last.vh,
            win_w: last.win_w,
            win_h: last.win_h,
            dw: last.dw,
            dh: last.dh,
            scale: last.scale,
            square: last.square,
            radius_card: last.radius_card,
            has_cover: last.has_cover,
            refl_opacity: last.refl_opacity,
            bg: last.bg,
            tint: last.tint,
            backdrop_opacity: last.backdrop_opacity,
            backdrop_visible: last.backdrop_visible,
            panel_rect: last.panel_rect,
            panel_radius: last.panel_radius,
        };
        if !self.encode_and_submit(&tex, None, &render) {
            return None;
        }

        let bpr = (last.dw as usize) * 4;
        let mut pixels = vec![0u8; bpr * (last.dh as usize)];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize { width: last.dw as usize, height: last.dh as usize, depth: 1 },
        };
        // SAFETY: `pixels` holds `bpr * dh` bytes matching `region` at mip level 0.
        unsafe {
            tex.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                NonNull::new(pixels.as_mut_ptr().cast())?,
                bpr,
                region,
                0,
            );
        }
        Some(pixels)
    }

    fn ensure_ctx(&mut self) -> bool {
        if self.ctx.is_some() {
            return true;
        }
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            log::warn!("visualizer metal: no default Metal device");
            self.init_failed = true;
            return false;
        };
        let Some(queue) = device.newCommandQueue() else {
            log::warn!("visualizer metal: failed to create command queue");
            self.init_failed = true;
            return false;
        };
        let src = NSString::from_str(MSL_SOURCE);
        let library = match device.newLibraryWithSource_options_error(&src, None) {
            Ok(lib) => lib,
            Err(err) => {
                log::warn!("visualizer metal: shader compile failed: {err:?}");
                self.init_failed = true;
                return false;
            }
        };
        let Some(pipes) = build_pipelines(&device, &library) else {
            self.init_failed = true;
            return false;
        };
        let sdesc = MTLSamplerDescriptor::new();
        sdesc.setMinFilter(MTLSamplerMinMagFilter::Linear);
        sdesc.setMagFilter(MTLSamplerMinMagFilter::Linear);
        sdesc.setSAddressMode(MTLSamplerAddressMode::ClampToEdge);
        sdesc.setTAddressMode(MTLSamplerAddressMode::ClampToEdge);
        let Some(sampler) = device.newSamplerStateWithDescriptor(&sdesc) else {
            self.init_failed = true;
            return false;
        };

        let ds_none_desc = MTLDepthStencilDescriptor::new();
        let Some(ds_none) = device.newDepthStencilStateWithDescriptor(&ds_none_desc) else {
            self.init_failed = true;
            return false;
        };
        let st_desc = MTLStencilDescriptor::new();
        st_desc.setStencilCompareFunction(MTLCompareFunction::Equal);
        st_desc.setStencilFailureOperation(MTLStencilOperation::Keep);
        st_desc.setDepthFailureOperation(MTLStencilOperation::Keep);
        st_desc.setDepthStencilPassOperation(MTLStencilOperation::IncrementClamp);
        let ds_once_desc = MTLDepthStencilDescriptor::new();
        ds_once_desc.setFrontFaceStencil(Some(&st_desc));
        ds_once_desc.setBackFaceStencil(Some(&st_desc));
        let Some(ds_once) = device.newDepthStencilStateWithDescriptor(&ds_once_desc) else {
            self.init_failed = true;
            return false;
        };
        let memoryless_stencil = device.supportsFamily(MTLGPUFamily::Apple1);

        self.ctx = Some(MetalCtx {
            device,
            queue,
            sampler,
            pipes,
            ds_none,
            ds_once,
            memoryless_stencil,
        });
        true
    }

    fn ensure_attached(&mut self, app: &AppWindow) -> bool {
        let Some(root) = app_root_layer(app) else {
            return false;
        };
        if let (Some(scene), Some(fg)) = (&self.scene, &self.foreground)
            && scene.superlayer().as_deref() == Some(&*root)
            && fg.superlayer().as_deref() == Some(&*root)
        {
            return true;
        }
        // Detach stale layers before reattaching.
        if let Some(old) = self.scene.take() {
            CATransaction::begin();
            CATransaction::setDisableActions(true);
            old.removeFromSuperlayer();
            CATransaction::commit();
        }
        self.foreground = None;
        self.panel_layer = None;
        self.viz_layer = None;
        self.stencil_tex = None;
        self.backdrop_state = None;
        self.art_tex = None;
        self.layout_state = None;
        self.cover_image = None;
        self.cover_tex = None;

        let Some(fg) = find_slint_foreground(&root) else {
            return false;
        };
        let Some(ctx) = &self.ctx else {
            return false;
        };

        CATransaction::begin();
        CATransaction::setDisableActions(true);

        // Flat, minimal Core Animation tree under `foreground`:
        // `scene` holds the pre-baked static backdrop CGImage (art + tint + bg),
        // `panel` (hidden except on Panel tab), and `viz` (opaque CAMetalLayer).
        let scene = CALayer::layer();
        scene.setGeometryFlipped(fg.isGeometryFlipped());
        scene.setMasksToBounds(true);
        scene.setOpaque(true);
        // SAFETY: Apple CoreAnimation constant kCAGravityResize.
        scene.setContentsGravity(unsafe { kCAGravityResize });
        scene.setFrame(root.bounds());

        let panel = CALayer::layer();
        let panel_color = CGColor::new_srgb(0.0, 0.0, 0.0, 64.0 / 255.0);
        panel.setBackgroundColor(Some(&panel_color));
        panel.setHidden(true);
        scene.addSublayer(&panel);

        let viz = CAMetalLayer::layer();
        viz.setDevice(Some(&*ctx.device));
        viz.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
        viz.setOpaque(true);
        viz.setFramebufferOnly(true);
        viz.setMaximumDrawableCount(2);
        viz.setDisplaySyncEnabled(true);
        scene.addSublayer(&viz);

        root.insertSublayer_below(&scene, Some(&fg));
        CATransaction::commit();

        self.scene = Some(scene);
        self.foreground = Some(fg);
        self.panel_layer = Some(panel);
        self.viz_layer = Some(viz);
        true
    }

    fn sync_backdrop(&mut self, app: &AppWindow, win_w: f32, win_h: f32) {
        let t = app.global::<Theme>();
        let bd = app.global::<Backdrop>();
        let visible = bd.get_visible();
        let aspect = ((win_w / win_h.max(1.0)) * 1000.0).round() as u32;
        let state = BackdropState {
            bg: t.get_bg(),
            tint: t.get_backdrop_tint(),
            opacity: t.get_backdrop_opacity(),
            visible,
            image: visible.then(|| bd.get_image()),
            win_aspect_milli: aspect,
        };
        if self.backdrop_state.as_ref() == Some(&state) {
            return;
        }
        let Some(scene) = &self.scene else {
            return;
        };

        let bg_cg = cg_color(state.bg);
        let baked = bake_backdrop_cgimage(
            win_w,
            win_h,
            state.bg,
            state.tint,
            state.opacity,
            state.visible,
            state.image.as_ref(),
        );

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        scene.setBackgroundColor(Some(&bg_cg));
        if let Some(img) = baked {
            // SAFETY: CGImage is toll-free bridged to an Objective-C object for CALayer.contents.
            unsafe {
                let obj: &AnyObject =
                    &*(objc2_core_foundation::CFRetained::as_ptr(&img).as_ptr().cast());
                scene.setContents(Some(obj));
            }
        } else {
            // SAFETY: passing None clears CALayer.contents.
            unsafe {
                scene.setContents(None);
            }
        }
        CATransaction::commit();

        self.art_tex = state.image.as_ref().and_then(|img| {
            let ctx = self.ctx.as_ref()?;
            upload_rgba_texture(&ctx.device, img)
        });
        self.backdrop_state = Some(state);
    }

    #[allow(clippy::too_many_arguments)]
    fn sync_layout(
        &mut self,
        win_px_w: u32,
        scale: f32,
        vx: f32,
        vy: f32,
        vw: f32,
        vh: f32,
        dw: u32,
        dh: u32,
        panel_rect: [f32; 4],
        panel_radius: f32,
    ) {
        let (Some(scene), Some(panel), Some(viz)) = (&self.scene, &self.panel_layer, &self.viz_layer)
        else {
            return;
        };
        let Some(root) = scene.superlayer() else {
            return;
        };
        let bounds = root.bounds();
        let points = bounds.size.width * f64::from(scale) / f64::from(win_px_w.max(1));
        if !points.is_finite() || points <= 0.0 {
            return;
        }
        let next = LayoutState {
            root_w: bounds.size.width,
            root_h: bounds.size.height,
            points,
            scale,
            vx,
            vy,
            vw,
            vh,
            dw,
            dh,
            panel_rect,
            panel_radius,
        };
        if self.layout_state.as_ref() == Some(&next) {
            return;
        }

        CATransaction::begin();
        CATransaction::setDisableActions(true);
        scene.setFrame(bounds);
        if panel_rect[2] > 0.0 && panel_rect[3] > 0.0 {
            panel.setHidden(false);
            panel.setFrame(CGRect::new(
                CGPoint::new(f64::from(panel_rect[0]) * points, f64::from(panel_rect[1]) * points),
                CGSize::new(f64::from(panel_rect[2]) * points, f64::from(panel_rect[3]) * points),
            ));
            panel.setCornerRadius(f64::from(panel_radius) * points);
        } else {
            panel.setHidden(true);
        }
        viz.setFrame(CGRect::new(
            CGPoint::new(f64::from(vx) * points, f64::from(vy) * points),
            CGSize::new(f64::from(vw) * points, f64::from(vh) * points),
        ));
        viz.setDrawableSize(CGSize::new(f64::from(dw), f64::from(dh)));
        viz.setContentsScale(f64::from(scale) / points);
        CATransaction::commit();
        self.layout_state = Some(next);
    }

    fn sync_cover_texture(&mut self, img: Image) {
        if self.cover_image.as_ref() == Some(&img) && self.cover_tex.is_some() {
            return;
        }
        let Some(ctx) = &self.ctx else { return };
        self.cover_tex = upload_rgba_texture(&ctx.device, &img).map(|(_, _, t)| t);
        self.cover_image = Some(img);
    }

    fn ensure_stencil_tex(&mut self, w: u32, h: u32) -> Option<Retained<ProtocolObject<dyn MTLTexture>>> {
        if let Some((tw, th, ref tex)) = self.stencil_tex
            && tw == w
            && th == h
        {
            return Some(tex.clone());
        }
        let ctx = self.ctx.as_ref()?;
        // SAFETY: valid 2D Stencil8 texture descriptor.
        let desc = unsafe {
            MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
                MTLPixelFormat::Stencil8,
                w as usize,
                h as usize,
                false,
            )
        };
        desc.setStorageMode(if ctx.memoryless_stencil {
            MTLStorageMode::Memoryless
        } else {
            MTLStorageMode::Private
        });
        desc.setUsage(MTLTextureUsage::RenderTarget);
        let tex = ctx.device.newTextureWithDescriptor(&desc)?;
        self.stencil_tex = Some((w, h, tex.clone()));
        Some(tex)
    }

    fn encode_and_submit(
        &mut self,
        target: &ProtocolObject<dyn MTLTexture>,
        present_drawable: Option<&ProtocolObject<dyn CAMetalDrawable>>,
        r: &RenderParams<'_>,
    ) -> bool {
        // ---- Stage all geometry into one linear staging buffer ----
        self.staging.reset();
        let mut cmds: Vec<DrawCmd> = Vec::with_capacity(24);
        let light = if r.frame.light.abs() < 0.01 { 0.0 } else { r.frame.light.clamp(-1.0, 1.0) };
        let fill_alpha = (1.0 + light).clamp(0.0, 1.5);
        let refl_alpha = (fill_alpha * r.refl_opacity).clamp(0.0, 1.5);
        let aa_pad = (1.0 / r.scale).max(0.75);

        // 1. Bloom (tight bounding quad around the cover where bloom alpha > 0)
        if r.look.style.around() && self.brushes.bloom.stop_count > 0 {
            let h = draw::halo(r.stage);
            let reach = h.hub + h.gap + h.travel + 1.0;
            let bloom_r = h.hub + (reach - h.hub) * 0.62;
            let pulse = 0.4 + 0.6 * r.frame.pulse.clamp(0.0, 1.0);
            let q = QuadInstance {
                rect: [h.cx - bloom_r, h.cy - bloom_r, bloom_r * 2.0, bloom_r * 2.0],
                top_radius: 0.0,
                crisp: 0.0,
                _pad: [0.0; 2],
            };
            let off = self.staging.push_slice(&[q]);
            let mut b = self.brushes.bloom;
            b.alpha_mul = pulse;
            cmds.push(DrawCmd::Quads { offset: off, count: 1, brush: b });
        }

        // 2. Fill: bars, open curve area, or ring corona
        if !r.frame.bars.is_empty() {
            let crisp = if r.look.style == Style::Bars { 1.0 } else { 0.0 };
            let quads: Vec<QuadInstance> = r
                .frame
                .bars
                .iter()
                .map(|b| QuadInstance {
                    rect: [b.x, b.y, b.w, b.h],
                    top_radius: b.top_radius,
                    crisp,
                    _pad: [0.0; 2],
                })
                .collect();
            let off = self.staging.push_slice(&quads);
            let mut b = self.brushes.fill;
            b.alpha_mul = fill_alpha;
            cmds.push(DrawCmd::Quads { offset: off, count: quads.len(), brush: b });
        }

        if let Some(oc) = &r.frame.open_curve {
            flatten_open_curve(&mut self.staging.poly_pts, oc);
            build_area_verts(&mut self.staging.poly_verts, &self.staging.poly_pts, oc.baseline, r.stage.w, aa_pad);
            if !self.staging.poly_verts.is_empty() {
                let off = self.staging.push_poly_verts();
                let mut b = self.brushes.fill;
                b.alpha_mul = fill_alpha;
                cmds.push(DrawCmd::Poly {
                    offset: off,
                    count: self.staging.poly_verts.len(),
                    brush: b,
                });
            }
        }

        if !r.frame.ring_contour.is_empty() {
            if let Some(inner) = &r.frame.corona_inner {
                build_corona_verts(
                    &mut self.staging.poly_verts,
                    &mut self.staging.poly_pts,
                    &r.frame.ring_contour,
                    inner,
                    &mut self.staging.inner_pts,
                    aa_pad,
                );
                if !self.staging.poly_verts.is_empty() {
                    let off = self.staging.push_poly_verts();
                    let mut b = self.brushes.fill;
                    b.alpha_mul = fill_alpha;
                    cmds.push(DrawCmd::Poly {
                        offset: off,
                        count: self.staging.poly_verts.len(),
                        brush: b,
                    });
                }
            } else {
                flatten_closed_curve(&mut self.staging.poly_pts, &r.frame.ring_contour);
            }
        }

        // 3. Glow & 4. Line (wide translucent glow uses stencil-deduplicated analytic capsules
        //    so sharp concave bends never bowtie or double-blend; thin lines use miter ribbons).
        let has_line_pts = !self.staging.poly_pts.is_empty();
        let closed_line = !r.frame.ring_contour.is_empty();
        if has_line_pts {
            if self.brushes.glow_width > 0.0 && self.brushes.glow.stop_count > 0 {
                let gw = self.brushes.glow_width * (1.0 + r.frame.glow.clamp(0.0, 1.0));
                let caps = build_capsules(&self.staging.poly_pts, gw * 0.5, closed_line);
                if !caps.is_empty() {
                    let off = self.staging.push_slice(&caps);
                    cmds.push(DrawCmd::Capsule {
                        offset: off,
                        count: caps.len(),
                        brush: self.brushes.glow,
                    });
                }
            }
            if self.brushes.line_width > 0.0 && self.brushes.line.stop_count > 0 {
                let swell = if r.frame.swell < 0.02 { 1.0 } else { 1.0 + r.frame.swell.min(1.0) };
                let lw = self.brushes.line_width * swell;
                self.staging.stroke_verts.clear();
                append_stroke_verts(&mut self.staging.stroke_verts, &self.staging.poly_pts, lw * 0.5, aa_pad, closed_line);
                if !self.staging.stroke_verts.is_empty() {
                    let off = self.staging.push_stroke_verts();
                    cmds.push(DrawCmd::Stroke {
                        offset: off,
                        count: self.staging.stroke_verts.len(),
                        brush: self.brushes.line,
                    });
                }
            }
        }

        // 5. Caps
        if !r.frame.caps.is_empty() {
            let quads: Vec<QuadInstance> = r
                .frame
                .caps
                .iter()
                .map(|c| QuadInstance {
                    rect: [c.x, c.y, c.w, c.h],
                    top_radius: 0.0,
                    crisp: 1.0,
                    _pad: [0.0; 2],
                })
                .collect();
            let off = self.staging.push_slice(&quads);
            cmds.push(DrawCmd::Quads { offset: off, count: quads.len(), brush: self.brushes.caps });
        }

        // 6. Reflection (bars and/or curve strips, shifted by +frame.top_h)
        if r.frame.refl_h > 1.0 {
            let y_off = r.frame.top_h;
            for k in 0..STRIPS {
                if !r.frame.refl_bars[k].is_empty() {
                    let quads: Vec<QuadInstance> = r.frame.refl_bars[k]
                        .iter()
                        .map(|q: &BarQuad| QuadInstance {
                            rect: [q.x, q.y + y_off, q.w, q.h],
                            top_radius: 0.0,
                            crisp: 1.0,
                            _pad: [0.0; 2],
                        })
                        .collect();
                    let off = self.staging.push_slice(&quads);
                    let mut b = self.brushes.refl[k];
                    b.alpha_mul = refl_alpha;
                    cmds.push(DrawCmd::Quads { offset: off, count: quads.len(), brush: b });
                }
                if let Some(rc) = &r.frame.refl_curves[k] {
                    flatten_refl_curve(&mut self.staging.poly_pts, rc, y_off);
                    build_area_verts(
                        &mut self.staging.poly_verts,
                        &self.staging.poly_pts,
                        rc.top_y + y_off,
                        r.stage.w,
                        aa_pad,
                    );
                    if !self.staging.poly_verts.is_empty() {
                        let off = self.staging.push_poly_verts();
                        let mut b = self.brushes.refl[k];
                        b.alpha_mul = refl_alpha;
                        cmds.push(DrawCmd::Poly {
                            offset: off,
                            count: self.staging.poly_verts.len(),
                            brush: b,
                        });
                    }
                }
            }
        }

        // 7. Ripples
        if r.look.style.around() && !r.frame.ripples.is_empty() && self.brushes.ripple_width > 0.0 {
            self.staging.stroke_verts.clear();
            let hw = self.brushes.ripple_width * 0.5;
            for rip in &r.frame.ripples {
                flatten_closed_curve(&mut self.staging.poly_pts, rip);
                append_stroke_verts(&mut self.staging.stroke_verts, &self.staging.poly_pts, hw, aa_pad, true);
            }
            if !self.staging.stroke_verts.is_empty() {
                let off = self.staging.push_stroke_verts();
                cmds.push(DrawCmd::Stroke {
                    offset: off,
                    count: self.staging.stroke_verts.len(),
                    brush: self.brushes.ripples,
                });
            }
        }

        // 8. Snare rings
        if r.look.style.around() && !r.frame.rings.is_empty() && self.brushes.ring_width > 0.0 {
            self.staging.stroke_verts.clear();
            let hw = self.brushes.ring_width * 0.5;
            for rng in &r.frame.rings {
                flatten_closed_curve(&mut self.staging.poly_pts, rng);
                append_stroke_verts(&mut self.staging.stroke_verts, &self.staging.poly_pts, hw, aa_pad, true);
            }
            if !self.staging.stroke_verts.is_empty() {
                let off = self.staging.push_stroke_verts();
                cmds.push(DrawCmd::Stroke {
                    offset: off,
                    count: self.staging.stroke_verts.len(),
                    brush: self.brushes.rings,
                });
            }
        }

        // 9. Sparks behind cover
        push_dots(&mut self.staging, &mut cmds, &r.frame.sparks_glow, self.brushes.sparks_glow);
        push_dots(&mut self.staging, &mut cmds, &r.frame.sparks_old, self.brushes.sparks_old);
        push_dots(&mut self.staging, &mut cmds, &r.frame.sparks, self.brushes.sparks);

        // 10. Ring cover shadow + cover image
        if r.look.style.around() {
            let h = draw::halo(r.stage);
            let cover_scale = if r.look.cover_pulse {
                1.0 + crate::visualizer::COVER_PULSE * r.frame.punch.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let (blur, off_y, alpha) = if r.at == Where::Behind {
                (40.0, 12.0, 192.0 / 255.0)
            } else {
                (24.0, 4.0, 144.0 / 255.0)
            };
            let sh_hub = if r.look.cover_pulse { h.hub } else { h.hub * cover_scale };
            let sh_rad = if r.square { r.radius_card.min(sh_hub) } else { sh_hub };
            cmds.push(DrawCmd::Shadow(ShadowUniforms {
                center: [h.cx, h.cy + off_y],
                half_size: [sh_hub, sh_hub],
                radius: sh_rad,
                sigma: blur * 0.5,
                alpha,
                _pad: 0.0,
            }));

            let cv_hub = h.hub * cover_scale;
            let cv_rad = if r.square { r.radius_card.min(cv_hub) } else { cv_hub };
            cmds.push(DrawCmd::Cover(CoverUniforms {
                center: [h.cx, h.cy],
                half_size: [cv_hub, cv_hub],
                radius: cv_rad,
                has_cover: if r.has_cover && self.cover_tex.is_some() { 1.0 } else { 0.0 },
                _pad: [0.0; 2],
            }));
        }

        // 11. Front grains (Saturn near side, in front of the cover)
        push_dots(&mut self.staging, &mut cmds, &r.frame.front_glow, self.brushes.sparks_glow);
        push_dots(&mut self.staging, &mut cmds, &r.frame.front, self.brushes.sparks);

        // ---- Upload staged bytes into ring buffer ----
        let stencil_tex = self.ensure_stencil_tex(r.dw, r.dh);
        let ctx = match &self.ctx {
            Some(c) => c,
            None => return false,
        };
        let slot = self.ring_idx % RING_BUFFERS;
        self.ring_idx = self.ring_idx.wrapping_add(1);
        let needed = self.staging.bytes.len().max(256);
        let need_alloc = self.ring[slot].as_ref().is_none_or(|b| b.length() < needed);
        if need_alloc {
            let cap = needed.next_power_of_two().max(64 * 1024);
            let Some(buf) = ctx.device.newBufferWithLength_options(cap, MTLResourceOptions::StorageModeShared) else {
                return false;
            };
            self.ring[slot] = Some(buf);
        }
        let geom_buf = self.ring[slot].as_ref().unwrap();
        if !self.staging.bytes.is_empty() {
            // SAFETY: geom_buf is StorageModeShared with capacity >= staging.bytes.len().
            unsafe {
                std::ptr::copy_nonoverlapping(
                    self.staging.bytes.as_ptr(),
                    geom_buf.contents().as_ptr().cast::<u8>(),
                    self.staging.bytes.len(),
                );
            }
        }

        // ---- Encode single-pass 1x Metal render pass ----
        let Some(cmd_buf) = ctx.queue.commandBuffer() else {
            return false;
        };
        let pass = MTLRenderPassDescriptor::renderPassDescriptor();
        // SAFETY: attachment 0 always exists on a newly created render pass descriptor.
        let att = unsafe { pass.colorAttachments().objectAtIndexedSubscript(0) };
        let bg_c = rgba_f32(r.bg);
        att.setLoadAction(MTLLoadAction::DontCare);
        att.setClearColor(MTLClearColor {
            red: f64::from(bg_c[0]),
            green: f64::from(bg_c[1]),
            blue: f64::from(bg_c[2]),
            alpha: 1.0,
        });
        att.setTexture(Some(target));
        att.setStoreAction(MTLStoreAction::Store);
        if let Some(st) = &stencil_tex {
            let st_att = pass.stencilAttachment();
            st_att.setTexture(Some(st));
            st_att.setLoadAction(MTLLoadAction::Clear);
            st_att.setClearStencil(0);
            st_att.setStoreAction(MTLStoreAction::DontCare);
        }

        let Some(enc) = cmd_buf.renderCommandEncoderWithDescriptor(&pass) else {
            return false;
        };
        enc.setViewport(MTLViewport {
            originX: 0.0,
            originY: 0.0,
            width: f64::from(r.dw),
            height: f64::from(r.dh),
            znear: 0.0,
            zfar: 1.0,
        });

        // 0. Draw opaque backdrop (Theme.bg + art + tint + optional panel) covering the entire
        // drawable, so LoadAction::DontCare avoids redundant tile clearing and `viz_layer` stays opaque.
        let (art_origin, art_size, has_art) = if let Some((aw, ah, _)) = &self.art_tex {
            let sw = *aw as f32;
            let sh = *ah as f32;
            let sfill = (r.win_w / sw).max(r.win_h / sh);
            let rw = sw * sfill;
            let rh = sh * sfill;
            ([(r.win_w - rw) * 0.5, (r.win_h - rh) * 0.5], [rw, rh], 1.0)
        } else {
            ([0.0, 0.0], [1.0, 1.0], 0.0)
        };
        let bd_u = BackdropUniforms {
            bg: bg_c,
            tint: rgba_f32(r.tint),
            panel_rect: r.panel_rect,
            layer_origin: [r.vx, r.vy],
            layer_size: [r.vw, r.vh],
            win_size: [r.win_w, r.win_h],
            art_origin,
            art_size,
            opacity: r.backdrop_opacity,
            visible: if r.backdrop_visible { 1.0 } else { 0.0 },
            has_art,
            panel_radius: r.panel_radius,
        };
        let pipes = &ctx.pipes;

        // SAFETY: uniform structs and vertex buffer ranges match MSL bindings.
        unsafe {
            enc.setDepthStencilState(Some(&ctx.ds_none));
            enc.setRenderPipelineState(&pipes.backdrop);
            enc.setVertexBytes_length_atIndex(
                NonNull::from(&bd_u).cast(),
                std::mem::size_of::<BackdropUniforms>(),
                0,
            );
            enc.setFragmentBytes_length_atIndex(
                NonNull::from(&bd_u).cast(),
                std::mem::size_of::<BackdropUniforms>(),
                0,
            );
            if let Some((_, _, tex)) = &self.art_tex {
                enc.setFragmentTexture_atIndex(Some(tex), 0);
            }
            enc.setFragmentSamplerState_atIndex(Some(&ctx.sampler), 0);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);

            let vp = ViewportUniforms {
                stage_offset: [r.stage_x - r.vx, r.stage_y - r.vy],
                layer_size: [r.vw, r.vh],
                scale: r.scale,
                _pad: 0.0,
            };
            enc.setVertexBytes_length_atIndex(
                NonNull::from(&vp).cast(),
                std::mem::size_of::<ViewportUniforms>(),
                0,
            );
            enc.setFragmentBytes_length_atIndex(
                NonNull::from(&vp).cast(),
                std::mem::size_of::<ViewportUniforms>(),
                0,
            );

            for cmd in &cmds {
                match cmd {
                    DrawCmd::Quads { offset, count, brush } => {
                        enc.setRenderPipelineState(&pipes.quad);
                        enc.setVertexBuffer_offset_atIndex(Some(geom_buf), *offset, 1);
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(brush).cast(),
                            std::mem::size_of::<BrushUniforms>(),
                            1,
                        );
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            *count,
                        );
                    }
                    DrawCmd::Dots { offset, count, brush } => {
                        enc.setRenderPipelineState(&pipes.dot);
                        enc.setVertexBuffer_offset_atIndex(Some(geom_buf), *offset, 1);
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(brush).cast(),
                            std::mem::size_of::<BrushUniforms>(),
                            1,
                        );
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            *count,
                        );
                    }
                    DrawCmd::Poly { offset, count, brush } => {
                        enc.setRenderPipelineState(&pipes.poly);
                        enc.setVertexBuffer_offset_atIndex(Some(geom_buf), *offset, 1);
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(brush).cast(),
                            std::mem::size_of::<BrushUniforms>(),
                            1,
                        );
                        enc.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            *count,
                        );
                    }
                    DrawCmd::Stroke { offset, count, brush } => {
                        enc.setRenderPipelineState(&pipes.stroke);
                        enc.setVertexBuffer_offset_atIndex(Some(geom_buf), *offset, 1);
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(brush).cast(),
                            std::mem::size_of::<BrushUniforms>(),
                            1,
                        );
                        enc.drawPrimitives_vertexStart_vertexCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            *count,
                        );
                    }
                    DrawCmd::Capsule { offset, count, brush } => {
                        enc.setDepthStencilState(Some(&ctx.ds_once));
                        enc.setStencilReferenceValue(0);
                        enc.setVertexBuffer_offset_atIndex(Some(geom_buf), *offset, 1);
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(brush).cast(),
                            std::mem::size_of::<BrushUniforms>(),
                            1,
                        );
                        // Pass A: solid interior (cov >= 0.99), each pixel blended at most once.
                        enc.setRenderPipelineState(&pipes.capsule_core);
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            *count,
                        );
                        // Pass B: 1px outer AA fringe only where stencil is still 0.
                        enc.setRenderPipelineState(&pipes.capsule_aa);
                        enc.drawPrimitives_vertexStart_vertexCount_instanceCount(
                            MTLPrimitiveType::Triangle,
                            0,
                            6,
                            *count,
                        );
                        enc.setDepthStencilState(Some(&ctx.ds_none));
                    }
                    DrawCmd::Shadow(sh) => {
                        enc.setRenderPipelineState(&pipes.shadow);
                        enc.setVertexBytes_length_atIndex(
                            NonNull::from(sh).cast(),
                            std::mem::size_of::<ShadowUniforms>(),
                            1,
                        );
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(sh).cast(),
                            std::mem::size_of::<ShadowUniforms>(),
                            1,
                        );
                        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
                    }
                    DrawCmd::Cover(cv) => {
                        enc.setRenderPipelineState(&pipes.cover);
                        enc.setVertexBytes_length_atIndex(
                            NonNull::from(cv).cast(),
                            std::mem::size_of::<CoverUniforms>(),
                            1,
                        );
                        enc.setFragmentBytes_length_atIndex(
                            NonNull::from(cv).cast(),
                            std::mem::size_of::<CoverUniforms>(),
                            1,
                        );
                        if let Some(tex) = &self.cover_tex {
                            enc.setFragmentTexture_atIndex(Some(tex), 0);
                        }
                        enc.setFragmentSamplerState_atIndex(Some(&ctx.sampler), 0);
                        enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 6);
                    }
                }
            }
        }
        enc.endEncoding();

        if let Some(drawable) = present_drawable {
            cmd_buf.presentDrawable(ProtocolObject::from_ref(drawable));
            cmd_buf.commit();
        } else {
            cmd_buf.commit();
            cmd_buf.waitUntilCompleted();
        }
        true
    }
}

struct RenderParams<'a> {
    look: &'a Look,
    stage: &'a Stage,
    at: Where,
    frame: &'a NativeFrame,
    stage_x: f32,
    stage_y: f32,
    vx: f32,
    vy: f32,
    vw: f32,
    vh: f32,
    win_w: f32,
    win_h: f32,
    dw: u32,
    dh: u32,
    scale: f32,
    square: bool,
    radius_card: f32,
    has_cover: bool,
    refl_opacity: f32,
    bg: Color,
    tint: Color,
    backdrop_opacity: f32,
    backdrop_visible: bool,
    panel_rect: [f32; 4],
    panel_radius: f32,
}

enum DrawCmd {
    Quads { offset: usize, count: usize, brush: BrushUniforms },
    Dots { offset: usize, count: usize, brush: BrushUniforms },
    Poly { offset: usize, count: usize, brush: BrushUniforms },
    Stroke { offset: usize, count: usize, brush: BrushUniforms },
    Capsule { offset: usize, count: usize, brush: BrushUniforms },
    Shadow(ShadowUniforms),
    Cover(CoverUniforms),
}

fn push_dots(staging: &mut Staging, cmds: &mut Vec<DrawCmd>, dots: &[Dot], brush: BrushUniforms) {
    if dots.is_empty() || brush.stop_count == 0 {
        return;
    }
    let insts: Vec<DotInstance> = dots
        .iter()
        .map(|d| DotInstance { center: [d.x, d.y], radius: d.r, _pad: 0.0 })
        .collect();
    let off = staging.push_slice(&insts);
    cmds.push(DrawCmd::Dots { offset: off, count: insts.len(), brush });
}

fn build_pipelines(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
) -> Option<Pipelines> {
    Some(Pipelines {
        backdrop: make_pipeline(device, library, "vs_backdrop", "fs_backdrop", false)?,
        quad: make_pipeline(device, library, "vs_quad", "fs_quad", true)?,
        dot: make_pipeline(device, library, "vs_dot", "fs_dot", true)?,
        poly: make_pipeline(device, library, "vs_poly", "fs_poly", true)?,
        stroke: make_pipeline(device, library, "vs_stroke", "fs_stroke", true)?,
        capsule_core: make_pipeline(device, library, "vs_capsule", "fs_capsule_core", true)?,
        capsule_aa: make_pipeline(device, library, "vs_capsule", "fs_capsule_aa", true)?,
        shadow: make_pipeline(device, library, "vs_shadow", "fs_shadow", true)?,
        cover: make_pipeline(device, library, "vs_cover", "fs_cover", true)?,
    })
}

fn make_pipeline(
    device: &ProtocolObject<dyn MTLDevice>,
    library: &ProtocolObject<dyn MTLLibrary>,
    vs_name: &str,
    fs_name: &str,
    blend: bool,
) -> Option<Retained<ProtocolObject<dyn MTLRenderPipelineState>>> {
    let vs = library.newFunctionWithName(&NSString::from_str(vs_name))?;
    let fs = library.newFunctionWithName(&NSString::from_str(fs_name))?;
    let desc = MTLRenderPipelineDescriptor::new();
    desc.setVertexFunction(Some(&vs));
    desc.setFragmentFunction(Some(&fs));
    desc.setRasterSampleCount(1);
    desc.setStencilAttachmentPixelFormat(MTLPixelFormat::Stencil8);
    // SAFETY: color attachment 0 always exists on a render pipeline descriptor.
    let att = unsafe { desc.colorAttachments().objectAtIndexedSubscript(0) };
    att.setPixelFormat(MTLPixelFormat::BGRA8Unorm);
    att.setBlendingEnabled(blend);
    if blend {
        att.setRgbBlendOperation(MTLBlendOperation::Add);
        att.setAlphaBlendOperation(MTLBlendOperation::Add);
        att.setSourceRGBBlendFactor(MTLBlendFactor::One);
        att.setDestinationRGBBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
        att.setSourceAlphaBlendFactor(MTLBlendFactor::One);
        att.setDestinationAlphaBlendFactor(MTLBlendFactor::OneMinusSourceAlpha);
    }
    match device.newRenderPipelineStateWithDescriptor_error(&desc) {
        Ok(p) => Some(p),
        Err(err) => {
            log::warn!("visualizer metal: pipeline {vs_name}/{fs_name} failed: {err:?}");
            None
        }
    }
}

// ---- Brush conversion -------------------------------------------------------------------

fn rgba_f32(c: Color) -> [f32; 4] {
    [
        f32::from(c.red()) / 255.0,
        f32::from(c.green()) / 255.0,
        f32::from(c.blue()) / 255.0,
        f32::from(c.alpha()) / 255.0,
    ]
}

fn pack_stops(u: &mut BrushUniforms, iter: impl Iterator<Item = (f32, Color)>) {
    let mut raw: Vec<(f32, Color)> = iter.collect();
    if raw.is_empty() {
        u.stop_count = 0;
        return;
    }
    if raw.len() > MAX_STOPS {
        let last = raw.len() - 1;
        raw = (0..MAX_STOPS)
            .map(|i| raw[(i * last) / (MAX_STOPS - 1)])
            .collect();
    }
    u.stop_count = raw.len() as u32;
    for (i, (pos, col)) in raw.into_iter().enumerate() {
        u.colors[i] = rgba_f32(col);
        u.stops[i >> 2][i & 3] = pos;
    }
}

fn convert_brush(brush: &Brush, box_rect: [f32; 4]) -> BrushUniforms {
    let mut u = BrushUniforms { box_rect, alpha_mul: 1.0, ..BrushUniforms::default() };
    match brush {
        Brush::SolidColor(c) => {
            u.kind = 0;
            u.stop_count = 1;
            u.colors[0] = rgba_f32(*c);
            u.stops[0][0] = 0.0;
        }
        Brush::LinearGradient(g) => {
            u.kind = 1;
            let angle = g.angle().rem_euclid(360.0);
            let rad = angle.to_radians();
            let dx = rad.sin();
            let dy = -rad.cos();
            let half = 0.5 * (dx.abs() + dy.abs()).max(1e-4);
            u.geom = [0.5 - dx * half, 0.5 - dy * half, 0.5 + dx * half, 0.5 + dy * half];
            pack_stops(&mut u, g.stops().map(|s| (s.position, s.color)));
        }
        Brush::RadialGradient(g) => {
            u.kind = 2;
            let (cx, cy) = g.explicit_center().unwrap_or((box_rect[0] + box_rect[2] * 0.5, box_rect[1] + box_rect[3] * 0.5));
            let (rx, ry) = g.explicit_radii().unwrap_or((box_rect[2] * 0.5, box_rect[3] * 0.5));
            u.geom = [cx, cy, rx.max(1e-3), ry.max(1e-3)];
            pack_stops(&mut u, g.stops().map(|s| (s.position, s.color)));
        }
        Brush::ConicGradient(g) => {
            u.kind = 3;
            let (cx, cy) = g.center_or_default(box_rect[0] + box_rect[2] * 0.5, box_rect[1] + box_rect[3] * 0.5);
            u.geom = [cx, cy, 0.0, 0.0];
            pack_stops(&mut u, g.stops().map(|s| (s.position, s.color)));
        }
        _ => {}
    }
    u
}

// ---- Curve flattening & analytic-AA tessellation ----------------------------------------

fn eval_cubic(p0: (f32, f32), c1: (f32, f32), c2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let uu = u * u;
    let tt = t * t;
    let a = uu * u;
    let b = 3.0 * uu * t;
    let c = 3.0 * u * tt;
    let d = tt * t;
    (
        a * p0.0 + b * c1.0 + c * c2.0 + d * p3.0,
        a * p0.1 + b * c1.1 + c * c2.1 + d * p3.1,
    )
}

fn span_subdivs(p1: (f32, f32), p2: (f32, f32)) -> usize {
    let len = (p2.0 - p1.0).hypot(p2.1 - p1.1);
    ((len / 4.5).ceil() as usize).clamp(3, 16)
}

fn flatten_open_curve(out: &mut Vec<(f32, f32)>, oc: &OpenCurve) {
    out.clear();
    let pts = &oc.pts;
    if pts.len() < 2 {
        return;
    }
    out.reserve(pts.len() * 6);
    out.push(pts[0]);
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, (p1.1 + (p2.1 - p0.1) / 6.0).clamp(oc.lo, oc.hi));
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, (p2.1 - (p3.1 - p1.1) / 6.0).clamp(oc.lo, oc.hi));
        let subdivs = span_subdivs(p1, p2);
        for s in 1..=subdivs {
            let t = s as f32 / subdivs as f32;
            out.push(eval_cubic(p1, c1, c2, p2, t));
        }
    }
}

fn flatten_refl_curve(out: &mut Vec<(f32, f32)>, rc: &ReflCurve, y_off: f32) {
    out.clear();
    let pts = &rc.pts;
    if pts.len() < 2 {
        return;
    }
    if !rc.smooth {
        out.extend(pts.iter().map(|&(x, y)| (x, y + y_off)));
        return;
    }
    out.reserve(pts.len() * 6);
    out.push((pts[0].0, pts[0].1 + y_off));
    for i in 0..pts.len() - 1 {
        let p0 = pts[i.saturating_sub(1)];
        let (p1, p2) = (pts[i], pts[i + 1]);
        let p3 = pts[(i + 2).min(pts.len() - 1)];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, (p1.1 + (p2.1 - p0.1) / 6.0).clamp(rc.lo, rc.hi));
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, (p2.1 - (p3.1 - p1.1) / 6.0).clamp(rc.lo, rc.hi));
        let subdivs = span_subdivs(p1, p2);
        for s in 1..=subdivs {
            let t = s as f32 / subdivs as f32;
            let (x, y) = eval_cubic(p1, c1, c2, p2, t);
            out.push((x, y + y_off));
        }
    }
}

fn flatten_closed_curve(out: &mut Vec<(f32, f32)>, pts: &[(f32, f32)]) {
    out.clear();
    let n = pts.len();
    if n < 3 {
        return;
    }
    out.reserve(n * 8);
    for i in 0..n {
        let p0 = pts[(i + n - 1) % n];
        let (p1, p2) = (pts[i], pts[(i + 1) % n]);
        let p3 = pts[(i + 2) % n];
        let c1 = (p1.0 + (p2.0 - p0.0) / 6.0, p1.1 + (p2.1 - p0.1) / 6.0);
        let c2 = (p2.0 - (p3.0 - p1.0) / 6.0, p2.1 - (p3.1 - p1.1) / 6.0);
        let subdivs = span_subdivs(p1, p2);
        for s in 0..subdivs {
            let t = s as f32 / subdivs as f32;
            out.push(eval_cubic(p1, c1, c2, p2, t));
        }
    }
}

fn flatten_closed_pair(
    out_a: &mut Vec<(f32, f32)>,
    out_b: &mut Vec<(f32, f32)>,
    a: &[(f32, f32)],
    b: &[(f32, f32)],
) {
    out_a.clear();
    out_b.clear();
    let n = a.len();
    if n < 3 || b.len() != n {
        return;
    }
    out_a.reserve(n * 8);
    out_b.reserve(n * 8);
    for i in 0..n {
        let (a0, a1, a2, a3) = (a[(i + n - 1) % n], a[i], a[(i + 1) % n], a[(i + 2) % n]);
        let (b0, b1, b2, b3) = (b[(i + n - 1) % n], b[i], b[(i + 1) % n], b[(i + 2) % n]);
        let ac1 = (a1.0 + (a2.0 - a0.0) / 6.0, a1.1 + (a2.1 - a0.1) / 6.0);
        let ac2 = (a2.0 - (a3.0 - a1.0) / 6.0, a2.1 - (a3.1 - a1.1) / 6.0);
        let bc1 = (b1.0 + (b2.0 - b0.0) / 6.0, b1.1 + (b2.1 - b0.1) / 6.0);
        let bc2 = (b2.0 - (b3.0 - b1.0) / 6.0, b2.1 - (b3.1 - b1.1) / 6.0);
        let subdivs = span_subdivs(a1, a2).max(span_subdivs(b1, b2));
        for s in 0..subdivs {
            let t = s as f32 / subdivs as f32;
            out_a.push(eval_cubic(a1, ac1, ac2, a2, t));
            out_b.push(eval_cubic(b1, bc1, bc2, b2, t));
        }
    }
}

fn pv(x: f32, y: f32, cov: f32) -> PolyVertex {
    PolyVertex { pos: [x, y], cov, _pad: 0.0 }
}

fn push_quad(out: &mut Vec<PolyVertex>, a: PolyVertex, b: PolyVertex, c: PolyVertex, d: PolyVertex) {
    out.extend_from_slice(&[a, b, c, b, d, c]);
}

/// Filled area between `pts` and `baseline`, with a 1px analytic AA feather along the curve edge.
fn build_area_verts(
    out: &mut Vec<PolyVertex>,
    pts: &[(f32, f32)],
    baseline: f32,
    _stage_w: f32,
    aa_pad: f32,
) {
    out.clear();
    let n = pts.len();
    if n < 2 {
        return;
    }
    out.reserve((n - 1) * 12);
    // Direction from baseline to curve (+1 if reflection below baseline, -1 if top curve above baseline).
    let dir_y = if pts.iter().any(|&(_, y)| y > baseline) { 1.0 } else { -1.0 };
    for i in 0..n - 1 {
        let p0 = pts[i];
        let p1 = pts[i + 1];
        // Solid interior quad.
        push_quad(
            out,
            pv(p0.0, baseline, 1.0),
            pv(p0.0, p0.1, 1.0),
            pv(p1.0, baseline, 1.0),
            pv(p1.0, p1.1, 1.0),
        );
        // 1px outer AA fringe along the curve edge.
        let mut norm = unit_normal(p0, p1);
        if norm.1 * dir_y < 0.0 {
            norm = (-norm.0, -norm.1);
        }
        push_quad(
            out,
            pv(p0.0, p0.1, 1.0),
            pv(p0.0 + norm.0 * aa_pad, p0.1 + norm.1 * aa_pad, 0.0),
            pv(p1.0, p1.1, 1.0),
            pv(p1.0 + norm.0 * aa_pad, p1.1 + norm.1 * aa_pad, 0.0),
        );
    }
}

/// Closed corona annulus between `inner` and `outer` with 1px analytic AA feather on the outer contour.
fn build_corona_verts(
    out: &mut Vec<PolyVertex>,
    outer: &mut Vec<(f32, f32)>,
    contour: &[(f32, f32)],
    inner: &CoronaInner,
    scratch: &mut Vec<(f32, f32)>,
    aa_pad: f32,
) {
    out.clear();
    match inner {
        CoronaInner::Circle { cx, cy, r } => {
            flatten_closed_curve(outer, contour);
            scratch.clear();
            scratch.reserve(outer.len());
            for &p in outer.iter() {
                let dx = p.0 - cx;
                let dy = p.1 - cy;
                let len = (dx * dx + dy * dy).sqrt().max(1e-4);
                scratch.push((cx + dx / len * r, cy + dy / len * r));
            }
        }
        CoronaInner::Poly(foot) => {
            flatten_closed_pair(outer, scratch, contour, foot);
        }
    }
    let m = outer.len();
    if m < 3 || scratch.len() != m {
        return;
    }
    out.reserve(m * 12);
    for i in 0..m {
        let j = (i + 1) % m;
        let i0 = scratch[i];
        let i1 = scratch[j];
        let o0 = outer[i];
        let o1 = outer[j];
        push_quad(out, pv(i0.0, i0.1, 1.0), pv(o0.0, o0.1, 1.0), pv(i1.0, i1.1, 1.0), pv(o1.0, o1.1, 1.0));
        let (dx0, dy0) = (o0.0 - i0.0, o0.1 - i0.1);
        let l0 = (dx0 * dx0 + dy0 * dy0).sqrt().max(1e-4);
        let (dx1, dy1) = (o1.0 - i1.0, o1.1 - i1.1);
        let l1 = (dx1 * dx1 + dy1 * dy1).sqrt().max(1e-4);
        push_quad(
            out,
            pv(o0.0, o0.1, 1.0),
            pv(o0.0 + dx0 / l0 * aa_pad, o0.1 + dy0 / l0 * aa_pad, 0.0),
            pv(o1.0, o1.1, 1.0),
            pv(o1.0 + dx1 / l1 * aa_pad, o1.1 + dy1 / l1 * aa_pad, 0.0),
        );
    }
}

fn build_capsules(pts: &[(f32, f32)], hw: f32, closed: bool) -> Vec<CapsuleInstance> {
    let n = pts.len();
    if n < 2 || hw <= 0.0 {
        return Vec::new();
    }
    // Decimate overly dense points for wide glows so consecutive capsules don't stack
    // redundant bounding boxes (chord length <= min(hw * 0.8, 7.0) keeps curvature exact).
    let min_step = (hw * 0.8).clamp(2.5, 7.0);
    let min_step_sq = min_step * min_step;
    let mut kept: Vec<(f32, f32)> = Vec::with_capacity(n / 2 + 2);
    kept.push(pts[0]);
    for &p in &pts[1..] {
        let prev = *kept.last().unwrap();
        let dx = p.0 - prev.0;
        let dy = p.1 - prev.1;
        if dx * dx + dy * dy >= min_step_sq {
            kept.push(p);
        }
    }
    if !closed && kept.last() != pts.last() {
        kept.push(*pts.last().unwrap());
    }
    let m = kept.len();
    if m < 2 {
        return Vec::new();
    }
    let segs = if closed { m } else { m - 1 };
    let mut out = Vec::with_capacity(segs);
    for i in 0..segs {
        let j = (i + 1) % m;
        out.push(CapsuleInstance {
            p0: [kept[i].0, kept[i].1],
            p1: [kept[j].0, kept[j].1],
            half_width: hw,
            _pad: 0.0,
        });
    }
    out
}

fn unit_normal(a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let dx = b.0 - a.0;
    let dy = b.1 - a.1;
    let len = (dx * dx + dy * dy).sqrt();
    if len > 1e-5 { (-dy / len, dx / len) } else { (0.0, -1.0) }
}

/// Miter-clamped stroke ribbon expanded by `aa_pad` on each side, carrying exact signed distance
/// from centerline (`edge_dist`) for true 8-bit analytic fragment-shader AA at 1x sample count.
fn append_stroke_verts(
    out: &mut Vec<StrokeVertex>,
    pts: &[(f32, f32)],
    hw: f32,
    aa_pad: f32,
    closed: bool,
) {
    let n = pts.len();
    if n < 2 || hw <= 0.0 {
        return;
    }
    let ext = hw + aa_pad;
    let segs = if closed { n } else { n - 1 };
    let mut offsets: Vec<(f32, f32)> = Vec::with_capacity(n);
    for i in 0..n {
        let n_prev = if i > 0 {
            unit_normal(pts[i - 1], pts[i])
        } else if closed {
            unit_normal(pts[n - 1], pts[0])
        } else {
            unit_normal(pts[0], pts[1])
        };
        let n_next = if i + 1 < n {
            unit_normal(pts[i], pts[i + 1])
        } else if closed {
            unit_normal(pts[n - 1], pts[0])
        } else {
            n_prev
        };
        let mx = n_prev.0 + n_next.0;
        let my = n_prev.1 + n_next.1;
        let mlen = (mx * mx + my * my).sqrt();
        let (nx, ny) = if mlen > 1e-4 {
            let ux = mx / mlen;
            let uy = my / mlen;
            let dot = (ux * n_next.0 + uy * n_next.1).max(0.5);
            let scale = (ext / dot).min(ext * 2.0);
            (ux * scale, uy * scale)
        } else {
            (n_next.0 * ext, n_next.1 * ext)
        };
        offsets.push((nx, ny));
    }

    out.reserve(segs * 6);
    for i in 0..segs {
        let j = (i + 1) % n;
        let p0 = pts[i];
        let p1 = pts[j];
        let o0 = offsets[i];
        let o1 = offsets[j];
        let a = StrokeVertex { pos: [p0.0 - o0.0, p0.1 - o0.1], edge_dist: -ext, half_width: hw };
        let b = StrokeVertex { pos: [p0.0 + o0.0, p0.1 + o0.1], edge_dist: ext, half_width: hw };
        let c = StrokeVertex { pos: [p1.0 - o1.0, p1.1 - o1.1], edge_dist: -ext, half_width: hw };
        let d = StrokeVertex { pos: [p1.0 + o1.0, p1.1 + o1.1], edge_dist: ext, half_width: hw };
        out.extend_from_slice(&[a, b, c, b, d, c]);
    }
}

// ---- AppKit / CoreGraphics helpers ------------------------------------------------------

fn keep_window_opaque(app: &AppWindow) {
    app.window().with_winit_window(|w| {
        let Ok(handle) = w.window_handle() else { return };
        let RawWindowHandle::AppKit(ak) = handle.as_raw() else { return };
        // SAFETY: AppKit window handle points to the live NSView on the main thread.
        let view: &NSView = unsafe { ak.ns_view.cast::<NSView>().as_ref() };
        if let Some(win) = view.window()
            && !win.isOpaque()
        {
            win.setOpaque(true);
            win.setBackgroundColor(Some(&NSColor::blackColor()));
        }
    });
}

fn upload_rgba_texture(
    device: &ProtocolObject<dyn MTLDevice>,
    img: &Image,
) -> Option<SizedTexture> {
    let rgba = img.to_rgba8()?;
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    // SAFETY: valid 2D RGBA8Unorm texture descriptor.
    let desc = unsafe {
        MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(
            MTLPixelFormat::RGBA8Unorm,
            w,
            h,
            false,
        )
    };
    desc.setStorageMode(MTLStorageMode::Shared);
    desc.setUsage(MTLTextureUsage::ShaderRead);
    let tex = device.newTextureWithDescriptor(&desc)?;
    let region = MTLRegion {
        origin: MTLOrigin { x: 0, y: 0, z: 0 },
        size: MTLSize { width: w, height: h, depth: 1 },
    };
    let bytes = rgba.as_bytes();
    // SAFETY: `bytes` contains `w * h * 4` bytes matching `region` at mip level 0.
    unsafe {
        tex.replaceRegion_mipmapLevel_withBytes_bytesPerRow(
            region,
            0,
            NonNull::new_unchecked(bytes.as_ptr().cast_mut().cast()),
            w * 4,
        );
    }
    Some((w as u32, h as u32, tex))
}

fn app_root_layer(app: &AppWindow) -> Option<Retained<CALayer>> {
    app.window()
        .with_winit_window(|w| {
            let handle = w.window_handle().ok()?;
            let RawWindowHandle::AppKit(ak) = handle.as_raw() else {
                return None;
            };
            // SAFETY: AppKit window handle points to the window's live NSView on the main thread.
            let view: &NSView = unsafe { ak.ns_view.cast::<NSView>().as_ref() };
            view.layer()
        })
        .flatten()
}

fn find_slint_foreground(root: &CALayer) -> Option<Retained<CALayer>> {
    // SAFETY: reading CALayer.sublayers on the main thread.
    let subs = unsafe { root.sublayers() }?;
    for layer in subs.iter() {
        // SAFETY: checking Objective-C class membership and reading CAMetalLayer.device.
        let is_metal: bool = unsafe { msg_send![&*layer, isKindOfClass: CAMetalLayer::class()] };
        if is_metal {
            let dev: Option<Retained<AnyObject>> = unsafe { msg_send![&*layer, device] };
            if dev.is_some() {
                return Some(layer);
            }
        }
    }
    None
}

fn cg_color(c: Color) -> objc2_core_foundation::CFRetained<CGColor> {
    CGColor::new_srgb(
        f64::from(c.red()) / 255.0,
        f64::from(c.green()) / 255.0,
        f64::from(c.blue()) / 255.0,
        f64::from(c.alpha()) / 255.0,
    )
}

/// Pre-bake the heavily blurred backdrop art + vertical tint gradient into a compact
/// opaque sRGB CGImage once per track/aspect change so `scene` needs zero extra sublayers.
fn bake_backdrop_cgimage(
    win_w: f32,
    win_h: f32,
    bg: Color,
    tint: Color,
    opacity: f32,
    visible: bool,
    img: Option<&Image>,
) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    if !visible {
        return None;
    }
    // 256 px wide is far sharper than the 64 px blurred backdrop art and takes <0.2 ms once per track.
    let w = 256u32;
    let h = ((f32::from(w as u16) * win_h / win_w.max(1.0)).round() as u32).clamp(64, 512);
    let rgba = render_backdrop_rgba(
        w,
        h,
        bg,
        tint,
        opacity,
        visible,
        img.cloned(),
        [0.0; 4],
        0.0,
        1.0,
    );
    let data = CFData::from_bytes(rgba.as_raw());
    let provider = CGDataProvider::with_cf_data(Some(&data))?;
    // SAFETY: Apple CoreGraphics framework constant kCGColorSpaceSRGB.
    let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))?;
    let bitmap_info = CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0);
    // SAFETY: `data` owns the RGBA8 pixel buffer and is retained by `provider`.
    unsafe {
        CGImage::new(
            w as usize,
            h as usize,
            8,
            32,
            (w as usize) * 4,
            Some(&space),
            bitmap_info,
            Some(&provider),
            std::ptr::null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn render_backdrop_rgba(
    w: u32,
    h: u32,
    bg: Color,
    tint: Color,
    opacity: f32,
    visible: bool,
    img: Option<Image>,
    panel_rect: [f32; 4],
    panel_radius: f32,
    scale: f32,
) -> image::RgbaImage {
    let mut out = image::RgbaImage::from_pixel(w, h, image::Rgba([bg.red(), bg.green(), bg.blue(), 255]));
    if visible {
        if let Some(rgba) = img.and_then(|i| i.to_rgba8()) {
            let (sw, sh) = (rgba.width() as f32, rgba.height() as f32);
            if sw > 0.0 && sh > 0.0 {
                let scale_fill = (w as f32 / sw).max(h as f32 / sh);
                let ox = (w as f32 - sw * scale_fill) * 0.5;
                let oy = (h as f32 - sh * scale_fill) * 0.5;
                let src_bytes = rgba.as_bytes();
                let sw_u = rgba.width() as usize;
                let sh_u = rgba.height() as usize;
                let op = opacity.clamp(0.0, 1.0);
                for y in 0..h {
                    let sy = ((y as f32 - oy) / scale_fill).clamp(0.0, (sh_u - 1) as f32);
                    let y0 = sy.floor() as usize;
                    let y1 = (y0 + 1).min(sh_u - 1);
                    let fy = sy - y0 as f32;
                    for x in 0..w {
                        let sx = ((x as f32 - ox) / scale_fill).clamp(0.0, (sw_u - 1) as f32);
                        let x0 = sx.floor() as usize;
                        let x1 = (x0 + 1).min(sw_u - 1);
                        let fx = sx - x0 as f32;
                        let idx = |xx: usize, yy: usize| (yy * sw_u + xx) * 4;
                        let (i00, i10, i01, i11) = (idx(x0, y0), idx(x1, y0), idx(x0, y1), idx(x1, y1));
                        let bilerp = |ch: usize| -> f32 {
                            let top = f32::from(src_bytes[i00 + ch]) * (1.0 - fx) + f32::from(src_bytes[i10 + ch]) * fx;
                            let bot = f32::from(src_bytes[i01 + ch]) * (1.0 - fx) + f32::from(src_bytes[i11 + ch]) * fx;
                            top * (1.0 - fy) + bot * fy
                        };
                        let px = out.get_pixel_mut(x, y);
                        px[0] = (bilerp(0) * op + f32::from(px[0]) * (1.0 - op)).round().clamp(0.0, 255.0) as u8;
                        px[1] = (bilerp(1) * op + f32::from(px[1]) * (1.0 - op)).round().clamp(0.0, 255.0) as u8;
                        px[2] = (bilerp(2) * op + f32::from(px[2]) * (1.0 - op)).round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        }
        let (tr, tg, tb, ta) = (
            f32::from(tint.red()),
            f32::from(tint.green()),
            f32::from(tint.blue()),
            f32::from(tint.alpha()) / 255.0,
        );
        let (br, bgc, bb, ba) = (
            f32::from(bg.red()),
            f32::from(bg.green()),
            f32::from(bg.blue()),
            f32::from(bg.alpha()) / 255.0,
        );
        for y in 0..h {
            let k = ((y as f32 / h.max(1) as f32) / 0.7).clamp(0.0, 1.0);
            let r = tr + (br - tr) * k;
            let g = tg + (bgc - tg) * k;
            let b = tb + (bb - tb) * k;
            let a = (ta + (ba - ta) * k).clamp(0.0, 1.0);
            let inv = 1.0 - a;
            for x in 0..w {
                let px = out.get_pixel_mut(x, y);
                px[0] = (r * a + f32::from(px[0]) * inv).round().clamp(0.0, 255.0) as u8;
                px[1] = (g * a + f32::from(px[1]) * inv).round().clamp(0.0, 255.0) as u8;
                px[2] = (b * a + f32::from(px[2]) * inv).round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    if panel_rect[2] > 0.0 && panel_rect[3] > 0.0 {
        let px0 = (panel_rect[0] * scale).round().max(0.0) as u32;
        let py0 = (panel_rect[1] * scale).round().max(0.0) as u32;
        let pw = (panel_rect[2] * scale).round() as u32;
        let ph = (panel_rect[3] * scale).round() as u32;
        let rad = panel_radius * scale;
        let cx = px0 as f32 + pw as f32 * 0.5;
        let cy = py0 as f32 + ph as f32 * 0.5;
        let hx = pw as f32 * 0.5;
        let hy = ph as f32 * 0.5;
        let pa = 64.0 / 255.0;
        let inv = 1.0 - pa;
        for y in py0..(py0 + ph).min(h) {
            for x in px0..(px0 + pw).min(w) {
                let qx = (x as f32 - cx).abs() - (hx - rad);
                let qy = (y as f32 - cy).abs() - (hy - rad);
                let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad;
                if dist <= 0.0 {
                    let px = out.get_pixel_mut(x, y);
                    px[0] = (f32::from(px[0]) * inv).round().clamp(0.0, 255.0) as u8;
                    px[1] = (f32::from(px[1]) * inv).round().clamp(0.0, 255.0) as u8;
                    px[2] = (f32::from(px[2]) * inv).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metal_shaders_and_pipelines_compile() {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            return;
        };
        let src = NSString::from_str(MSL_SOURCE);
        let lib = device
            .newLibraryWithSource_options_error(&src, None)
            .expect("MSL_SOURCE must compile");
        assert!(build_pipelines(&device, &lib).is_some(), "1x pipelines must build");
    }
}
