use std::{fs, io, path::Path};

use gpui::{App, Bounds, DisplayId, Pixels, Size, WindowBounds, point, px, size};

pub fn load(path: &Path, minimum: Size<Pixels>, cx: &App) -> Option<(WindowBounds, DisplayId)> {
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let coordinate = |name: &str| {
        let value = saved[name].as_f64()? as f32;
        value.is_finite().then(|| px(value))
    };
    let bounds = Bounds::new(
        point(coordinate("x")?, coordinate("y")?),
        size(coordinate("width")?, coordinate("height")?),
    );
    if bounds.size.width < minimum.width || bounds.size.height < minimum.height {
        return None;
    }
    let display = cx.displays().into_iter().find(|display| {
        let available = display.bounds();
        // Keep the title area reachable and let GPUI restore to the display
        // containing the window center, including secondary monitors.
        available.contains(&bounds.center())
            && available.contains(&point(bounds.center().x, bounds.top() + px(16.)))
    })?;
    let state = if saved["maximized"].as_bool()? {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    };
    Some((state, display.id()))
}

pub fn save(path: &Path, state: WindowBounds) -> io::Result<()> {
    let bounds = state.get_bounds();
    let bytes = serde_json::to_vec(&serde_json::json!({
        "x": f32::from(bounds.origin.x),
        "y": f32::from(bounds.origin.y),
        "width": f32::from(bounds.size.width),
        "height": f32::from(bounds.size.height),
        "maximized": matches!(state, WindowBounds::Maximized(_)),
    }))?;
    let staged = path.with_extension("json.tmp");
    let result = fs::write(&staged, bytes).and_then(|()| fs::rename(&staged, path));
    if result.is_err() {
        let _ = fs::remove_file(staged);
    }
    result
}
