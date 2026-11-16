use gpui::{Image, ImageFormat, RenderImage};
use matou_native_desktop::recovery_water::{
    Fluid, background_svg, bounded_fluid_svg, edge_svg, fluid_svg, raster,
};
use matou_native_terminal::Theme;
use serde_json::{Value, json};
use std::{sync::Arc, time::Instant};
fn inspect(
    mut decode: impl FnMut(Vec<u8>) -> anyhow::Result<Arc<RenderImage>>,
) -> anyhow::Result<()> {
    let root = std::path::Path::new("artifacts/native-parity/native/evidence/recovery-water");
    let baseline: Value =
        serde_json::from_str(include_str!("../fixtures/recovery-water-svg-baseline.json"))?;
    let mut comparisons = Vec::new();
    let mut all_equal = true;
    for case in baseline.as_array().unwrap() {
        let frame = &case["frame"];
        let before = case["svg"].as_str().unwrap();
        let after = bounded_fluid_svg(frame);
        let dpr = frame["dpr"].as_f64().unwrap() as f32;
        let mut raw_before = Vec::new();
        let mut raw_after = Vec::new();
        raster(
            |bytes| {
                let image = decode(bytes)?;
                raw_before = image.as_bytes(0).unwrap().to_vec();
                Ok(image)
            },
            before,
            dpr,
        )?;
        raster(
            |bytes| {
                let image = decode(bytes)?;
                raw_after = image.as_bytes(0).unwrap().to_vec();
                Ok(image)
            },
            &after,
            dpr,
        )?;
        assert_eq!(raw_before.len(), raw_after.len());
        let count = raw_before
            .iter()
            .zip(&raw_after)
            .filter(|(a, b)| a != b)
            .count();
        let max = raw_before
            .iter()
            .zip(&raw_after)
            .map(|(a, b)| a.abs_diff(*b))
            .max()
            .unwrap_or(0);
        if count > 0 {
            all_equal = false;
        }
        comparisons.push(json!({"case":case["case"],"time":frame["time"],"width":frame["width"],"height":frame["height"],"dpr":dpr,"raw_premult_rgba_bytes":raw_before.len(),"different_bytes":count,"maximum_channel_difference":max,"entire_image_equal":count==0}));
    }
    std::fs::write(
        root.join("bounded-region-pixel-contract.json"),
        serde_json::to_vec_pretty(&comparisons)?,
    )?;
    println!(
        "Entire raw premultRGBA exact comparison: {} / {} frames equal",
        comparisons
            .iter()
            .filter(|c| c["entire_image_equal"] == true)
            .count(),
        comparisons.len()
    );
    if std::env::var_os("WATER_VERIFY_ONLY").is_some() {
        anyhow::ensure!(
            all_equal,
            "bounded filter failed whole-image raw pixel equality; candidate is not accepted"
        );
        return Ok(());
    }
    // Measure the candidate once even if bit-equality fails; that is diagnostic only.
    let mut rows = Vec::new();
    for (w, h) in [(560., 600.), (900., 700.)] {
        let mut f = Fluid::default();
        let mut samples = Vec::new();
        for i in 0..32 {
            let start = Instant::now();
            let frame = f.sample(i as f64 * 16.667, w, h, 2., false);
            let svg = bounded_fluid_svg(&frame);
            raster(&mut decode, &svg, 2.)?;
            let ms = start.elapsed().as_secs_f64() * 1000.;
            samples.push(ms);
            rows.push(json!({"cards":1,"width":w,"height":h,"dpr":2,"frame":i,"total_ms":ms}));
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "bounded {w}x{h}@2 p50={:.3} p95={:.3} max={:.3}ms",
            samples[16], samples[30], samples[31]
        );
    }
    let cold = Instant::now();
    let bg = raster(&mut decode, &background_svg(560., 600., Theme::LIGHT), 2.)?.0;
    let bg_ms = cold.elapsed().as_secs_f64() * 1000.;
    let cold = Instant::now();
    let edge = raster(&mut decode, &edge_svg(560., 600.), 2.)?.0;
    let edge_ms = cold.elapsed().as_secs_f64() * 1000.;
    let static_layers = (bg, edge);
    let ids = (static_layers.0.id, static_layers.1.id);
    let mut a = Fluid::default();
    let mut b = Fluid::default();
    let mut pairs = Vec::new();
    for i in 0..16 {
        let start = Instant::now();
        let images = (static_layers.0.clone(), static_layers.1.clone());
        assert_eq!((images.0.id, images.1.id), ids);
        for fluid in [&mut a, &mut b] {
            let frame = fluid.sample(i as f64 * 16.667, 560., 600., 2., false);
            let svg = if all_equal {
                bounded_fluid_svg(&frame)
            } else {
                fluid_svg(&frame)
            };
            raster(&mut decode, &svg, 2.)?;
        }
        let ms = start.elapsed().as_secs_f64() * 1000.;
        pairs.push(ms);
        rows.push(json!({"cards":2,"width":560,"height":600,"dpr":2,"frame":i,"total_ms":ms,"candidate":all_equal}));
    }
    pairs.sort_by(f64::total_cmp);
    println!(
        "two560x600@2 with static layers reused: p50={:.3} p95={:.3} max={:.3}ms; cold bg={bg_ms:.3} edge={edge_ms:.3}ms",
        pairs[8], pairs[15], pairs[15]
    );
    std::fs::write(
        root.join("bounded-region-performance.json"),
        serde_json::to_vec_pretty(
            &json!({"all_raw_pixels_equal":all_equal,"static_layer_rasterizations":2,"static_layer_reuse_frames":16,"static_background_ms":bg_ms,"static_edge_ms":edge_ms,"static_images_ever_uploaded":false,"rows":rows}),
        )?,
    )?;
    anyhow::ensure!(
        all_equal,
        "bounded filter failed whole-image raw pixel equality; candidate is not accepted"
    );
    Ok(())
}
fn main() {
    gpui::Application::new().run(|cx| {
        let renderer = cx.svg_renderer();
        let job = cx.background_executor().spawn(async move {
            inspect(|bytes| {
                Image::from_bytes(ImageFormat::Svg, bytes).to_image_data(renderer.clone())
            })
        });
        cx.spawn(async move |cx| {
            let result = job.await;
            cx.update(|cx| {
                // AppKit terminate exits0 without returning from Application::run. Publish
                // the negative contract status before asking GPUI to terminate this probe.
                if let Err(error) = result {
                    eprintln!("{error:#}");
                    std::process::exit(1);
                }
                cx.quit();
            })
            .unwrap();
        })
        .detach();
    });
}
