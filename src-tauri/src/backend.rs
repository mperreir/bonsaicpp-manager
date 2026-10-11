//! Installation and status of the llama.cpp backend (PrismML-Eng fork).
//!
//! Strategy: download the prebuilt binaries from GitHub Releases
//! (same approach as `Bonsai-demo/setup.ps1`), never build from source.
//! Layout: `<app_data>/bin/<tag>/<flavor>/llama-server.exe`

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Emitter, State};

use crate::locale::{lang_of, msg, Msg};
use crate::state::{backend_root, ServerState};

pub const UPSTREAM_OWNER: &str = "PrismML-Eng";
pub const UPSTREAM_REPO: &str = "llama.cpp";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendStatus {
    pub installed: bool,
    pub tag: Option<String>,
    pub flavor: Option<String>,
    pub server_exe: Option<String>,
    pub size_bytes: Option<u64>,
    pub suggested_flavor: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledBackend {
    pub tag: String,
    pub flavor: String,
    pub server_exe: String,
    pub size_bytes: u64,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseInfo {
    pub tag: String,
    pub name: String,
    pub published_at: Option<String>,
    pub assets: Vec<AssetInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub size: u64,
    pub download_url: String,
    pub flavor: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GhRelease {
    tag_name: String,
    name: Option<String>,
    published_at: Option<String>,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GhAsset {
    name: String,
    size: u64,
    browser_download_url: String,
}

/// Guess the flavor from the asset name (x64 Windows full binaries only).
///
/// Excluded: non-zip archives, non-Windows builds, ARM64 builds (this app
/// ships an x64 installer), CUDA runtime-only `cudart-*` zips (no
/// `llama-server.exe` inside — installing one leaves a backend dir without
/// a binary), and unsupported flavors (SYCL, xcframework… → None).
pub fn classify_asset(name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    if !lower.ends_with(".zip") {
        return None;
    }
    if !(lower.contains("win") || lower.contains("windows")) {
        return None;
    }
    // x64-only app: never pick an ARM64 binary.
    if lower.contains("arm64") || lower.contains("aarch64") {
        return None;
    }
    // Runtime DLLs only, not a runnable backend.
    if lower.starts_with("cudart-") || lower.contains("/cudart-") {
        return None;
    }
    if lower.contains("cuda") {
        Some("cuda".to_string())
    } else if lower.contains("vulkan") {
        Some("vulkan".to_string())
    } else if lower.contains("hip") || lower.contains("rocm") || lower.contains("radeon") {
        Some("hip".to_string())
    } else if lower.contains("cpu") {
        Some("cpu".to_string())
    } else {
        None
    }
}

/// Natural (human) ordering for release tags: digit runs compare as numbers,
/// other chunks lexicographically. Fixes lexicographic misordering
/// (`b99` > `b100` as plain strings) for tags like `prism-b10770-…`.
fn compare_tags(a: &str, b: &str) -> std::cmp::Ordering {
    fn chunks(s: &str) -> Vec<&str> {
        let b = s.as_bytes();
        let mut out = vec![];
        let mut i = 0;
        while i < b.len() {
            let digit = b[i].is_ascii_digit();
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() == digit {
                j += 1;
            }
            out.push(&s[i..j]);
            i = j;
        }
        out
    }
    let (ca, cb) = (chunks(a), chunks(b));
    for (x, y) in ca.iter().zip(cb.iter()) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            _ => x.cmp(y),
        };
        if ord != std::cmp::Ordering::Equal {
            return ord;
        }
    }
    ca.len().cmp(&cb.len())
}

/// GPU detection: nvidia-smi -> cuda, otherwise video card names (WMI):
/// AMD/Radeon -> hip, Intel Arc -> vulkan, otherwise cpu.
#[tauri::command]
pub async fn detect_gpu() -> Result<String, String> {
    // CREATE_NO_WINDOW: avoid flashing a console window from the release
    // (windows-subsystem) build when probing helper executables.
    // (tokio::process::Command exposes creation_flags() directly on Windows.)
    #[cfg(windows)]
    const NO_WINDOW: u32 = 0x08000000;
    let mut probe_cmd = tokio::process::Command::new("nvidia-smi");
    #[cfg(windows)]
    probe_cmd.creation_flags(NO_WINDOW);
    let probe = probe_cmd.arg("-L").output().await;
    match probe {
        Ok(out) if out.status.success() && !out.stdout.is_empty() => {
            return Ok("cuda".to_string())
        }
        _ => {}
    }
    // No NVIDIA: list the cards (covers AMD without rocm-smi,
    // nonexistent on Windows, and Intel iGPUs with no specific driver).
    let wmi = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        {
            let mut c = tokio::process::Command::new("powershell");
            #[cfg(windows)]
            c.creation_flags(NO_WINDOW);
            c.args([
                "-NoProfile",
                "-ExecutionPolicy",
                "Bypass",
                "-Command",
                "Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name",
            ])
            .output()
        },
    )
    .await;
    if let Ok(Ok(out)) = wmi {
        let names = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if names.contains("amd") || names.contains("radeon") {
            return Ok("hip".to_string());
        }
        if names.contains("nvidia") || names.contains("geforce") || names.contains("quadro") {
            return Ok("cuda".to_string());
        }
        if names.contains("arc") {
            return Ok("vulkan".to_string());
        }
    }
    Ok("cpu".to_string())
}

fn dir_size(path: &PathBuf) -> u64 {
    let mut total = 0u64;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_file() {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            } else if p.is_dir() {
                total += dir_size(&p);
            }
        }
    }
    total
}

