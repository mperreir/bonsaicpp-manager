//! Hugging Face models: local catalog + download.
//!
//! Default: `prism-ml/Ternary-Bonsai-2-27B-gguf` (`*PQ2_0.gguf` files for
//! speed, `*Q2_0_g64.gguf` for compatibility). Any public repo can be used
//! via the free-form field; optional HF token for private repos.

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::locale::{lang_of, msg, Lang, Msg};
use crate::state::models_root;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModel {
    pub file: String,
    pub size_bytes: u64,
    pub repo: String,
    pub quant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteFile {
    pub name: String,
    pub size: Option<u64>,
    pub recommended: bool,
    pub universal: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct HfSibling {
    rfilename: String,
}

#[derive(Debug, Clone, Deserialize)]
struct HfModelInfo {
    siblings: Option<Vec<HfSibling>>,
}

fn quant_of(name: &str, lang: Lang) -> String {
    let lower = name.to_lowercase();
    for q in ["pq2_0", "ptq1_0", "q2_0_g64", "q2_0", "q4_0", "q5_0", "q8_0", "f16"] {
        if lower.contains(q) {
            return q.to_uppercase();
        }
    }
    if lower.ends_with(".gguf") {
        return "GGUF".to_string();
    }
    if lang == Lang::En {
        "OTHER".to_string()
    } else {
        "AUTRE".to_string()
    }
}

#[tauri::command]
pub async fn list_local_models(app: AppHandle) -> Result<Vec<LocalModel>, String> {
    let root = models_root(&app)?;
    let mut out = vec![];
    if !root.exists() {
        return Ok(out);
    }
    // models/bonsai2-gguf/<version>/<files>
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| e.to_string())?;
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
                let repo = p
                    .parent()
                    .and_then(|par| par.file_name())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                out.push(LocalModel {
                    file: p.to_string_lossy().to_string(),
                    size_bytes: e.metadata().map(|m| m.len()).unwrap_or(0),
                    repo: name.clone(),
                    quant: quant_of(&name, lang_of(&app)),
                });
                let _ = repo;
            }
        }
    }
    out.sort_by(|a, b| b.size_bytes.cmp(&a.size_bytes));
    Ok(out)
}

#[tauri::command]
pub async fn list_remote_files(
    app: AppHandle,
    repo: String,
    token: Option<String>,
) -> Result<Vec<RemoteFile>, String> {
    let lang = lang_of(&app);
    let url = format!("https://huggingface.co/api/models/{}", repo);
    let mut req = reqwest::Client::new().get(&url);
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        req = req.bearer_auth(t);
    }
    let info: HfModelInfo = req
        .send()
        .await
        .map_err(|e| format!("API Hugging Face: {e}"))?
        .error_for_status()
        .map_err(|e| msg(lang, Msg::RepoPrivate(e.to_string())))?
        .json()
        .await
        .map_err(|e| format!("JSON HF: {e}"))?;
    let mut files: Vec<RemoteFile> = info
        .siblings
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.rfilename)
        .filter(|n| n.ends_with(".gguf"))
        .map(|n| {
            let lower = n.to_lowercase();
            RemoteFile {
                recommended: lower.contains("pq2_0"),
                universal: lower.contains("q2_0_g64") || lower.ends_with("q2_0.gguf"),
                name: n,
                size: None,
            }
        })
        .collect();
    // mmproj / vision projectors: useful but secondary
    files.sort_by(|a, b| {
        (b.recommended as u8)
            .cmp(&(a.recommended as u8))
            .then((b.universal as u8).cmp(&(a.universal as u8)))
    });
    Ok(files)
}

#[derive(Debug, Clone, Serialize)]
struct DlProgress {
    file: String,
    downloaded: u64,
    total: Option<u64>,
    stage: String,
}

/// Subdirectory for a repo (`owner/model` → `model`, sanitized): downloads
/// are grouped per repo instead of a hardcoded model-size folder.
fn subdir_for_repo(repo: &str) -> String {
    let base = repo.rsplit('/').next().unwrap_or(repo).trim();
    let base = if base.is_empty() { "misc" } else { base };
    let clean: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if clean.is_empty() {
        "misc".to_string()
    } else {
        clean
    }
}

