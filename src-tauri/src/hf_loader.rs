use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use keyring::Entry;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, RANGE};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;

const HF_SEARCH_URL: &str = "https://huggingface.co/api/models";
const HF_TREE_URL: &str = "https://huggingface.co/api/models";
const HF_PROGRESS_EVENT: &str = "hf-download-progress";
const CREDENTIAL_SERVICE: &str = "Flux Code Hugging Face";
const CREDENTIAL_USER: &str = "access-token";

#[derive(Default)]
pub struct DownloadState {
    next_id: AtomicU64,
    tasks: Arc<Mutex<HashMap<u64, Arc<DownloadControl>>>>,
}

#[derive(Default)]
struct DownloadControl {
    paused: AtomicBool,
    cancelled: AtomicBool,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct HfModel {
    #[serde(rename = "modelId")]
    pub model_id: String,
    #[serde(default)]
    pub downloads: u64,
    #[serde(default)]
    pub likes: u64,
    #[serde(default)]
    pub pipeline_tag: Option<String>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct HfFile {
    pub path: String,
    #[serde(default)]
    pub size: u64,
}

#[derive(Deserialize)]
struct HfTreeEntry {
    path: String,
    #[serde(default)]
    size: Option<u64>,
    #[serde(rename = "type")]
    entry_type: String,
}

#[derive(Serialize)]
pub struct LocalModel {
    pub path: String,
    pub name: String,
    pub size_bytes: u64,
    pub quantization: Option<String>,
    pub managed: bool,
}

#[derive(Serialize, Deserialize, Default)]
struct ModelRegistry {
    external_paths: HashSet<String>,
}

#[derive(Serialize, Clone)]
struct DownloadProgress {
    task_id: u64,
    repo_id: String,
    path: String,
    downloaded_bytes: u64,
    total_bytes: u64,
    speed_bytes_per_second: u64,
    status: String,
    message: String,
}

fn credential_entry() -> Result<Entry, String> {
    Entry::new(CREDENTIAL_SERVICE, CREDENTIAL_USER).map_err(|error| error.to_string())
}

fn saved_token() -> Option<String> {
    credential_entry().ok()?.get_password().ok()
}

fn auth_headers(token: Option<&str>) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    if let Some(token) = token {
        let value =
            HeaderValue::from_str(&format!("Bearer {token}")).map_err(|error| error.to_string())?;
        headers.insert(AUTHORIZATION, value);
    }
    Ok(headers)
}

fn validate_repo_id(repo_id: &str) -> Result<(), String> {
    if repo_id.is_empty()
        || repo_id.starts_with('/')
        || repo_id.ends_with('/')
        || repo_id
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || !repo_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_./".contains(character))
    {
        return Err("Invalid Hugging Face repository id".to_string());
    }
    Ok(())
}

fn validate_relative_path(path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if path.is_empty()
        || relative
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("gguf"))
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("Only safe relative GGUF paths can be downloaded".to_string());
    }
    Ok(relative.to_path_buf())
}

fn hf_download_url(repo_id: &str, file_path: &str) -> Result<reqwest::Url, String> {
    validate_repo_id(repo_id)?;
    validate_relative_path(file_path)?;
    let mut url =
        reqwest::Url::parse("https://huggingface.co/").map_err(|error| error.to_string())?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| "Invalid download URL".to_string())?;
        segments.pop_if_empty();
        segments
            .extend(repo_id.split('/'))
            .push("resolve")
            .push("main");
        segments.extend(file_path.split('/'));
    }
    url.query_pairs_mut().append_pair("download", "true");
    Ok(url)
}

#[tauri::command]
pub async fn search_hf_models(query: String) -> Result<Vec<HfModel>, String> {
    let query = query.trim();
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let mut request = client.get(HF_SEARCH_URL).query(&[
        ("filter", "gguf"),
        ("sort", "downloads"),
        ("direction", "-1"),
        ("limit", "30"),
    ]);
    if !query.is_empty() {
        request = request.query(&[("search", query)]);
    }
    let models = request
        .headers(auth_headers(saved_token().as_deref())?)
        .send()
        .await
        .map_err(|error| format!("Hugging Face search failed: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Hugging Face search failed: {error}"))?
        .json::<Vec<HfModel>>()
        .await
        .map_err(|error| format!("Could not read Hugging Face search results: {error}"))?;
    Ok(models)
}