/// File persisting the user's default backend (`<root>/default.txt`, `tag/flavor`).
const DEFAULT_FILE: &str = "default.txt";

fn read_default(root: &PathBuf) -> Option<(String, String)> {
    let text = std::fs::read_to_string(root.join(DEFAULT_FILE)).ok()?;
    let (tag, flavor) = text.trim().split_once('/')?;
    let tag = tag.trim();
    let flavor = flavor.trim();
    if tag.is_empty() || flavor.is_empty() {
        return None;
    }
    Some((tag.to_string(), flavor.to_string()))
}

fn write_default(root: &PathBuf, tag: &str, flavor: &str) -> Result<(), String> {
    std::fs::write(root.join(DEFAULT_FILE), format!("{tag}/{flavor}"))
        .map_err(|e| e.to_string())
}

/// All installed binaries (newest tag first).
pub fn list_all_installed(root: &PathBuf) -> Vec<(String, String, PathBuf)> {
    let mut candidates: Vec<(String, String, PathBuf)> = vec![];
    let tags = match std::fs::read_dir(root) {
        Ok(t) => t,
        Err(_) => return candidates,
    };
    for tag_entry in tags.flatten() {
        let tag = tag_entry.file_name().to_string_lossy().to_string();
        if !tag_entry.path().is_dir() {
            continue;
        }
        if let Ok(flavors) = std::fs::read_dir(tag_entry.path()) {
            for fl in flavors.flatten() {
                let flavor = fl.file_name().to_string_lossy().to_string();
                let exe = fl.path().join("llama-server.exe");
                let exe2 = fl.path().join("bin").join("llama-server.exe");
                if exe.is_file() {
                    candidates.push((tag.clone(), flavor, exe));
                } else if exe2.is_file() {
                    candidates.push((tag.clone(), flavor, exe2));
                }
            }
        }
    }
    candidates.sort_by(|a, b| compare_tags(&b.0, &a.0));
    candidates
}

/// Find the installed binary: the persisted default if still present,
/// otherwise the newest.
pub fn find_installed(root: &PathBuf) -> Option<(String, String, PathBuf)> {
    let all = list_all_installed(root);
    if all.is_empty() {
        return None;
    }
    if let Some((tag, flavor)) = read_default(root) {
        if let Some(hit) = all
            .iter()
            .find(|(t, f, _)| t == &tag && f == &flavor)
            .cloned()
        {
            return Some(hit);
        }
    }
    all.into_iter().next()
}

/// All installed backends with sizes, default first-flagged.
#[tauri::command]
pub async fn list_installed_backends(app: AppHandle) -> Result<Vec<InstalledBackend>, String> {
    let root = backend_root(&app)?;
    let all = list_all_installed(&root);
    let def = find_installed(&root);
    Ok(all
        .into_iter()
        .map(|(tag, flavor, exe)| {
            let is_default = def
                .as_ref()
                .map(|(t, f, _)| t == &tag && f == &flavor)
                .unwrap_or(false);
            let size = dir_size(&exe.parent().unwrap().to_path_buf());
            InstalledBackend {
                tag,
                flavor,
                server_exe: exe.to_string_lossy().to_string(),
                size_bytes: size,
                is_default,
            }
        })
        .collect())
}

