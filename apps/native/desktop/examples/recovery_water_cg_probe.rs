#[cfg(target_os = "macos")]
mod mac {
    use anyhow::{Result, ensure};
    use matou_native_desktop::{
        recovery_water::Fluid,
        recovery_water_macos::{self as cg, Bitmap},
    };
    use matou_native_terminal::Theme;
    use serde_json::{Value, json};
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Instant,
    };
    fn pixel(data: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
        data[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize]
            .try_into()
            .unwrap()
    }
    fn save(root: &std::path::Path, name: &str, b: &Bitmap) -> Result<()> {
        image::save_buffer(
            root.join(format!("{name}.png")),
            &b.straight_rgba(),
            b.width,
            b.height,
            image::ColorType::Rgba8,
        )?;
        Ok(())
    }
    fn summary(values: &[f64]) -> Value {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        json!({"samples":n,"p50_ms":sorted[n/2],"p95_ms":sorted[(n*95/100).min(n-1)],"max_ms":sorted[n-1]})
    }
    fn alpha_stats(b: &Bitmap) -> Value {
        let mut count = 0usize;
        let mut alpha_sum = 0u64;
        let mut min = 255u8;
        let mut max = 0u8;
        for p in b.premult_rgba.chunks_exact(4) {
            if p[3] > 0 {
                count += 1;
                min = min.min(p[3]);
                max = max.max(p[3]);
                alpha_sum += p[3] as u64;
            }
        }
        json!({"nonzero_pixels":count,"alpha_min_nonzero":min,"alpha_max":max,"alpha_sum":alpha_sum,"pixels":[b.width,b.height]})
    }
    fn inspect_profile() -> Result<Value> {
        let root =
            std::path::Path::new("artifacts/native-parity/native/evidence/recovery-water-cg");
        std::fs::create_dir_all(root)?;
        let cached = (
            cg::draw_background(560., 600., 2., Theme::LIGHT).render_image(),
            cg::draw_edges(560., 600., 2.).render_image(),
        );
        let mut a = Fluid::default();
        let mut b = Fluid::default();
        let mut rows = Vec::new();
        let mut times = Vec::new();
        for i in 0..400 {
            let start = Instant::now();
            std::hint::black_box((cached.0.clone(), cached.1.clone()));
            let mut math = 0.;
            let mut draw = 0.;
            let mut conversion = 0.;
            for f in [&mut a, &mut b] {
                let s = Instant::now();
                let frame = f.sample(i as f64 * 16.667, 560., 600., 2., false);
                let m = Instant::now();
                let bitmap = cg::draw_fluid(&frame)?;
                let d = Instant::now();
                std::hint::black_box(bitmap.render_image());
                let c = Instant::now();
                math += m.duration_since(s).as_secs_f64() * 1000.;
                draw += d.duration_since(m).as_secs_f64() * 1000.;
                conversion += c.duration_since(d).as_secs_f64() * 1000.;
            }
            let total = start.elapsed().as_secs_f64() * 1000.;
            times.push(total);
            rows.push(json!({"pair":i,"math_ms":math,"draw_ms":draw,"conversion_ms":conversion,"total_ms":total}));
        }
        let value = json!({"profiled_run":true,"cards":2,"width":560,"height":600,"dpr":2,"summary":summary(&times),"rows":rows});
        std::fs::write(
            root.join("profile-performance.json"),
            serde_json::to_vec_pretty(&value)?,
        )?;
        println!("CG profiled two-card diagnostic: {}", value["summary"]);
        Ok(value)
    }
    fn inspect() -> Result<Value> {
        let root =
            std::path::Path::new("artifacts/native-parity/native/evidence/recovery-water-cg");
        std::fs::create_dir_all(root)?;
        let mut checks = Vec::new();
        let mut profiles = Vec::new();
        for dpr in [1., 2.] {
            let b = cg::diagnostic(dpr, 0., (0., 0.));
            let top = pixel(
                &b.premult_rgba,
                b.width,
                (5. * dpr) as u32,
                (4. * dpr) as u32,
            );
            let bottom = pixel(
                &b.premult_rgba,
                b.width,
                (5. * dpr) as u32,
                (81. * dpr) as u32,
            );
            println!("DPR{dpr} raw top={top:?} bottom={bottom:?}");
            ensure!(
                top == [64, 16, 32, 128] && bottom == [32, 16, 64, 128],
                "CG sRGB premult RGBA or top-down orientation mismatch"
            );
            let image = b.render_image();
            let top_bgra = pixel(
                image.as_bytes(0).unwrap(),
                b.width,
                (5. * dpr) as u32,
                (4. * dpr) as u32,
            );
            let bottom_bgra = pixel(
                image.as_bytes(0).unwrap(),
                b.width,
                (5. * dpr) as u32,
                (81. * dpr) as u32,
            );
            ensure!(
                top_bgra == [64, 32, 128, 128] && bottom_bgra == [128, 32, 64, 128],
                "GPUI straight BGRA channel/alpha mismatch"
            );
            save(root, &format!("diagnostic-base-dpr{dpr}"), &b)?;
            let shadow = cg::diagnostic(dpr, 5., (32., 0.));
            save(root, &format!("diagnostic-shadow-dpr{dpr}"), &shadow)?;
            let x = (20. * dpr) as i32 + 32;
            let y = (45. * dpr) as u32;
            let profile = (-20..=20)
                .map(|dx| pixel(&shadow.premult_rgba, shadow.width, (x + dx) as u32, y)[3])
                .collect::<Vec<_>>();
            let mass = profile.iter().map(|v| *v as f64).sum::<f64>();
            let mean = profile
                .iter()
                .enumerate()
                .map(|(i, a)| (i as f64 - 20.) * (*a as f64))
                .sum::<f64>()
                / mass;
            let variance = profile
                .iter()
                .enumerate()
                .map(|(i, a)| (i as f64 - 20. - mean).powi(2) * (*a as f64))
                .sum::<f64>()
                / mass;
            let support = profile
                .iter()
                .enumerate()
                .filter(|(_, v)| **v > 0)
                .map(|(i, _)| i as i32 - 20)
                .collect::<Vec<_>>();
            checks.push(json!({"dpr":dpr,"top_premult_rgba":top,"bottom_premult_rgba":bottom,"top_straight_bgra":top_bgra,"bottom_straight_bgra":bottom_bgra,"cg_blur_parameter_device":5,"source_canvas_sigma_device":2.5,"shadow_profile_alpha":profile,"shadow_support_device":[support.first(),support.last()],"shadow_mass":mass,"shadow_weighted_mean_device":mean,"shadow_observed_sigma_device":variance.sqrt(),"shadow_matches_canvas":null}));
            profiles.push(profile);
        }
        ensure!(
            profiles[0] == profiles[1],
            "CG shadow device support changed with DPR despite explicit device coordinates"
        );
        let mut zero = json!({"width":64,"height":64,"dpr":1,"ops":[{"op":"stroke","path":[["M",10,20],["L",50,20]],"style":"rgba(51,102,204,.5)","width":1,"blur":0,"shadow":"rgba(204,51,102,1)","alpha":1}]});
        let inherited = cg::draw_fluid(&zero)?;
        zero["ops"][0]["shadow"] = Value::Null;
        let none = cg::draw_fluid(&zero)?;
        ensure!(
            inherited.premult_rgba == none.premult_rgba,
            "zero blur inherited shadowColor drew an unwanted shadow"
        );
        std::fs::write(
            root.join("color-coordinate-shadow-checks.json"),
            serde_json::to_vec_pretty(
                &json!({"checks":checks,"dpr_shadow_profiles_equal":true,"zero_blur_inherited_shadow_equals_none":true,"gpu_upload_verified":false,"cg_kernel_mapping_to_canvas":"unknown; no tuning performed"}),
            )?,
        )?;
        println!(
            "Color, Y orientation, straight BGRA, DPR1/2 device shadow support and zero-blur inherited-shadow checks passed."
        );
        let mut rows = Vec::new();
        let mut totals = Vec::new();
        for (w, h, dpr) in [(560., 600., 2.), (900., 700., 2.), (336., 280., 1.25)] {
            let mut fluid = Fluid::default();
            let mut durations = Vec::new();
            for i in 0..32 {
                let start = Instant::now();
                let frame = fluid.sample(i as f64 * 16.667, w, h, dpr, false);
                let after_math = Instant::now();
                let bitmap = cg::draw_fluid(&frame)?;
                let after_draw = Instant::now();
                let image = bitmap.render_image();
                let after_convert = Instant::now();
                let total = after_convert.duration_since(start).as_secs_f64() * 1000.;
                durations.push(total);
                rows.push(json!({"cards":1,"width":w,"height":h,"dpr":dpr,"frame":i,"math_ms":after_math.duration_since(start).as_secs_f64()*1000.,"draw_ms":after_draw.duration_since(after_math).as_secs_f64()*1000.,"conversion_ms":after_convert.duration_since(after_draw).as_secs_f64()*1000.,"total_ms":total,"pixels":[bitmap.width,bitmap.height]}));
                std::hint::black_box(image);
                if i == 31 && w == 560. {
                    save(root, "fluid-516ms", &bitmap)?;
                }
            }
            let s = summary(&durations);
            println!("CG {w}x{h}@{dpr} {s}");
            totals.push(json!({"width":w,"height":h,"dpr":dpr,"result":s}));
        }
        let mut static_times = Vec::new();
        let fixture: Value = serde_json::from_str(include_str!("../fixtures/recovery-water.json"))?;
        let frame = &fixture["cases"][0]["frames"][5];
        let fluid = cg::draw_fluid(frame)?;
        let mut cached = None;
        for (name, theme) in [("light", Theme::LIGHT), ("dark", Theme::DARK)] {
            let start = Instant::now();
            let bg = cg::draw_background(560., 600., 2., theme);
            let after_bg = Instant::now();
            let edges = cg::draw_edges(560., 600., 2.);
            let after_edges = Instant::now();
            let bg_image = bg.render_image();
            let edges_image = edges.render_image();
            let after_conversion = Instant::now();
            let top_alpha = pixel(&edges.premult_rgba, edges.width, 560, 6)[3];
            let bottom_alpha = pixel(&edges.premult_rgba, edges.width, 560, 1180)[3];
            println!("edge alpha top={top_alpha},bottom={bottom_alpha}");
            ensure!(
                bottom_alpha > top_alpha,
                "inner shadow points upward or failed to enter the box"
            );
            save(root, &format!("{name}-background"), &bg)?;
            if name == "light" {
                save(root, "edges", &edges)?;
                save(root, "fluid-2400ms", &fluid)?;
            }
            let mut composite =
                image::RgbaImage::from_raw(bg.width, bg.height, bg.straight_rgba()).unwrap();
            let fluid_rgba =
                image::RgbaImage::from_raw(fluid.width, fluid.height, fluid.straight_rgba())
                    .unwrap();
            let edge_rgba =
                image::RgbaImage::from_raw(edges.width, edges.height, edges.straight_rgba())
                    .unwrap();
            image::imageops::overlay(&mut composite, &fluid_rgba, 0, 0);
            image::imageops::overlay(&mut composite, &edge_rgba, 0, 0);
            composite.save(root.join(format!("{name}-2400.png")))?;
            static_times.push(json!({"theme":name,"background_ms":after_bg.duration_since(start).as_secs_f64()*1000.,"edges_ms":after_edges.duration_since(after_bg).as_secs_f64()*1000.,"both_conversion_ms":after_conversion.duration_since(after_edges).as_secs_f64()*1000.,"top_inner_shadow_alpha":top_alpha,"bottom_inner_shadow_alpha":bottom_alpha,"background":alpha_stats(&bg),"edge":alpha_stats(&edges)}));
            if name == "light" {
                cached = Some((bg_image, edges_image));
            }
        }
        let cached = cached.unwrap();
        let ids = (cached.0.id, cached.1.id);
        let mut a = Fluid::default();
        let mut b = Fluid::default();
        let mut pairs = Vec::new();
        for i in 0..16 {
            let start = Instant::now();
            let layers = (cached.0.clone(), cached.1.clone());
            ensure!(
                (layers.0.id, layers.1.id) == ids,
                "static cache replaced unexpectedly"
            );
            for f in [&mut a, &mut b] {
                let frame = f.sample(i as f64 * 16.667, 560., 600., 2., false);
                let bitmap = cg::draw_fluid(&frame)?;
                std::hint::black_box(bitmap.render_image());
            }
            let ms = start.elapsed().as_secs_f64() * 1000.;
            pairs.push(ms);
            rows.push(json!({"cards":2,"width":560,"height":600,"dpr":2,"frame":i,"total_ms":ms}));
        }
        let two = summary(&pairs);
        println!("CG two560x600@2 with static layers reused: {two}");
        let result = json!({"profile":"dev opt-level1","single":totals,"two_card":two,"static_first_costs":static_times,"rows":rows,"static_layer_reuse_pairs":16,"static_uploaded":false,"png_encoding_in_measured_frame":false,"gpu_paint_tested":false,"input_latency_measured":false});
        std::fs::write(
            root.join("performance.json"),
            serde_json::to_vec_pretty(&result)?,
        )?;
        Ok(result)
    }
    pub fn run() {
        let done = Arc::new(AtomicBool::new(false));
        gpui::Application::new().run(move |cx| {
            let done_for_job = done.clone();
            let job = cx.background_executor().spawn(async move {
                let result = if std::env::var_os("WATER_CG_PROFILE").is_some() {
                    inspect_profile()
                } else {
                    inspect()
                };
                done_for_job.store(true, Ordering::Release);
                result
            });
            let heartbeat = done.clone();
            let heartbeat_task = cx.spawn(async move |cx| {
                let mut rows = Vec::new();
                while !heartbeat.load(Ordering::Acquire) {
                    let start = Instant::now();
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(16))
                        .await;
                    cx.update(|_| {
                        rows.push(start.elapsed().as_secs_f64() * 1000.);
                    })
                    .unwrap();
                }
                rows
            });
            cx.spawn(async move |cx| {
                let result = job.await;
                let heartbeat = heartbeat_task.await;
                cx.update(|cx| {
                    let name = if std::env::var_os("WATER_CG_PROFILE").is_some() {
                        "main-thread-heartbeat-profile.json"
                    } else {
                        "main-thread-heartbeat.json"
                    };
                    std::fs::write(
                        std::path::Path::new(
                            "artifacts/native-parity/native/evidence/recovery-water-cg",
                        )
                        .join(name),
                        serde_json::to_vec_pretty(&heartbeat).unwrap(),
                    )
                    .unwrap();
                    if let Err(e) = result {
                        eprintln!("{e:#}");
                        std::process::exit(1);
                    }
                    cx.quit();
                })
                .unwrap();
            })
            .detach();
        });
    }
}
#[cfg(target_os = "macos")]
fn main() {
    mac::run()
}
#[cfg(not(target_os = "macos"))]
fn main() {}