#[tauri::command]
pub async fn list_hf_gguf_files(repo_id: String) -> Result<Vec<HfFile>, String> {
    validate_repo_id(&repo_id)?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let mut entries = Vec::new();
    let mut next_url = Some(format!(
        "{HF_TREE_URL}/{repo_id}/tree/main?recursive=true&expand=true&limit=100"
    ));
    let headers = auth_headers(saved_token().as_deref())?;
    while let Some(url) = next_url.take() {
        let response = client
            .get(&url)
            .headers(headers.clone())
            .send()
            .await
            .map_err(|error| format!("Hugging Face file-list request failed: {error}"))?
            .error_for_status()
            .map_err(|error| format!("Hugging Face file-list request failed: {error}"))?;
        next_url = response
            .headers()
            .get("link")
            .and_then(|value| value.to_str().ok())
            .and_then(next_link_url);
        let mut page = response
            .json::<Vec<HfTreeEntry>>()
            .await
            .map_err(|error| format!("Could not read Hugging Face file list: {error}"))?;
        entries.append(&mut page);
        if entries.len() > 10_000 {
            return Err("This repository has too many files to list safely".to_string());
        }
    }
    Ok(entries
        .into_iter()
        .filter(|entry| {
            entry.entry_type == "file" && entry.path.to_ascii_lowercase().ends_with(".gguf")
        })
        .map(|entry| HfFile {
            path: entry.path,
            size: entry.size.unwrap_or_default(),
        })
        .collect())
}

