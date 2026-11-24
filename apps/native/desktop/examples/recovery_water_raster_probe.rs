use matou_native_desktop::recovery_water::{Fluid, background_svg, edge_svg, fluid_svg, raster};
use matou_native_terminal::Theme;
use std::time::Instant;
fn main() {
    gpui::Application::new().run(|cx| {
 let renderer=cx.svg_renderer();
 let task=cx.background_executor().spawn(async move {
 let result=(|| -> anyhow::Result<()> {
 let root=std::path::Path::new("artifacts/native-parity/native/evidence/recovery-water");std::fs::create_dir_all(root)?;
 if std::env::var_os("WATER_BASELINE_SVGS").is_some(){
 let fixture:serde_json::Value=serde_json::from_str(include_str!("../fixtures/recovery-water.json"))?;
 let mut baselines=Vec::new();for case in fixture["cases"].as_array().unwrap(){for frame in case["frames"].as_array().unwrap(){baselines.push(serde_json::json!({"case":case["name"],"frame":frame,"svg":fluid_svg(frame)}));}}
 std::fs::write("apps/native/desktop/fixtures/recovery-water-svg-baseline.json",serde_json::to_vec_pretty(&baselines)?)?;return Ok(());
 }
 let mut rows=Vec::new();
 for (w,h,dpr) in [(560.,600.,2.),(900.,700.,2.),(336.,280.,1.25)]{
  if std::env::var_os("WATER_EVIDENCE_ONLY").is_some(){continue;}
  if std::env::var_os("WATER_PROFILE").is_some()&&w!=560.{continue;}
  let mut fluid=Fluid::default();let mut times=Vec::new();
  for i in 0..if std::env::var_os("WATER_PROFILE").is_some(){240}else{32} {let start=Instant::now();let sample=fluid.sample(i as f64*16.667,w,h,dpr,false);let after_math=Instant::now();let svg=fluid_svg(&sample);let after_svg=Instant::now();let(_image,rgba,pw,ph)=raster(|bytes|gpui::Image::from_bytes(gpui::ImageFormat::Svg,bytes).to_image_data(renderer.clone()),&svg,dpr as f32)?;let elapsed=start.elapsed().as_secs_f64()*1000.;let raster_ms=elapsed-after_svg.duration_since(start).as_secs_f64()*1000.;times.push(elapsed);if i==31&&w==560.{image::save_buffer(root.join("fluid.png"),&rgba,pw,ph,image::ColorType::Rgba8)?;std::fs::write(root.join("fluid.svg"),svg.as_bytes())?;}rows.push(serde_json::json!({"width":w,"height":h,"dpr":dpr,"frame":i,"math_ms":after_math.duration_since(start).as_secs_f64()*1000.,"svg_ms":after_svg.duration_since(after_math).as_secs_f64()*1000.,"raster_ms":raster_ms,"total_ms":elapsed,"pixels":[pw,ph]}));}
  times.sort_by(f64::total_cmp);println!("{w}x{h}@{dpr}: p50={:.3}ms p95={:.3}ms max={:.3}ms",times[times.len()/2],times[(times.len()*95/100).min(times.len()-1)],times[times.len()-1]);
 }
 for(name,svg)in [("background",background_svg(560.,600.,Theme::LIGHT)),("edges",edge_svg(560.,600.))]{let start=Instant::now();let(_,rgba,w,h)=raster(|bytes|gpui::Image::from_bytes(gpui::ImageFormat::Svg,bytes).to_image_data(renderer.clone()),&svg,2.)?;println!("static {name}: {:.3}ms",start.elapsed().as_secs_f64()*1000.);image::save_buffer(root.join(format!("{name}.png")),&rgba,w,h,image::ColorType::Rgba8)?;std::fs::write(root.join(format!("{name}.svg")),svg)?;}
 if std::env::var_os("WATER_PROFILE").is_none(){
 let fixture:serde_json::Value=serde_json::from_str(include_str!("../fixtures/recovery-water.json"))?;
 let fluid=fluid_svg(&fixture["cases"][0]["frames"][5]);
 for(name,theme)in [("light",Theme::LIGHT),("dark",Theme::DARK)] {
 let svg=format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="560" height="600" viewBox="0 0 560 600">{}{}{}</svg>"#,background_svg(560.,600.,theme),fluid,edge_svg(560.,600.));
 let(image,rgba,w,h)=raster(|bytes|gpui::Image::from_bytes(gpui::ImageFormat::Svg,bytes).to_image_data(renderer.clone()),&svg,2.)?;
 assert_eq!(image.size(0).width.0,w as i32);
 image::save_buffer(root.join(format!("{name}-2400.png")),&rgba,w,h,image::ColorType::Rgba8)?;
 }
 let(image,rgba,_,_)=raster(|bytes|gpui::Image::from_bytes(gpui::ImageFormat::Svg,bytes).to_image_data(renderer.clone()),r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><rect width="2" height="2" fill="red" opacity=".5"/></svg>"#,1.)?;
 assert_eq!(&rgba[..4],&[255,0,0,128]);assert_eq!(&image.as_bytes(0).unwrap()[..4],&[0,0,255,128]);
 println!("SVG decode premultRGBA -> straight BGRA probe passed: half-alpha red=BGRA[0,0,255,128].");
 }
 if std::env::var_os("WATER_EVIDENCE_ONLY").is_none(){std::fs::write(root.join(if std::env::var_os("WATER_PROFILE").is_some(){"profile-raster-times.json"}else{"raster-times.json"}),serde_json::to_vec_pretty(&rows)?)?;}Ok(())
 })();result
 });
 cx.spawn(async move|cx| {let result=task.await;cx.update(|cx| {result.unwrap();cx.quit();}).unwrap();}).detach();
 });
}
