//! Tray icon state: colored while the server runs, gray when stopped.
//!
//! The gray variant is derived at runtime from the app icon (luma per
//! pixel), so no second asset can drift out of sync.

use std::sync::Mutex;
use tauri::Manager;

/// Last icon shown (`None` = unknown, force set on first call so the
/// launch-time gray actually applies). Skips redundant `set_icon` calls
/// (e.g. the frontend polls `server_status` every 5 s).
#[derive(Default)]
pub struct TrayIconState(pub Mutex<Option<bool>>);

/// Luminance grayscale in place (RGBA, alpha preserved).
fn to_grayscale(rgba: &mut [u8]) {
    for px in rgba.chunks_exact_mut(4) {
        let l = ((299 * px[0] as u32 + 587 * px[1] as u32 + 114 * px[2] as u32) / 1000).min(255) as u8;
        px[0] = l;
        px[1] = l;
        px[2] = l;
    }
}

fn gray_icon(app: &tauri::AppHandle) -> Option<tauri::image::Image<'static>> {
    let src = app.default_window_icon()?;
    let mut rgba = src.rgba().to_vec();
    to_grayscale(&mut rgba);
    Some(tauri::image::Image::new_owned(rgba, src.width(), src.height()))
}

/// Switch the tray icon, only on transition. Errors are ignored (tray may
/// be gone in tests); missing state means unmanaged (tests) — still apply.
pub fn refresh_tray_icon(app: &tauri::AppHandle, running: bool) {
    let changed = app
        .try_state::<TrayIconState>()
        .map(|s| {
            let mut g = s.0.lock().unwrap_or_else(|e| e.into_inner());
            let changed = *g != Some(running);
            *g = Some(running);
            changed
        })
        .unwrap_or(true);
    if !changed {
        return;
    }
    let icon = if running {
        app.default_window_icon().cloned()
    } else {
        gray_icon(app)
    };
    if let (Some(tray), Some(icon)) = (app.tray_by_id("main"), icon) {
        let _ = tray.set_icon(Some(icon));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_luma() {
        // pure red -> 76, green -> 149, blue -> 29, white -> 255; alpha kept
        let mut px = vec![255, 0, 0, 200, 0, 255, 0, 200, 0, 0, 255, 200, 255, 255, 255, 200];
        to_grayscale(&mut px);
        assert_eq!(px, vec![76, 76, 76, 200, 149, 149, 149, 200, 29, 29, 29, 200, 255, 255, 255, 200]);
    }
}