#[tauri::command]
pub async fn download_model_file(
    app: AppHandle,
    repo: String,
    filename: String,
    token: Option<String>,
) -> Result<String, String> {
    let root = models_root(&app)?;
    let dest_dir = root.join(subdir_for_repo(&repo));
    std::fs::create_dir_all(&dest_dir).map_err(|e| e.to_string())?;
    let dest = dest_dir.join(&filename);
    if dest.exists() {
        return Ok(dest.to_string_lossy().to_string());
    }
    let url = format!(
        "https://huggingface.co/{}/resolve/main/{}",
        repo, filename
    );
    let tmp = dest.with_extension("part");
    // Resume: continue from the existing `.part` size (plain `File::create`
    // used to restart every interrupted multi-GB download from zero).
    let resume_from = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    let mut req = reqwest::Client::builder()
        .user_agent("bonsaicpp-manager/0.1")
        .build()
        .map_err(|e| e.to_string())?
        .get(&url);
    if let Some(t) = token.filter(|t| !t.trim().is_empty()) {
        req = req.bearer_auth(t);
    }
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
    }
    let resp = req
        .send()
        .await
        .map_err(|e| msg(lang_of(&app), Msg::Download(e.to_string())))?;
    // 416 = range past EOF (stale `.part` from a finished download that was
    // never renamed): promote it instead of failing.
    if resp.status() == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        tokio::fs::rename(&tmp, &dest).await.map_err(|e| e.to_string())?;
        return Ok(dest.to_string_lossy().to_string());
    }
    if let Err(e) = resp.error_for_status_ref() {
        // Drop an empty stale `.part` on a hard failure (404…), but keep a
        // partial file so the next attempt can resume.
        if resume_from == 0 {
            let _ = std::fs::remove_file(&tmp);
        }
        return Err(msg(lang_of(&app), Msg::HttpStatus(e.to_string())));
    }
    let resumed = resp.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    // `Content-Length` on a 206 is the *remaining* bytes: add the base back
    // so progress shows the full size.
    let total = resp
        .content_length()
        .map(|n| if resumed { n + resume_from } else { n });
    let mut stream = resp.bytes_stream();
    let mut file = if resumed {
        tokio::fs::OpenOptions::new()
            .append(true)
            .open(&tmp)
            .await
            .map_err(|e| e.to_string())?
    } else {
        // Server ignored our Range (200): restart from zero, don't append
        // stale bytes to a fresh stream.
        tokio::fs::File::create(&tmp).await.map_err(|e| e.to_string())?
    };
    let mut downloaded = if resumed { resume_from } else { 0u64 };
    let mut last = std::time::Instant::now();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| msg(lang_of(&app), Msg::Stream(e.to_string())))?;
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(|e| e.to_string())?;
        downloaded += chunk.len() as u64;
        if last.elapsed().as_millis() > 300 {
            let _ = app.emit(
                "model-download-progress",
                DlProgress {
                    file: filename.clone(),
                    downloaded,
                    total,
                    stage: "download".to_string(),
                },
            );
            last = std::time::Instant::now();
        }
    }
    drop(file);
    tokio::fs::rename(&tmp, &dest).await.map_err(|e| e.to_string())?;
    let _ = app.emit(
        "model-download-progress",
        DlProgress {
            file: filename.clone(),
            downloaded,
            total,
            stage: "done".to_string(),
        },
    );
    Ok(dest.to_string_lossy().to_string())
}

#[tauri::command]
pub async fn delete_model_file(app: AppHandle, path: String) -> Result<(), String> {
    let root = models_root(&app)?;
    let p = std::path::PathBuf::from(&path);
    if !p.starts_with(&root) {
        return Err(msg(lang_of(&app), Msg::ModelsPathOutside));
    }
    std::fs::remove_file(&p).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdir_derives_from_repo() {
        assert_eq!(subdir_for_repo("prism-ml/Ternary-Bonsai-2-27B-gguf"), "Ternary-Bonsai-2-27B-gguf");
        assert_eq!(subdir_for_repo("owner/my model:v2"), "my_model_v2");
        assert_eq!(subdir_for_repo(""), "misc");
    }
}