fn next_link_url(header: &str) -> Option<String> {
    header.split(',').find_map(|part| {
        let (url, relation) = part.trim().split_once(';')?;
        relation.contains("rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

#[tauri::command]
pub fn save_hf_token(token: String) -> Result<(), String> {
    let token = token.trim();
    if token.is_empty() {
        return Err("Token cannot be empty".to_string());
    }
    credential_entry()?
        .set_password(token)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_hf_token() -> Result<(), String> {
    match credential_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn get_hf_token_status() -> bool {
    saved_token().is_some()
}

#[tauri::command]
pub fn start_hf_download(
    app: AppHandle,
    state: State<'_, DownloadState>,
    repo_id: String,
    files: Vec<HfFile>,
) -> Result<u64, String> {
    validate_repo_id(&repo_id)?;
    if files.is_empty() {
        return Err("Select at least one GGUF file".to_string());
    }
    for file in &files {
        validate_relative_path(&file.path)?;
    }

    let task_id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let control = Arc::new(DownloadControl::default());
    state
        .tasks
        .lock()
        .map_err(|error| error.to_string())?
        .insert(task_id, control.clone());
    let tasks = Arc::clone(&state.tasks);
    let token = saved_token();
    tauri::async_runtime::spawn(async move {
        run_download_queue(app.clone(), task_id, repo_id, files, token, control).await;
        if let Ok(mut tasks) = tasks.lock() {
            tasks.remove(&task_id);
        }
    });
    Ok(task_id)
}

#[tauri::command]
pub fn set_hf_download_paused(
    state: State<'_, DownloadState>,
    task_id: u64,
    paused: bool,
) -> Result<(), String> {
    let tasks = state.tasks.lock().map_err(|error| error.to_string())?;
    let task = tasks
        .get(&task_id)
        .ok_or_else(|| "Download task not found".to_string())?;
    task.paused.store(paused, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub fn cancel_hf_download(state: State<'_, DownloadState>, task_id: u64) -> Result<(), String> {
    let tasks = state.tasks.lock().map_err(|error| error.to_string())?;
    let task = tasks
        .get(&task_id)
        .ok_or_else(|| "Download task not found".to_string())?;
    task.cancelled.store(true, Ordering::Relaxed);
    Ok(())
}

fn emit_progress(app: &AppHandle, progress: DownloadProgress) {
    let _ = app.emit(HF_PROGRESS_EVENT, progress);
}

async fn run_download_queue(
    app: AppHandle,
    task_id: u64,
    repo_id: String,
    files: Vec<HfFile>,
    token: Option<String>,
    control: Arc<DownloadControl>,
) {
    let Some(app_data) = app.path().app_data_dir().ok() else {
        return;
    };
    let model_dir = app_data.join("models").join(&repo_id);
    let client = reqwest::Client::new();
    for file in files {
        if control.cancelled.load(Ordering::Relaxed) {
            emit_progress(
                &app,
                progress(
                    task_id,
                    &repo_id,
                    &file.path,
                    0,
                    file.size,
                    0,
                    "cancelled",
                    "Download cancelled",
                ),
            );
            return;
        }
        match download_file(
            &app,
            &client,
            task_id,
            &repo_id,
            &model_dir,
            &file,
            token.as_deref(),
            &control,
        )
        .await
        {
            Ok(()) => {}
            Err(error) => {
                emit_progress(
                    &app,
                    progress(
                        task_id, &repo_id, &file.path, 0, file.size, 0, "failed", &error,
                    ),
                );
                return;
            }
        }
        if control.cancelled.load(Ordering::Relaxed) {
            return;
        }
    }
    emit_progress(
        &app,
        progress(task_id, &repo_id, "", 0, 0, 0, "complete", "Queue complete"),
    );
}

#[allow(clippy::too_many_arguments)] // The arguments map directly to independent download progress fields.
fn progress(
    task_id: u64,
    repo_id: &str,
    path: &str,
    downloaded: u64,
    total: u64,
    speed: u64,
    status: &str,
    message: &str,
) -> DownloadProgress {
    DownloadProgress {
        task_id,
        repo_id: repo_id.to_string(),
        path: path.to_string(),
        downloaded_bytes: downloaded,
        total_bytes: total,
        speed_bytes_per_second: speed,
        status: status.to_string(),
        message: message.to_string(),
    }
}

#[allow(clippy::too_many_arguments)] // Each argument is a distinct download control or immutable request value.
async fn download_file(
    app: &AppHandle,
    client: &reqwest::Client,
    task_id: u64,
    repo_id: &str,
    model_dir: &Path,
    file: &HfFile,
    token: Option<&str>,
    control: &DownloadControl,
) -> Result<(), String> {
    let relative = validate_relative_path(&file.path)?;
    let destination = model_dir.join(relative);
    let parent = destination
        .parent()
        .ok_or_else(|| "Invalid model file path".to_string())?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| error.to_string())?;
    let partial = destination.with_extension("gguf.part");
    let existing = tokio::fs::metadata(&partial)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    if existing == file.size && file.size > 0 {
        tokio::fs::rename(&partial, &destination)
            .await
            .map_err(|error| error.to_string())?;
        return Ok(());
    }

    let url = hf_download_url(repo_id, &file.path)?;
    let mut request = client.get(url).headers(auth_headers(token)?);
    if existing > 0 {
        request = request.header(RANGE, format!("bytes={existing}-"));
    }
    let response = request
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let append = existing > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT;
    let initial_bytes = if append { existing } else { 0 };
    let mut output = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(append)
        .truncate(!append)
        .open(&partial)
        .await
        .map_err(|error| error.to_string())?;
    let mut stream = response.bytes_stream();
    let mut downloaded = initial_bytes;
    let started = Instant::now();
    while let Some(chunk) = stream.next().await {
        while control.paused.load(Ordering::Relaxed) && !control.cancelled.load(Ordering::Relaxed) {
            emit_progress(
                app,
                progress(
                    task_id,
                    repo_id,
                    &file.path,
                    downloaded,
                    file.size,
                    0,
                    "paused",
                    "Download paused",
                ),
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if control.cancelled.load(Ordering::Relaxed) {
            output.flush().await.map_err(|error| error.to_string())?;
            emit_progress(
                app,
                progress(
                    task_id,
                    repo_id,
                    &file.path,
                    downloaded,
                    file.size,
                    0,
                    "cancelled",
                    "Partial file kept for resume",
                ),
            );
            return Ok(());
        }
        let chunk = chunk.map_err(|error| error.to_string())?;
        output
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        downloaded += chunk.len() as u64;
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        emit_progress(
            app,
            progress(
                task_id,
                repo_id,
                &file.path,
                downloaded,
                file.size,
                ((downloaded - initial_bytes) as f64 / elapsed) as u64,
                "downloading",
                "Downloading GGUF file",
            ),
        );
    }
    output.flush().await.map_err(|error| error.to_string())?;
    drop(output);
    if downloaded != file.size {
        return Err(format!(
            "Size check failed for {}: expected {}, received {}",
            file.path, file.size, downloaded
        ));
    }
    tokio::fs::rename(&partial, &destination)
        .await
        .map_err(|error| error.to_string())?;
    emit_progress(
        app,
        progress(
            task_id,
            repo_id,
            &file.path,
            downloaded,
            file.size,
            0,
            "file-complete",
            "File verified",
        ),
    );
    Ok(())
}

fn models_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("models"))
        .map_err(|error| error.to_string())
}

fn registry_path(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("model-registry.json"))
        .map_err(|error| error.to_string())
}

fn read_registry(path: &Path) -> Result<ModelRegistry, String> {
    match fs::read(path) {
        Ok(contents) => serde_json::from_slice(&contents).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(ModelRegistry::default()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn list_local_models(app: AppHandle) -> Result<Vec<LocalModel>, String> {
    let directory = models_dir(&app)?;
    let mut paths = Vec::new();
    if let Ok(entries) = fs::read_dir(&directory) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
            {
                paths.push(path);
            }
        }
    }
    let registry = read_registry(&registry_path(&app)?)?;
    for external in registry.external_paths {
        let path = PathBuf::from(&external);
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    let models = paths
        .into_iter()
        .filter_map(|path| {
            let metadata = fs::metadata(&path).ok()?;
            let name = path.file_name()?.to_string_lossy().to_string();
            let upper = name.to_ascii_uppercase();
            let quantization = upper
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .find(|part| {
                    part.starts_with('Q')
                        && part
                            .chars()
                            .skip(1)
                            .any(|character| character.is_ascii_digit())
                })
                .map(str::to_string);
            Some(LocalModel {
                path: path.to_string_lossy().to_string(),
                name,
                size_bytes: metadata.len(),
                quantization,
                managed: path.starts_with(&directory),
            })
        })
        .collect();
    Ok(models)
}

#[tauri::command]
pub fn delete_local_model(app: AppHandle, model_path: String) -> Result<(), String> {
    let directory = models_dir(&app)?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let model = PathBuf::from(&model_path)
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !model.starts_with(&root) {
        let registry_path = registry_path(&app)?;
        let mut registry = read_registry(&registry_path)?;
        registry.external_paths.remove(&model_path);
        registry
            .external_paths
            .remove(&model.to_string_lossy().to_string());
        let bytes = serde_json::to_vec_pretty(&registry).map_err(|error| error.to_string())?;
        fs::write(registry_path, bytes).map_err(|error| error.to_string())?;
        return Ok(());
    }
    if !model.starts_with(&root)
        || !model.is_file()
        || model
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("gguf"))
    {
        return Err("Model must be a GGUF file inside the local models folder".to_string());
    }
    fs::remove_file(model).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn import_local_gguf(app: AppHandle, source_path: String) -> Result<String, String> {
    let source = PathBuf::from(source_path)
        .canonicalize()
        .map_err(|error| format!("Could not open model file: {error}"))?;
    if !source.is_file()
        || source
            .extension()
            .and_then(|extension| extension.to_str())
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("gguf"))
    {
        return Err("Choose the .gguf model file itself, not its containing folder".to_string());
    }
    let registry_file = registry_path(&app)?;
    let mut registry = read_registry(&registry_file)?;
    registry
        .external_paths
        .insert(source.to_string_lossy().to_string());
    let bytes = serde_json::to_vec_pretty(&registry).map_err(|error| error.to_string())?;
    fs::write(registry_file, bytes).map_err(|error| error.to_string())?;
    Ok(source.to_string_lossy().to_string())
}

#[tauri::command]
pub fn find_gguf_in_folder(folder_path: String) -> Result<Vec<String>, String> {
    let directory = PathBuf::from(folder_path);
    if !directory.is_dir() {
        return Err("Select a folder that contains GGUF files".to_string());
    }
    let entries = fs::read_dir(directory).map_err(|error| error.to_string())?;
    let mut models = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
        {
            models.push(path.to_string_lossy().to_string());
            if models.len() >= 20 {
                break;
            }
        }
    }
    models.sort();
    Ok(models)
}

#[tauri::command]
pub fn open_models_folder(app: AppHandle) -> Result<(), String> {
    let path = models_dir(&app)?;
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    #[cfg(windows)]
    std::process::Command::new("explorer")
        .arg(path)
        .spawn()
        .map_err(|error| error.to_string())?;
    #[cfg(not(windows))]
    return Err("Opening the model folder is currently supported on Windows".to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{hf_download_url, validate_relative_path, validate_repo_id};

    #[test]
    fn accepts_nested_gguf_shards() {
        assert!(validate_relative_path("Q4/model-00001-of-00003.gguf").is_ok());
    }

    #[test]
    fn rejects_paths_that_escape_the_model_directory() {
        assert!(validate_relative_path("../../outside.gguf").is_err());
        assert!(validate_repo_id("owner/../other").is_err());
    }

    #[test]
    fn builds_download_url_from_nested_repo_and_file_segments() {
        let url = hf_download_url("owner/model", "Q4/model shard-00001-of-00002.gguf").unwrap();
        assert_eq!(
            url.path(),
            "/owner/model/resolve/main/Q4/model%20shard-00001-of-00002.gguf"
        );
    }
}
