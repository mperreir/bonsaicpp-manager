//! Live GPU VRAM usage (Windows, all vendors).
//!
//! `nvidia-smi` only covers NVIDIA and `rocm-smi` does not exist on
//! Windows. Adapter identity + totals come from DXGI, but DXGI
//! `CurrentUsage` is per-process (our own ~0): adapter-wide usage comes
//! from the `GPU Adapter Memory` PDH counters.

use serde::Serialize;

use crate::locale::{msg, Lang, Msg};

#[derive(Debug, Clone, Serialize)]
pub struct GpuMemory {
    /// Bytes currently in use on the adapter (all processes).
    pub used_bytes: u64,
    /// Dedicated video memory of the adapter.
    pub total_bytes: u64,
    /// Bytes the OS currently lets apps use.
    pub budget_bytes: u64,
    pub adapter: String,
}

#[cfg(windows)]
pub fn query_gpu_memory(lang: Lang) -> Result<GpuMemory, String> {
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::System::Com::*;
    use windows::core::Interface;

    unsafe {
        // S_FALSE when already initialised on this thread: ignore.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| format!("DXGI: {e}"))?;
        // Richest dedicated-memory adapter, skipping software (WARP) ones.
        // Totals are summed over adapters so multi-GPU stays consistent.
        let mut best: Option<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> = None;
        let mut total_all = 0u64;
        let mut i = 0;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else {
                continue;
            };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            total_all += desc.DedicatedVideoMemory as u64;
            let replace = best
                .as_ref()
                .map(|(_, d)| desc.DedicatedVideoMemory > d.DedicatedVideoMemory)
                .unwrap_or(true);
            if replace {
                best = Some((adapter, desc));
            }
        }
        let (adapter, desc) = best.ok_or(msg(lang, Msg::GpuNoneDxgi))?;
        let adapter3: IDXGIAdapter3 = adapter.cast().map_err(|e| format!("IDXGIAdapter3: {e}"))?;
        let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
        adapter3
            .QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)
            .map_err(|e| format!("QueryVideoMemoryInfo: {e}"))?;
        let len = desc
            .Description
            .iter()
            .position(|&c| c == 0)
            .unwrap_or(desc.Description.len());
        let name = String::from_utf16_lossy(&desc.Description[..len]);
        // DXGI `CurrentUsage` is per-process (our own ~0): adapter-wide
        // usage comes from PDH, with DXGI as fallback.
        let used = query_adapter_used_bytes(lang).unwrap_or(info.CurrentUsage);
        Ok(GpuMemory {
            used_bytes: used,
            total_bytes: total_all,
            budget_bytes: info.Budget,
            adapter: name,
        })
    }
}

/// Adapter-wide dedicated VRAM usage via the `GPU Adapter Memory` PDH
/// counters (summed over adapters). English counter names so it works
/// regardless of the OS language.
#[cfg(windows)]
fn query_adapter_used_bytes(lang: Lang) -> Result<u64, String> {
    use windows::Win32::System::Performance::*;
    use windows::core::w;

    unsafe {
        let mut query = PDH_HQUERY::default();
        if PdhOpenQueryW(None, 0, &mut query) != 0 {
            return Err("PdhOpenQuery".to_string());
        }
        let used = (|| {
            let mut counter = PDH_HCOUNTER::default();
            if PdhAddEnglishCounterW(
                query,
                w!("\\GPU Adapter Memory(*)\\Dedicated Usage"),
                0,
                &mut counter,
            ) != 0
            {
                return Err(msg(lang, Msg::PdhCounterUnavailable));
            }
            if PdhCollectQueryData(query) != 0 {
                return Err(msg(lang, Msg::PdhCollect(1)));
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            if PdhCollectQueryData(query) != 0 {
                return Err(msg(lang, Msg::PdhCollect(2)));
            }
            let mut buf_len = 0u32;
            let mut item_count = 0u32;
            let st = PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_LARGE,
                &mut buf_len,
                &mut item_count,
                None,
            );
            if st != PDH_MORE_DATA || buf_len == 0 || item_count == 0 {
                return Err(msg(lang, Msg::PdhNoInstance));
            }
            let mut buf = vec![0u8; buf_len as usize];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_LARGE,
                &mut buf_len,
                &mut item_count,
                Some(items),
            ) != 0
            {
                return Err(msg(lang, Msg::PdhRead));
            }
            let mut used = 0u64;
            for i in 0..item_count as usize {
                let item = &*items.add(i);
                if item.FmtValue.CStatus == 0 {
                    used += item.FmtValue.Anonymous.largeValue.max(0) as u64;
                }
            }
            Ok(used)
        })();
        PdhCloseQuery(query);
        used
    }
}

#[cfg(not(windows))]
pub fn query_gpu_memory(lang: Lang) -> Result<GpuMemory, String> {
    Err(msg(lang, Msg::GpuMemUnavailable))
}

/// Tauri command: live VRAM counters for the topbar.
/// `app` is injected by Tauri (invisible to the frontend `invoke`).
#[tauri::command]
pub fn gpu_memory(app: tauri::AppHandle) -> Result<GpuMemory, String> {
    use crate::locale::lang_of;
    query_gpu_memory(lang_of(&app))
}
