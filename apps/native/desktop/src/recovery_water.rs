//! The frozen recovery canvas, sampled and rasterized natively. No web surface.
use gpui::*;
use matou_native_terminal::Theme;
use serde_json::{Value, json};
use std::{f64::consts::PI, sync::Arc};

fn clamp(v: f64, a: f64, b: f64) -> f64 {
    v.max(a).min(b)
}
fn smooth(v: f64) -> f64 {
    v * v * (3. - 2. * v)
}
#[derive(Default, Clone)]
pub struct Fluid {
    nodes: Vec<f64>,
    velocities: Vec<f64>,
    width: f64,
    height: f64,
    dpr: f64,
    previous: f64,
    cycle: i64,
    started: bool,
    slosh: f64,
    turbulence: f64,
    bulk_phase: f64,
    bulk: f64,
    inner: f64,
    counter: f64,
}
impl Fluid {
    /// Source visibility/start resets the delta clock, retaining origin and waves.
    pub fn reset_previous_frame(&mut self, elapsed: f64) {
        self.previous = elapsed;
    }

    fn reset(&mut self) {
        let count = (self.width / 7.).round().clamp(48., 76.) as usize;
        self.nodes = (0..count)
            .map(|i| ((i as f64 / (count - 1) as f64) * PI).sin() * 1.2)
            .collect();
        self.velocities = (0..count)
            .map(|i| ((i as f64 / (count - 1) as f64) * PI).cos() * 0.18)
            .collect();
    }
    fn offset(&self, x: f64) -> f64 {
        let p = clamp(
            x / self.width.max(1.) * (self.nodes.len() - 1) as f64,
            0.,
            (self.nodes.len() - 1) as f64,
        );
        let l = p.floor() as usize;
        let r = (l + 1).min(self.nodes.len() - 1);
        let mix = p - l as f64;
        let displacement = self.nodes[l] * (1. - mix) + self.nodes[r] * mix;
        let normalized = x / self.width.max(1.);
        displacement - 3.1 * (-x.min(self.width - x).max(0.) / 11.).exp()
            + self.slosh * (normalized - 0.5) * 2.
            + (normalized * PI).sin() * self.inner * self.turbulence * 16.
            + (normalized * PI * 2.).sin() * self.counter * self.turbulence * 8.2
            + (normalized * PI * 3.).sin()
                * (self.bulk_phase * 1.47 - 1.55).sin()
                * self.turbulence
                * 4.4
    }
    fn trace(&self, base: f64) -> Vec<Value> {
        let mut p = vec![json!([
            "M",
            0.,
            clamp(base + self.offset(0.), 2., self.height - 2.)
        ])];
        let mut x = 3.;
        while x <= self.width + 3. {
            p.push(json!([
                "L",
                x,
                clamp(base + self.offset(x), 2., self.height - 2.)
            ]));
            x += 3.;
        }
        p
    }
    /// `elapsed` is from the entity's single animation origin, including hidden time.
    pub fn sample(
        &mut self,
        elapsed: f64,
        width: f64,
        height: f64,
        scale: f64,
        reduced: bool,
    ) -> Value {
        let width = width.max(0.).round();
        let height = height.max(0.).round();
        let dpr = if scale == 0. { 1. } else { scale.min(2.) };
        if width != self.width
            || height != self.height
            || (width * dpr).round() != (self.width * self.dpr).round()
        {
            self.width = width;
            self.height = height;
            self.dpr = dpr;
            self.reset();
        }
        let cycle = if reduced {
            0
        } else {
            (elapsed / 7200.).floor() as i64
        };
        if !self.started || cycle != self.cycle {
            self.cycle = cycle;
            self.started = true;
            self.reset();
        }
        let ce = if reduced { elapsed } else { elapsed % 7200. };
        let frame_scale = clamp((elapsed - self.previous) / 16.667, 0.35, 1.5);
        self.previous = elapsed;
        let rise = if reduced {
            0.5
        } else {
            0.94 * smooth(1. - (-elapsed.max(0.).rem_euclid(7200.) / 2400.).exp())
        };
        let alpha = if reduced {
            1.
        } else {
            clamp(elapsed.max(0.).rem_euclid(7200.) / 288., 0., 1.)
        };
        let surface = height * (0.85 - rise * 0.74);
        let energy = if reduced {
            0.08
        } else {
            (1. - rise).powf(1.45)
        };
        let intro = if reduced {
            1.
        } else {
            1. + (1. - smooth(clamp(ce / 1700., 0., 1.))) * 0.65
        };
        self.bulk_phase = ce * 0.00272 + 2.1 * (1. - (-ce / 1050.).exp()) - 1.08;
        self.bulk = if reduced {
            0.
        } else {
            (self.bulk_phase.sin()
                + (self.bulk_phase * 0.53 + 0.75).sin() * 0.2
                + (self.bulk_phase * 1.83 + 1.4).sin() * 0.12)
                / 1.24
        };
        self.inner = if reduced {
            0.
        } else {
            ((self.bulk_phase - 0.82).sin() + (self.bulk_phase * 1.66 + 0.35).sin() * 0.24) / 1.18
        };
        self.counter = if reduced {
            0.
        } else {
            (self.bulk_phase * 1.52 + 1.72).sin()
        };
        self.slosh = self.bulk * (2.4 + energy * 31.5) * intro;
        self.turbulence = if reduced {
            0.
        } else {
            (1. - rise).powf(0.96) * intro
        };
        if !reduced {
            let mut next = self.velocities.clone();
            let damping = 0.022 + rise * 0.07;
            let limit = 2.5 + (energy * intro).min(1.5) * 18.;
            for (i, v) in next
                .iter_mut()
                .enumerate()
                .take(self.nodes.len() - 1)
                .skip(1)
            {
                let p = self.nodes[i];
                let velocity = self.velocities[i];
                *v += (-p * 0.026 - velocity * damping
                    + (self.nodes[i - 1] + self.nodes[i + 1] - p * 2.) * 0.2)
                    * frame_scale;
            }
            for (i, v) in next.iter().enumerate().take(self.nodes.len() - 1).skip(1) {
                self.velocities[i] = *v;
                self.nodes[i] = clamp(self.nodes[i] + v * frame_scale, -limit, limit);
            }
            let drive = (self.bulk_phase.cos() + (self.bulk_phase * 1.74 + 0.5).cos() * 0.24)
                * (0.03 + (energy * intro).min(1.5) * 0.15)
                * frame_scale;
            let n = self.nodes.len();
            self.velocities[1] += drive;
            self.velocities[n - 2] -= drive;
            for v in &mut self.velocities {
                *v = clamp(*v, -1.8, 1.8);
            }
            self.nodes[0] = self.nodes[1] * 0.72;
            self.nodes[n - 1] = self.nodes[n - 2] * 0.72;
        }
        let mut water = self.trace(surface);
        water.extend([
            json!(["L", width, height + 2.]),
            json!(["L", 0., height + 2.]),
            json!(["Z"]),
        ]);
        let gradient = json!({"points":[0.,surface-5.,0.,height],"stops":[[0.,"rgba(91,194,239,.36)"],[0.08,"rgba(58,164,226,.32)"],[0.54,"rgba(31,116,197,.39)"],[1.,"rgba(17,72,151,.52)"]]});
        let center = clamp(0.5 + self.inner * self.turbulence * 0.22, 0.24, 0.76);
        let sheen = json!({"points":[0.,0.,width,0.],"stops":[[0.,"rgba(193,231,247,0)"],[clamp(center-0.16,0.02,0.72),"rgba(193,231,247,0)"],[center,format!("rgba(193,231,247,{})",0.025+self.turbulence*0.035)],[clamp(center+0.2,0.28,0.98),"rgba(193,231,247,0)"],[1.,"rgba(193,231,247,0)"]]});
        let mut ops = vec![
            json!({"op":"fill","path":water,"style":gradient,"alpha":alpha}),
            json!({"op":"clip","path":water}),
            json!({"op":"rect","rect":[0.,surface-20.,width,height-surface+20.],"style":sheen,"alpha":alpha}),
        ];
        for i in 0..5 {
            let anchor = width * ((i + 1) as f64 / 6.);
            let drift = anchor
                + self.bulk * (34. - i as f64 * 3.)
                + self.inner * (if i % 2 == 0 { 23. } else { -17. }) * self.turbulence;
            let y = surface + 58. + i as f64 * 63.;
            ops.push(json!({"op":"stroke","path":[["M",drift-75.,y+5.],["C",drift-28.,y-6.,drift+23.,y+9.,drift+76.,y-4.]],"style":format!("rgba(184,229,248,{})",0.022+i as f64*0.006),"width":0.8+(i%2) as f64*0.35,"blur":0.,"shadow":null,"alpha":alpha}));
        }
        for i in 0..11 {
            let bx = ((i * 71) % 91 + 4) as f64 / 100.;
            let radius = 1. + (i % 3) as f64 * 0.72;
            let speed = 0.68 + (i % 4) as f64 * 0.11;
            let offset = ((i * 41) % 100) as f64 / 100.;
            let progress = (offset + ce * 0.000065 * speed) % 1.;
            let y = height + 9. - progress * (height - surface + 24.).max(1.);
            if y < surface + 14. || y > height + 5. {
                continue;
            }
            let x = width * bx
                + self.bulk * 22.
                + self.inner * self.turbulence * (radius * 4.5)
                + (progress * 7. + offset * 8.).sin() * 3.;
            ops.push(json!({"op":"stroke","path":[["A",x,y,radius,0.,PI*2.]],"style":format!("rgba(215,243,252,{})",0.15+radius*0.045),"width":0.65,"blur":0.,"shadow":null,"alpha":alpha}));
        }
        ops.push(json!({"op":"stroke","path":self.trace(surface),"style":"rgba(211,242,251,.78)","width":1.05,"blur":5.,"shadow":"rgba(92,188,237,.36)","alpha":alpha}));
        let mut x = 2.;
        while x < width - 2. {
            let slope = clamp(
                (self.offset(x + 2.) - self.offset(x - 2.)).abs() / 4.,
                0.,
                1.,
            );
            if slope >= 0.08 {
                ops.push(json!({"op":"stroke","path":[["M",x,surface+self.offset(x)-0.6],["L",x+4.,surface+self.offset(x+4.)-0.6]],"style":format!("rgba(239,252,255,{})",0.08+slope*0.24),"width":0.7,"blur":0.,"shadow":"rgba(92,188,237,.36)","alpha":alpha}));
            }
            x += 7.;
        }
        json!({"width":width,"height":height,"dpr":dpr,"frameScale":frame_scale,"rise":rise,"alpha":alpha,"surface":surface,"nodes":self.nodes,"velocities":self.velocities,"signals":[energy,intro,self.bulk_phase,self.bulk,self.inner,self.counter,self.slosh,self.turbulence],"ops":ops})
    }
    pub fn resume_at(&mut self, elapsed: f64) {
        self.previous = elapsed;
    }
}
fn number(v: &Value) -> f64 {
    v.as_f64().unwrap()
}
fn color(v: &Value) -> (String, f64) {
    let s = v.as_str().unwrap();
    if let Some(s) = s.strip_prefix("rgba(") {
        let a = s
            .trim_end_matches(')')
            .split(',')
            .map(|s| s.parse::<f64>().unwrap())
            .collect::<Vec<_>>();
        (format!("rgb({},{},{})", a[0], a[1], a[2]), a[3])
    } else {
        (s.into(), 1.)
    }
}
fn path_svg(p: &Value) -> String {
    let mut out = String::new();
    for cmd in p.as_array().unwrap() {
        let a = cmd.as_array().unwrap();
        let op = a[0].as_str().unwrap();
        if op == "A" {
            let (x, y, r) = (number(&a[1]), number(&a[2]), number(&a[3]));
            out += &format!(
                "M{} {}a{r} {r} 0 1 0 {} 0a{r} {r} 0 1 0 {} 0",
                x + r,
                y,
                -2. * r,
                2. * r
            );
        } else {
            out += op;
            for n in &a[1..] {
                out += &format!("{} ", number(n));
            }
        }
    }
    out
}
fn svg_root(w: f64, h: f64, body: &str) -> String {
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"#
    )
}
/// Canvas shadowBlur is device-bitmap sigma=blur/2, independent of CTM (HTML shadows).
/// CSS inset blur48 uses CSS sigma24 (CSS Backgrounds 6.1.1/6.1.2).
pub fn fluid_svg(frame: &Value) -> String {
    render_fluid_svg(frame, false)
}