/// Set the default backend (must be installed).
#[tauri::command]
pub async fn set_default_backend(
    app: AppHandle,
    tag: String,
    flavor: String,
) -> Result<Vec<InstalledBackend>, String> {
    let root = backend_root(&app)?;
    let ok = list_all_installed(&root)
        .iter()
        .any(|(t, f, _)| t == &tag && f == &flavor);
    if !ok {
        return Err(msg(lang_of(&app), Msg::BackendTagNotInstalled { tag, flavor }));
    }
    write_default(&root, &tag, &flavor)?;
    list_installed_backends(app).await
}

/// Delete an installed backend. Refused while the server runs
/// (stop it first — deleting the running binary is never safe).
#[tauri::command]
pub async fn delete_backend(
    app: AppHandle,
    state: State<'_, ServerState>,
    tag: String,
    flavor: String,
) -> Result<Vec<InstalledBackend>, String> {
    if crate::server_cmd::is_server_running(&state).await {
        return Err(msg(lang_of(&app), Msg::ServerRunningStopFirst));
    }
    let root = backend_root(&app)?;
    let dir = root.join(&tag).join(&flavor);
    if !dir.is_dir() {
        return Err(msg(lang_of(&app), Msg::BackendNotFound { tag, flavor }));
    }
    // Safety: only delete inside our own bin dir.
    if !dir.starts_with(&root) {
        return Err(msg(lang_of(&app), Msg::BackendPathOutside));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    // Prune an empty tag dir; drop a stale default pointer.
    let tag_dir = root.join(&tag);
    if tag_dir.is_dir() && std::fs::read_dir(&tag_dir).map(|mut d| d.next().is_none()).unwrap_or(false) {
        let _ = std::fs::remove_dir(&tag_dir);
    }
    if read_default(&root).map(|(t, f)| t == tag && f == flavor).unwrap_or(false) {
        let _ = std::fs::remove_file(root.join(DEFAULT_FILE));
    }
    list_installed_backends(app).await
}

#[tauri::command]
pub async fn backend_status(app: AppHandle) -> Result<BackendStatus, String> {
    let root = backend_root(&app)?;
    let suggested = detect_gpu().await.unwrap_or_else(|_| "cpu".to_string());
    match find_installed(&root) {
        Some((tag, flavor, exe)) => {
            let size = dir_size(&exe.parent().unwrap().to_path_buf());
            Ok(BackendStatus {
                installed: true,
                tag: Some(tag),
                flavor: Some(flavor),
                server_exe: Some(exe.to_string_lossy().to_string()),
                size_bytes: Some(size),
                suggested_flavor: suggested,
            })
        }
        None => Ok(BackendStatus {
            installed: false,
            tag: None,
            flavor: None,
            server_exe: None,
            size_bytes: None,
            suggested_flavor: suggested,
        }),
    }
}

#[tauri::command]
pub async fn list_backend_releases(app: AppHandle, limit: Option<u8>) -> Result<Vec<ReleaseInfo>, String> {
    let lang = lang_of(&app);
    let limit = limit.unwrap_or(5).min(10);
    let url = format!(
        "https://api.github.com/repos/{}/{}/releases?per_page={}",
        UPSTREAM_OWNER, UPSTREAM_REPO, limit
    );
    let client = reqwest::Client::builder()
        .user_agent("bonsaicpp-manager/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let releases: Vec<GhRelease> = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("GitHub API: {e}"))?
        .error_for_status()
        .map_err(|e| msg(lang, Msg::GithubStatus(e.to_string())))?
        .json()
        .await
        .map_err(|e| format!("JSON GitHub: {e}"))?;
    Ok(releases
        .into_iter()
        .map(|r| ReleaseInfo {
            tag: r.tag_name,
            name: r.name.unwrap_or_default(),
            published_at: r.published_at,
            assets: r
                .assets
                .into_iter()
                .filter_map(|a| {
                    classify_asset(&a.name).map(|flavor| AssetInfo {
                        name: a.name,
                        size: a.size,
                        download_url: a.browser_download_url,
                        flavor,
                    })
                })
                .collect(),
        })
        .collect())
}

#[derive(Debug, Clone, Serialize)]
struct InstallProgress {
    stage: String,
    downloaded: u64,
    total: Option<u64>,
}

/// Download a release asset and unzip it into <bin>/<tag>/<flavor>/.
/// Emit `backend-install-progress` events to the frontend.
/// If a server runs, it is stopped first (never overwrite a running binary).
/// The new backend becomes the default.
#[tauri::command]
pub async fn install_backend(
    app: AppHandle,
    state: State<'_, ServerState>,
    tag: String,
    flavor: String,
    download_url: String,
) -> Result<BackendStatus, String> {
    use futures_util::StreamExt;

    if crate::server_cmd::is_server_running(&state).await {
        crate::server_cmd::stop_server_impl(&app, &state).await?;
    }

    let root = backend_root(&app)?;
    let dest_dir = root.join(&tag).join(&flavor);
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let zip_path = dest_dir.join("backend.zip");

    let client = reqwest::Client::builder()
        .user_agent("bonsaicpp-manager/0.1")
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| msg(lang_of(&app), Msg::Download(e.to_string())))?
        .error_for_status()
        .map_err(|e| msg(lang_of(&app), Msg::HttpStatus(e.to_string())))?;
    let total = resp.content_length();
    let mut stream = resp.bytes_stream();
    let mut file = tokio::fs::File::create(&zip_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut downloaded = 0u64;
    let mut last_emit = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| msg(lang_of(&app), Msg::Stream(e.to_string())))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if last_emit.elapsed().as_millis() > 250 {
            let _ = app.emit(
                "backend-install-progress",
                InstallProgress {
                    stage: "download".to_string(),
                    downloaded,
                    total,
                },
            );
            last_emit = std::time::Instant::now();
        }
    }
    drop(file);
    let _ = app.emit(
        "backend-install-progress",
        InstallProgress {
            stage: "extract".to_string(),
            downloaded,
            total,
        },
    );

    // Unzip (blocking -> spawn_blocking).
    let zip_path_c = zip_path.clone();
    let dest_c = dest_dir.clone();
    let lang = lang_of(&app);
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let f = std::fs::File::open(&zip_path_c).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip: {e}"))?;
        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            // Zip-Slip guard: `enclosed_name` rejects absolute paths and
            // `..` escapes; the `starts_with` check pins extraction inside
            // our own backend dir even on case-insensitive Windows paths.
            let safe = entry.enclosed_name().ok_or(msg(lang, Msg::ZipEntryUnsafe))?;
            let out = dest_c.join(safe);
            if !out.starts_with(&dest_c) {
                return Err(msg(lang, Msg::ZipEntryOutside));
            }
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            } else {
                if let Some(parent) = out.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                let mut out_f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
                std::io::copy(&mut entry, &mut out_f).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = std::fs::remove_file(&zip_path);
    let _ = app.emit(
        "backend-install-progress",
        InstallProgress {
            stage: "done".to_string(),
            downloaded,
            total,
        },
    );
    // The freshly installed backend becomes the default.
    let _ = write_default(&root, &tag, &flavor);
    backend_status(app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_sort_numerically_not_lexicographically() {
        // Regression: plain string sort puts b99 before b100 ("9" > "1").
        let mut tags = vec!["prism-b99-abc", "prism-b100-abc", "prism-b10770-abc"];
        tags.sort_by(|a, b| compare_tags(b, a));
        assert_eq!(tags, vec!["prism-b10770-abc", "prism-b100-abc", "prism-b99-abc"]);
    }

    #[test]
    fn classify_real_release_assets() {
        // Names observed on the PrismML-Eng releases (Oct 2026).
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-cpu-x64.zip").as_deref(),
            Some("cpu")
        );
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-cuda-12.4-x64.zip").as_deref(),
            Some("cuda")
        );
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-vulkan-x64.zip").as_deref(),
            Some("vulkan")
        );
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-hip-radeon-x64.zip").as_deref(),
            Some("hip")
        );
        // CUDA runtime-only zip: must NOT be picked as a backend.
        assert_eq!(classify_asset("cudart-llama-bin-win-cuda-12.4-x64.zip"), None);
        // ARM64 binaries: this app ships an x64 installer.
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-cpu-arm64.zip"),
            None
        );
        // Other platforms / formats.
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-ubuntu-x64.tar.gz"),
            None
        );
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-xcframework.zip"),
            None
        );
        // SYCL is not a supported flavor in the UI.
        assert_eq!(
            classify_asset("llama-prism-b10770-6684606-bin-win-sycl-x64.zip"),
            None
        );
    }
}
