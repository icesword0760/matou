//! CoreGraphics candidate for the frozen recovery canvas. No view or scheduling.
//! CG's blur kernel is not specified as Canvas Gaussian sigma; this backend
//! forwards the source blur in device pixels and reports that mapping as unverified.
use anyhow::{Result, ensure};
use core_foundation::base::TCFType;
use core_graphics::{
    base::{kCGBitmapByteOrder32Big, kCGImageAlphaPremultipliedLast},
    color::CGColor,
    color_space::{CGColorSpace, kCGColorSpaceSRGB},
    context::{CGContext, CGContextRef, CGLineCap, CGLineJoin},
    geometry::{CGPoint, CGRect, CGSize},
    gradient::{CGGradient, CGGradientDrawingOptions},
};
use gpui::RenderImage;
use matou_native_terminal::Theme;
use serde_json::Value;
use std::{ffi::c_void, sync::Arc};

// These stable CoreGraphics calls are absent from core-graphics0.24's safe
// context surface. ForeignTypeRef::as_ptr itself uses this same reference cast;
// no extra foreign-types dependency or vendor modification is needed.
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGColorCreate(
        space: *const c_void,
        components: *const f64,
    ) -> core_graphics::sys::CGColorRef;
    fn CGContextSetFillColorSpace(context: *mut c_void, space: *const c_void);
    fn CGContextSetStrokeColorSpace(context: *mut c_void, space: *const c_void);
    fn CGContextSetFillColor(context: *mut c_void, components: *const f64);
    fn CGContextSetStrokeColor(context: *mut c_void, components: *const f64);
    fn CGContextAddEllipseInRect(context: *mut c_void, rect: CGRect);
}
fn ctx_ptr(ctx: &CGContextRef) -> *mut c_void {
    (ctx as *const CGContextRef).cast_mut().cast()
}
fn space_ptr(space: &CGColorSpace) -> *const c_void {
    (&**space as *const core_graphics::color_space::CGColorSpaceRef).cast()
}
fn srgb() -> CGColorSpace {
    unsafe { CGColorSpace::create_with_name(kCGColorSpaceSRGB).expect("system sRGB color space") }
}
fn cg_color(space: &CGColorSpace, rgba: [f64; 4]) -> CGColor {
    unsafe { CGColor::wrap_under_create_rule(CGColorCreate(space_ptr(space), rgba.as_ptr())) }
}
fn rgba(value: &Value) -> [f64; 4] {
    let s = value.as_str().expect("source RGBA style");
    let c = s
        .trim_start_matches("rgba(")
        .trim_end_matches(')')
        .split(',')
        .map(|v| v.parse::<f64>().unwrap())
        .collect::<Vec<_>>();
    [c[0] / 255., c[1] / 255., c[2] / 255., c[3]]
}
fn num(v: &Value) -> f64 {
    v.as_f64().unwrap()
}
fn rect(x: f64, y: f64, w: f64, h: f64) -> CGRect {
    CGRect::new(&CGPoint::new(x, y), &CGSize::new(w, h))
}
fn fill(ctx: &CGContextRef, color: [f64; 4]) {
    unsafe { CGContextSetFillColor(ctx_ptr(ctx), color.as_ptr()) }
}
fn stroke(ctx: &CGContextRef, color: [f64; 4]) {
    unsafe { CGContextSetStrokeColor(ctx_ptr(ctx), color.as_ptr()) }
}
fn disable_shadow(ctx: &CGContextRef, space: &CGColorSpace) {
    ctx.set_shadow_with_color(CGSize::new(0., 0.), 0., &cg_color(space, [0.; 4]));
}
fn bitmap(width: u32, height: u32) -> (CGContext, CGColorSpace) {
    let space = srgb();
    let ctx = CGContext::create_bitmap_context(
        None,
        width as usize,
        height as usize,
        8,
        width as usize * 4,
        &space,
        kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big,
    );
    ctx.clear_rect(rect(0., 0., width as f64, height as f64));
    ctx.translate(0., height as f64);
    ctx.scale(1., -1.);
    unsafe {
        CGContextSetFillColorSpace(ctx_ptr(&ctx), space_ptr(&space));
        CGContextSetStrokeColorSpace(ctx_ptr(&ctx), space_ptr(&space));
    }
    ctx.set_line_cap(CGLineCap::CGLineCapButt);
    ctx.set_line_join(CGLineJoin::CGLineJoinMiter);
    ctx.set_miter_limit(10.);
    disable_shadow(&ctx, &space);
    (ctx, space)
}
fn path(ctx: &CGContextRef, commands: &Value, dpr: f64) {
    ctx.begin_path();
    for c in commands.as_array().unwrap() {
        match c[0].as_str().unwrap() {
            "M" => ctx.move_to_point(num(&c[1]) * dpr, num(&c[2]) * dpr),
            "L" => ctx.add_line_to_point(num(&c[1]) * dpr, num(&c[2]) * dpr),
            "C" => ctx.add_curve_to_point(
                num(&c[1]) * dpr,
                num(&c[2]) * dpr,
                num(&c[3]) * dpr,
                num(&c[4]) * dpr,
                num(&c[5]) * dpr,
                num(&c[6]) * dpr,
            ),
            "A" => {
                let (x, y, r) = (num(&c[1]) * dpr, num(&c[2]) * dpr, num(&c[3]) * dpr);
                unsafe {
                    CGContextAddEllipseInRect(ctx_ptr(ctx), rect(x - r, y - r, 2. * r, 2. * r));
                }
            }
            "Z" => ctx.close_path(),
            _ => unreachable!("source path operation"),
        }
    }
}
fn gradient(space: &CGColorSpace, stops: &Value) -> CGGradient {
    let mut colors = Vec::new();
    let mut locations = Vec::new();
    for stop in stops.as_array().unwrap() {
        locations.push(num(&stop[0]));
        colors.extend_from_slice(&rgba(&stop[1]));
    }
    CGGradient::create_with_color_components(space, &colors, &locations, locations.len())
}
fn paint(ctx: &CGContextRef, space: &CGColorSpace, style: &Value, dpr: f64) {
    if let Some(points) = style["points"].as_array() {
        ctx.clip();
        let g = gradient(space, &style["stops"]);
        ctx.draw_linear_gradient(
            &g,
            CGPoint::new(num(&points[0]) * dpr, num(&points[1]) * dpr),
            CGPoint::new(num(&points[2]) * dpr, num(&points[3]) * dpr),
            CGGradientDrawingOptions::CGGradientDrawsBeforeStartLocation
                | CGGradientDrawingOptions::CGGradientDrawsAfterEndLocation,
        );
    } else {
        fill(ctx, rgba(style));
        ctx.fill_path();
    }
}
#[derive(Clone)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub premult_rgba: Vec<u8>,
}
impl Bitmap {
    pub fn straight_rgba(&self) -> Vec<u8> {
        let mut out = self.premult_rgba.clone();
        for p in out.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 {
                for c in &mut p[..3] {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
    /// GPUI's ordinary image path expects straight BGRA, not CG premultiplied RGBA.
    pub fn render_image(&self) -> Arc<RenderImage> {
        let mut data = self.straight_rgba();
        for p in data.chunks_exact_mut(4) {
            p.swap(0, 2);
        }
        Arc::new(RenderImage::new(vec![image::Frame::new(
            image::RgbaImage::from_raw(self.width, self.height, data).unwrap(),
        )]))
    }
}
fn finish(mut ctx: CGContext) -> Bitmap {
    ctx.flush();
    let (width, height) = (ctx.width() as u32, ctx.height() as u32);
    Bitmap {
        width,
        height,
        premult_rgba: ctx.data().to_vec(),
    }
}
/// Draw all source ops; the caller owns time sampling and background scheduling.
pub fn draw_fluid(frame: &Value) -> Result<Bitmap> {
    let (w, h, dpr) = (
        num(&frame["width"]),
        num(&frame["height"]),
        num(&frame["dpr"]),
    );
    ensure!(w > 0. && h > 0. && dpr > 0., "nonzero source canvas");
    let (ctx, space) = bitmap((w * dpr).round() as u32, (h * dpr).round() as u32);
    let mut clipped = false;
    for op in frame["ops"].as_array().unwrap() {
        let kind = op["op"].as_str().unwrap();
        if kind == "clip" {
            ctx.save();
            path(&ctx, &op["path"], dpr);
            ctx.clip();
            clipped = true;
            continue;
        }
        let blur = op["blur"].as_f64().unwrap_or(0.);
        if clipped && blur > 0. {
            ctx.restore();
            clipped = false;
        }
        ctx.save();
        ctx.set_alpha(num(&op["alpha"]));
        disable_shadow(&ctx, &space);
        match kind {
            "fill" => {
                path(&ctx, &op["path"], dpr);
                paint(&ctx, &space, &op["style"], dpr);
            }
            "rect" => {
                let r = &op["rect"];
                ctx.begin_path();
                let (x, y, w, h) = (
                    num(&r[0]) * dpr,
                    num(&r[1]) * dpr,
                    num(&r[2]) * dpr,
                    num(&r[3]) * dpr,
                );
                ctx.move_to_point(x, y);
                ctx.add_line_to_point(x + w, y);
                ctx.add_line_to_point(x + w, y + h);
                ctx.add_line_to_point(x, y + h);
                ctx.close_path();
                paint(&ctx, &space, &op["style"], dpr);
            }
            "stroke" => {
                path(&ctx, &op["path"], dpr);
                stroke(&ctx, rgba(&op["style"]));
                ctx.set_line_width(num(&op["width"]) * dpr);
                // Canvas paints no shadow when blur and both offsets are zero. Highlights
                // inherit a shadowColor but must not receive a zero-blur duplicate stroke.
                if blur > 0. && !op["shadow"].is_null() {
                    let shadow = rgba(&op["shadow"]);
                    if shadow[3] > 0. {
                        ctx.set_shadow_with_color(
                            CGSize::new(0., 0.),
                            blur,
                            &cg_color(&space, shadow),
                        );
                    }
                }
                ctx.stroke_path();
            }
            _ => unreachable!("source draw operation"),
        }
        ctx.restore();
    }
    if clipped {
        ctx.restore();
    }
    Ok(finish(ctx))
}
pub fn water_base_color(theme: Theme) -> u32 {
    if theme == Theme::LIGHT {
        // The frozen renderer presents CSS #fafcff as (251, 252, 255) in sRGB
        // captures. Keep the one-byte raster quantization in the static layer.
        0xfbfcff
    } else {
        theme.background
    }
}

/// CSS background is independent of the animated canvas alpha.
pub fn draw_background(width: f64, height: f64, dpr: f64, theme: Theme) -> Bitmap {
    background_layers(width, height, dpr, theme, true)
}

fn background_layers(width: f64, height: f64, dpr: f64, theme: Theme, radial: bool) -> Bitmap {
    let (w, h) = ((width * dpr).round() as u32, (height * dpr).round() as u32);
    let bg = water_base_color(theme);
    let base = [
        f64::from((bg >> 16) & 255),
        f64::from((bg >> 8) & 255),
        f64::from(bg & 255),
    ];
    let (tint, alpha) = if theme == Theme::LIGHT {
        ([86., 153., 219.], 0.055)
    } else {
        ([71., 147., 222.], 0.05)
    };
    let (center_x, center_y) = (width * 0.5 * dpr, height * 0.35 * dpr);
    let radius = 0.48 * center_x.hypot(height * 0.65 * dpr);
    let x_distance_squared = (0..w)
        .map(|x| (f64::from(x) + 0.5 - center_x).powi(2))
        .collect::<Vec<_>>();
    let mut premult_rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let dy_squared = (f64::from(y) + 0.5 - center_y).powi(2);
        for dx_squared in &x_distance_squared {
            // CSS interpolates the tint in premultiplied sRGB. Composite it
            // over the opaque base before quantizing to 8-bit channels.
            let mix = if radial {
                alpha * (1. - (dx_squared + dy_squared).sqrt() / radius).max(0.)
            } else {
                0.
            };
            for (base, tint) in base.into_iter().zip(tint) {
                premult_rgba.push((base + (tint - base) * mix).round() as u8);
            }
            premult_rgba.push(255);
        }
    }
    Bitmap {
        width: w,
        height: h,
        premult_rgba,
    }
}
// Cumulative Gaussian coverage of an edge at a signed distance in sigmas.
fn normal_cdf(distance: f64) -> f64 {
    let x = distance.abs();
    let t = 1. / (1. + 0.2316419 * x);
    let tail = (-x * x / 2.).exp()
        * 0.3989422804
        * t
        * (0.319381530
            + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    if distance < 0. { tail } else { 1. - tail }
}

fn inset_coverage(size: u32, shift: f64, sigma: f64) -> Vec<f64> {
    (0..size)
        .map(|index| {
            let center = f64::from(index) + 0.5 - shift;
            normal_cdf(center / sigma) - normal_cdf((center - f64::from(size)) / sigma)
        })
        .collect()
}

fn inset_layers(width: f64, height: f64, dpr: f64, shadow: bool, stroke: bool) -> Bitmap {
    let width = (width * dpr).round() as u32;
    let height = (height * dpr).round() as u32;
    let sigma = 24. * dpr; // CSS box-shadow blur radius 48px.
    let x_inside = inset_coverage(width, 0., sigma);
    let y_inside = inset_coverage(height, -28. * dpr, sigma);
    let mut premult_rgba = Vec::with_capacity(width as usize * height as usize * 4);
    for (y, &inside_y) in y_inside.iter().enumerate() {
        for (x, &inside_x) in x_inside.iter().enumerate() {
            let edge_distance = x
                .min(width as usize - x - 1)
                .min(y.min(height as usize - y - 1));
            let line_alpha = if stroke {
                0.055 * (dpr - edge_distance as f64).clamp(0., 1.)
            } else {
                0.
            };
            // CSS lists the stroke first, so it composites above the soft shadow.
            let shadow_alpha = if shadow {
                0.055 * (1. - inside_x * inside_y) * (1. - line_alpha)
            } else {
                0.
            };
            for (line, shade) in [181., 216., 255.].into_iter().zip([19., 92., 159.]) {
                premult_rgba.push((line * line_alpha + shade * shadow_alpha).round() as u8);
            }
            premult_rgba.push(((line_alpha + shadow_alpha) * 255.).round() as u8);
        }
    }
    Bitmap {
        width,
        height,
        premult_rgba,
    }
}

/// The CSS inset stroke and shadow above the canvas, clipped by the card at paint time.
pub fn draw_edges(width: f64, height: f64, dpr: f64) -> Bitmap {
    inset_layers(width, height, dpr, true, true)
}

#[cfg(debug_assertions)]
pub fn debug_static_layers(width: f64, height: f64, dpr: f64) -> [Bitmap; 5] {
    [
        background_layers(width, height, dpr, Theme::LIGHT, false),
        draw_background(width, height, dpr, Theme::LIGHT),
        inset_layers(width, height, dpr, false, true),
        inset_layers(width, height, dpr, true, false),
        draw_edges(width, height, dpr),
    ]
}
/// Diagnostic primitives use the same context, sRGB state and byte conversion.
pub fn diagnostic(dpr: f64, shadow_blur: f64, shadow_offset: (f64, f64)) -> Bitmap {
    let (ctx, space) = bitmap((128. * dpr).round() as u32, (96. * dpr).round() as u32);
    fill(&ctx, [128. / 255., 32. / 255., 64. / 255., 128. / 255.]);
    ctx.fill_rect(rect(4. * dpr, 3. * dpr, 6. * dpr, 5. * dpr));
    fill(&ctx, [64. / 255., 32. / 255., 128. / 255., 128. / 255.]);
    ctx.fill_rect(rect(4. * dpr, 80. * dpr, 6. * dpr, 5. * dpr));
    let shadow = cg_color(&space, [0.8, 0.2, 0.4, 1.]);
    ctx.set_shadow_with_color(
        CGSize::new(shadow_offset.0, -shadow_offset.1),
        shadow_blur,
        &shadow,
    );
    fill(&ctx, [0.2, 0.4, 0.8, 1.]);
    ctx.fill_rect(rect(20. * dpr, 20. * dpr, 1., 50. * dpr));
    finish(ctx)
}