/// Rejected diagnostic candidate:2/16 saved frames differ. Never the default path.
pub fn bounded_fluid_svg(frame: &Value) -> String {
    render_fluid_svg(frame, true)
}

fn render_fluid_svg(frame: &Value, bounded: bool) -> String {
    let w = number(&frame["width"]);
    let h = number(&frame["height"]);
    let dpr = number(&frame["dpr"]);
    let mut out = String::new();
    let mut clipped = false;
    let mut gradient_id = 0;
    for op in frame["ops"].as_array().unwrap() {
        let kind = op["op"].as_str().unwrap();
        if kind == "clip" {
            out += &format!(
                r#"<defs><clipPath id="water"><path d="{}"/></clipPath></defs><g clip-path="url(#water)">"#,
                path_svg(&op["path"])
            );
            clipped = true;
            continue;
        }
        let blur = op["blur"].as_f64().unwrap_or(0.);
        if clipped && blur > 0. {
            out += "</g>";
            clipped = false;
        }
        let style = &op["style"];
        let alpha = number(&op["alpha"]);
        let (paint, opacity) = if let Some(points) = style["points"].as_array() {
            gradient_id += 1;
            let id = gradient_id;
            out += &format!(
                r#"<defs><linearGradient id="g{id}" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}" color-interpolation="sRGB">"#,
                points[0], points[1], points[2], points[3]
            );
            for stop in style["stops"].as_array().unwrap() {
                let (c, a) = color(&stop[1]);
                out += &format!(
                    r#"<stop offset="{}" stop-color="{c}" stop-opacity="{a}"/>"#,
                    stop[0]
                );
            }
            out += "</linearGradient></defs>";
            (format!("url(#g{id})"), alpha)
        } else {
            let (c, a) = color(style);
            (c, a * alpha)
        };
        if kind == "rect" {
            let r = &op["rect"];
            out += &format!(
                r#"<rect x="{}" y="{}" width="{}" height="{}" fill="{paint}" opacity="{opacity}"/>"#,
                r[0], r[1], r[2], r[3]
            );
            continue;
        }
        let path = path_svg(&op["path"]);
        if kind == "fill" {
            out += &format!(r#"<path d="{path}" fill="{paint}" opacity="{opacity}"/>"#);
        } else {
            if blur > 0. {
                let (c, a) = color(&op["shadow"]);
                let sigma = blur / 2. / dpr;
                // resvg0.45.1 box_blur.rs uses five separable finite box passes.
                // Their radii sum is the exact support, not an assumed3-sigma crop.
                let support = box_blur_support((blur / 2.) as f32) as f64;
                let mut top = -16.;
                let mut bottom = h + 16.;
                if bounded {
                    let points = op["path"].as_array().unwrap();
                    let min = points
                        .iter()
                        .map(|p| number(&p[2]))
                        .fold(f64::INFINITY, f64::min);
                    let max = points
                        .iter()
                        .map(|p| number(&p[2]))
                        .fold(f64::NEG_INFINITY, f64::max);
                    // Bound any allowed miter plus one antialias device pixel.
                    let margin = number(&op["width"]) * 0.5 * 10. + (support + 1.) / dpr;
                    // Keep the original x region and the original device-pixel lattice.
                    top = (-16. * dpr + ((min - margin + 16.) * dpr).floor()) / dpr;
                    bottom = (-16. * dpr + ((max + margin + 16.) * dpr).ceil()) / dpr;
                    top = top.max(-16.);
                    bottom = bottom.min(h + 16.);
                }
                out += &format!(
                    r#"<defs><filter id="glow" filterUnits="userSpaceOnUse" x="-16" y="{top}" width="{}" height="{}" color-interpolation-filters="sRGB"><feGaussianBlur stdDeviation="{sigma}"/></filter></defs><path d="{path}" fill="none" stroke="{c}" stroke-opacity="{}" stroke-width="{}" stroke-miterlimit="10" filter="url(#glow)"/>"#,
                    w + 32.,
                    bottom - top,
                    opacity * a,
                    op["width"]
                );
            }
            out += &format!(
                r#"<path d="{path}" fill="none" stroke="{paint}" stroke-opacity="{opacity}" stroke-width="{}" stroke-miterlimit="10"/>"#,
                op["width"]
            );
        }
    }
    if clipped {
        out += "</g>";
    }
    svg_root(w, h, &out)
}
/// Exact finite support of pinned resvg's five-pass box Gaussian approximation.
fn box_blur_support(sigma: f32) -> u32 {
    let n = 5_f32;
    let ideal = (12. * sigma * sigma / n).sqrt() + 1.;
    let mut low = ideal.floor() as i32;
    if low % 2 == 0 {
        low -= 1;
    }
    let high = low + 2;
    let l = low as f32;
    let count =
        ((12. * sigma * sigma - n * l * l - 4. * n * l - 3. * n) / (-4. * l - 4.)).round() as usize;
    (0..5)
        .map(|i| {
            if i < count {
                (low - 1) / 2
            } else {
                (high - 1) / 2
            }
        })
        .sum::<i32>() as u32
}

pub fn background_svg(w: f64, h: f64, theme: Theme) -> String {
    let light = theme.background == Theme::LIGHT.background;
    let bg = if light { 0xfafcff } else { theme.background };
    let (color, alpha) = if light {
        ("86,153,219", 0.055)
    } else {
        ("71,147,222", 0.05)
    };
    let r = ((w * 0.5).powi(2) + (h * 0.65).powi(2)).sqrt();
    svg_root(
        w,
        h,
        &format!(
            r##"<defs><radialGradient id="radial" gradientUnits="userSpaceOnUse" cx="{}" cy="{}" r="{r}"><stop stop-color="rgb({color})" stop-opacity="{alpha}"/><stop offset=".48" stop-color="rgb({color})" stop-opacity="0"/></radialGradient><clipPath id="box"><rect width="{w}" height="{h}"/></clipPath></defs><rect width="{w}" height="{h}" fill="#{bg:06x}"/><rect width="{w}" height="{h}" fill="url(#radial)"/>"##,
            w * 0.5,
            h * 0.35
        ),
    )
}
/// CSS ::after is above the canvas, and is not affected by canvas globalAlpha.
pub fn edge_svg(w: f64, h: f64) -> String {
    // Outside the shifted padding rectangle is opaque. Blur that inverse shape,
    // then clip inside the original box, as specified for an inset box shadow.
    svg_root(
        w,
        h,
        &format!(
            r#"<defs><clipPath id="box"><rect width="{w}" height="{h}"/></clipPath><filter id="inset" filterUnits="userSpaceOnUse" x="-160" y="-160" width="{}" height="{}" color-interpolation-filters="sRGB"><feGaussianBlur stdDeviation="24"/></filter></defs><g clip-path="url(#box)"><path d="M-160 -188H{}V{}H-160Z M0 -28V{}H{w}V-28Z" fill="rgb(19,92,159)" fill-opacity=".055" fill-rule="evenodd" filter="url(#inset)"/><rect width="{w}" height="{h}" fill="none" stroke="rgb(181,216,255)" stroke-opacity=".055" stroke-width="2"/></g>"#,
            w + 320.,
            h + 320.,
            w + 160.,
            h + 132.,
            h - 28.
        ),
    )
}
/// Return straight BGRA, exactly as GPUI's decoded-image path and SourceAlpha blend expect.
pub fn raster(
    render: impl FnOnce(Vec<u8>) -> anyhow::Result<Arc<RenderImage>>,
    svg: &str,
    dpr: f32,
) -> anyhow::Result<(Arc<RenderImage>, Vec<u8>, u32, u32)> {
    // GPUI exposes its SVG decoder, while SvgSize itself is private. Its public
    // Image SVG path calls the same render_pixmap at factor1; scale root dimensions.
    let width_start = svg.find("width=\"").unwrap() + 7;
    let width_end = width_start + svg[width_start..].find('"').unwrap();
    let height_start = svg.find("height=\"").unwrap() + 8;
    let height_end = height_start + svg[height_start..].find('"').unwrap();
    let w = (svg[width_start..width_end].parse::<f64>()? * f64::from(dpr)).round() as u32;
    let h = (svg[height_start..height_end].parse::<f64>()? * f64::from(dpr)).round() as u32;
    let mut scaled = svg.to_owned();
    scaled.replace_range(height_start..height_end, &h.to_string());
    scaled.replace_range(width_start..width_end, &w.to_string());
    let view_start = scaled.find("viewBox=\"").map(|v| v + 9);
    if let Some(start) = view_start {
        let end = start + scaled[start..].find('"').unwrap();
        scaled.replace_range(
            start..end,
            &format!(
                "0 0 {} {}",
                f64::from(w) / f64::from(dpr),
                f64::from(h) / f64::from(dpr)
            ),
        );
    }
    let decoded = render(scaled.into_bytes())?;
    let mut rgba = decoded.as_bytes(0).unwrap().to_vec();
    for p in rgba.chunks_exact_mut(4) {
        let a = p[3] as u32;
        if a > 0 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    let mut bgra = rgba.clone();
    for p in bgra.chunks_exact_mut(4) {
        p.swap(0, 2);
    }
    let frame = image::Frame::new(image::RgbaImage::from_raw(w, h, bgra).unwrap());
    Ok((Arc::new(RenderImage::new(vec![frame])), rgba, w, h))
}
