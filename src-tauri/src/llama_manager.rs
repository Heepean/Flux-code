use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sysinfo::System;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncWriteExt;
use zip::ZipArchive;

const RELEASES_URL: &str = "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20";
const INSTALL_PROGRESS_EVENT: &str = "llama-install-progress";
const SERVER_LOG_EVENT: &str = "llama-server-log";

#[derive(Default)]
pub struct RuntimeState {
    child: Mutex<Option<Child>>,
    port: Mutex<Option<u16>>,
}

#[derive(Serialize)]
pub struct ManagerStatus {
    pub install_dir: String,
    pub server_path: Option<String>,
    pub version: Option<String>,
    pub running: bool,
    pub port: Option<u16>,
}

#[derive(Serialize)]
pub struct MachineProfile {
    pub total_memory_bytes: u64,
    pub available_memory_bytes: u64,
    pub gpu_name: Option<String>,
    pub gpu_memory_bytes: Option<u64>,
    pub cpu_context: u32,
    pub gpu_context: u32,
    pub recommended_gpu_layers: u32,
    pub supports_vulkan_asset: bool,
}

#[derive(Serialize)]
pub struct MemoryEstimate {
    pub model_bytes: u64,
    pub kv_cache_bytes: u64,
    pub estimated_total_bytes: u64,
    pub available_bytes: u64,
    pub fits: bool,
}

#[derive(Serialize)]
pub struct ModelCapabilities {
    pub context_length: Option<u64>,
    pub tool_calling: bool,
    pub vision: bool,
    pub props: serde_json::Value,
}

#[derive(Clone, Serialize)]
struct InstallProgress {
    stage: String,
    downloaded_bytes: u64,
    total_bytes: Option<u64>,
    message: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
}

fn install_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("bin"))
        .map_err(|error| error.to_string())
}

fn emit_progress(
    app: &AppHandle,
    stage: &str,
    downloaded: u64,
    total: Option<u64>,
    message: String,
) {
    let _ = app.emit(
        INSTALL_PROGRESS_EVENT,
        InstallProgress {
            stage: stage.to_string(),
            downloaded_bytes: downloaded,
            total_bytes: total,
            message,
        },
    );
}

fn find_server(path: &Path) -> Option<PathBuf> {
    let mut pending = vec![path.to_path_buf()];
    while let Some(current) = pending.pop() {
        let Ok(entries) = fs::read_dir(current) else {
            continue;
        };
        for entry in entries.flatten() {
            let child = entry.path();
            if child.is_dir() {
                pending.push(child);
            } else if child
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("llama-server.exe"))
            {
                return Some(child);
            }
        }
    }
    None
}

fn hidden_command(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
}

fn read_version(executable: &Path) -> Option<String> {
    let mut command = Command::new(executable);
    command
        .arg("--version")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden_command(&mut command);
    let output = command.output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let text = if text.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        text
    };
    output.status.success().then_some(text)
}

#[tauri::command]
pub fn get_llama_manager_status(
    app: AppHandle,
    state: State<'_, RuntimeState>,
) -> Result<ManagerStatus, String> {
    let install_dir = install_dir(&app)?;
    let server_path = find_server(&install_dir);
    let version = server_path.as_deref().and_then(read_version);
    let mut child = state.child.lock().map_err(|error| error.to_string())?;
    let running = match child.as_mut() {
        Some(process) => match process.try_wait() {
            Ok(Some(_)) => {
                *child = None;
                if let Ok(mut port) = state.port.lock() {
                    *port = None;
                }
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    };
    let port = if running {
        *state.port.lock().map_err(|error| error.to_string())?
    } else {
        None
    };

    Ok(ManagerStatus {
        install_dir: install_dir.to_string_lossy().to_string(),
        server_path: server_path.map(|path| path.to_string_lossy().to_string()),
        version,
        running,
        port,
    })
}

fn select_asset<'a>(release: &'a GitHubRelease, flavor: &str) -> Option<&'a GitHubAsset> {
    let wanted = match flavor {
        "vulkan" => "vulkan-x64",
        _ => "cpu-x64",
    };
    release.assets.iter().find(|asset| {
        let name = asset.name.to_ascii_lowercase();
        name.contains("bin-win")
            && name.contains(wanted)
            && name.ends_with(".zip")
            && !name.contains("cudart")
    })
}

fn extract_archive(archive_path: &Path, destination: &Path) -> Result<(), String> {
    let input = File::open(archive_path).map_err(|error| error.to_string())?;
    let mut archive = ZipArchive::new(input).map_err(|error| error.to_string())?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        let Some(relative_path) = entry.enclosed_name() else {
            continue;
        };
        let output_path = destination.join(relative_path);
        if entry.is_dir() {
            fs::create_dir_all(&output_path).map_err(|error| error.to_string())?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let mut output = File::create(output_path).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub async fn install_llama(
    app: AppHandle,
    flavor: String,
    action: String,
) -> Result<ManagerStatus, String> {
    if !matches!(action.as_str(), "install" | "update" | "reinstall") {
        return Err("Unsupported installation action".to_string());
    }
    if let Some(state) = app.try_state::<RuntimeState>() {
        stop_runtime(state.inner())?;
    }

    let client = reqwest::Client::builder()
        .user_agent("FluxCode/0.1")
        .connect_timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())?;
    emit_progress(
        &app,
        "checking-release",
        0,
        None,
        format!("Searching recent releases for a Windows {flavor} build"),
    );
    let releases: Vec<GitHubRelease> = client
        .get(RELEASES_URL)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let (release, asset) = releases
        .iter()
        .find_map(|release| select_asset(release, &flavor).map(|asset| (release, asset)))
        .ok_or_else(|| {
            format!("No Windows {flavor} ZIP asset found in the latest 20 llama.cpp releases")
        })?;
    emit_progress(
        &app,
        "downloading",
        0,
        Some(asset.size),
        format!("Downloading {}", asset.name),
    );

    let response = client
        .get(&asset.browser_download_url)
        .send()
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?;
    let total_bytes = response.content_length().or(Some(asset.size));
    let app_dir = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&app_dir).map_err(|error| error.to_string())?;
    let archive_path = app_dir.join("llama-download.zip");
    let mut archive_file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|error| error.to_string())?;
    let mut stream = response.bytes_stream();
    let mut downloaded = 0u64;
    loop {
        let next_chunk = match tokio::time::timeout(Duration::from_secs(45), stream.next()).await {
            Ok(next_chunk) => next_chunk,
            Err(_) => {
                drop(archive_file);
                let _ = fs::remove_file(&archive_path);
                return Err("llama.cpp download stopped making progress for 45 seconds".to_string());
            }
        };
        let Some(chunk) = next_chunk else {
            break;
        };
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                drop(archive_file);
                let _ = fs::remove_file(&archive_path);
                return Err(error.to_string());
            }
        };
        archive_file
            .write_all(&chunk)
            .await
            .map_err(|error| error.to_string())?;
        downloaded += chunk.len() as u64;
        emit_progress(
            &app,
            "download",
            downloaded,
            total_bytes,
            format!(
                "Downloaded {} of {} bytes",
                downloaded,
                total_bytes.unwrap_or(0)
            ),
        );
    }
    archive_file
        .flush()
        .await
        .map_err(|error| error.to_string())?;
    drop(archive_file);
    if total_bytes.is_some_and(|total| total != downloaded) {
        let _ = fs::remove_file(&archive_path);
        return Err(format!(
            "Downloaded size mismatch: expected {}, received {}",
            total_bytes.unwrap_or(0),
            downloaded
        ));
    }

    let staging_dir = app_dir.join("bin-staging");
    if staging_dir.exists() {
        fs::remove_dir_all(&staging_dir).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(&staging_dir).map_err(|error| error.to_string())?;
    emit_progress(
        &app,
        "extracting",
        downloaded,
        total_bytes,
        "Extracting release archive".to_string(),
    );
    if let Err(error) = extract_archive(&archive_path, &staging_dir) {
        let _ = fs::remove_file(&archive_path);
        let _ = fs::remove_dir_all(&staging_dir);
        return Err(error);
    }
    let staged_server = find_server(&staging_dir)
        .ok_or_else(|| "The downloaded archive does not contain llama-server.exe".to_string())?;
    if read_version(&staged_server).is_none() {
        let _ = fs::remove_dir_all(&staging_dir);
        return Err("llama-server.exe failed its --version check".to_string());
    }

    let target_dir = install_dir(&app)?;
    let backup_dir = app_dir.join("bin-backup");
    if backup_dir.exists() {
        fs::remove_dir_all(&backup_dir).map_err(|error| error.to_string())?;
    }
    if target_dir.exists() {
        fs::rename(&target_dir, &backup_dir).map_err(|error| error.to_string())?;
    }
    if let Err(error) = fs::rename(&staging_dir, &target_dir) {
        if backup_dir.exists() {
            let _ = fs::rename(&backup_dir, &target_dir);
        }
        return Err(error.to_string());
    }
    if backup_dir.exists() {
        let _ = fs::remove_dir_all(backup_dir);
    }
    let _ = fs::remove_file(archive_path);
    emit_progress(
        &app,
        "complete",
        downloaded,
        total_bytes,
        format!("Installed {}", release.tag_name),
    );
    get_status_without_state(&app)
}

fn get_status_without_state(app: &AppHandle) -> Result<ManagerStatus, String> {
    let install_dir = install_dir(app)?;
    let server_path = find_server(&install_dir);
    let version = server_path.as_deref().and_then(read_version);
    Ok(ManagerStatus {
        install_dir: install_dir.to_string_lossy().to_string(),
        server_path: server_path.map(|path| path.to_string_lossy().to_string()),
        version,
        running: false,
        port: None,
    })
}

#[tauri::command]
pub fn uninstall_llama(app: AppHandle) -> Result<(), String> {
    if let Some(state) = app.try_state::<RuntimeState>() {
        stop_runtime(state.inner())?;
    }
    let target_dir = install_dir(&app)?;
    if target_dir.exists() {
        fs::remove_dir_all(target_dir).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn detect_gpu() -> Option<(String, Option<u64>)> {
    let mut command = Command::new("powershell");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "Get-CimInstance Win32_VideoController | Select-Object -ExpandProperty Name",
    ]);
    hidden_command(&mut command);
    let output = command.output().ok()?;
    let names: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .collect();
    let name = names
        .iter()
        .find(|name| name.to_ascii_lowercase().contains("7700 xt"))
        .or_else(|| names.first())
        .cloned()?;
    if name.is_empty() {
        return None;
    }
    let memory = if name.to_ascii_lowercase().contains("7700 xt") {
        Some(12 * 1024 * 1024 * 1024)
    } else {
        None
    };
    Some((name, memory))
}

#[tauri::command]
pub fn get_machine_profile() -> MachineProfile {
    let mut system = System::new_all();
    system.refresh_memory();
    let total_memory_bytes = system.total_memory();
    let available_memory_bytes = system.available_memory();
    let gpu = detect_gpu();
    let supports_vulkan_asset = gpu
        .as_ref()
        .is_some_and(|(name, _)| name.to_ascii_lowercase().contains("7700 xt"));
    let recommended_gpu_layers = if supports_vulkan_asset { 99 } else { 0 };
    let cpu_context = if total_memory_bytes < 16 * 1024 * 1024 * 1024 {
        4096
    } else {
        8192
    };
    let gpu_context = if supports_vulkan_asset {
        8192
    } else {
        cpu_context
    };

    MachineProfile {
        total_memory_bytes,
        available_memory_bytes,
        gpu_name: gpu.as_ref().map(|(name, _)| name.clone()),
        gpu_memory_bytes: gpu.and_then(|(_, memory)| memory),
        cpu_context,
        gpu_context,
        recommended_gpu_layers,
        supports_vulkan_asset,
    }
}

#[tauri::command]
pub fn estimate_model_memory(
    model_path: String,
    context_size: u32,
    gpu_layers: u32,
) -> Result<MemoryEstimate, String> {
    let model_bytes = fs::metadata(&model_path)
        .map_err(|error| error.to_string())?
        .len();
    let profile = get_machine_profile();
    let kv_cache_bytes = u64::from(context_size).saturating_mul(128 * 1024);
    // Include runtime overhead (graph/work buffers and allocator slack) so a borderline estimate does not page the OS.
    let estimated_total_bytes = model_bytes
        .saturating_add(kv_cache_bytes)
        .saturating_add(model_bytes / 5)
        .saturating_add(1024 * 1024 * 1024);
    let available_bytes = profile
        .available_memory_bytes
        .saturating_add(if gpu_layers > 0 {
            profile.gpu_memory_bytes.unwrap_or(0)
        } else {
            0
        });
    Ok(MemoryEstimate {
        model_bytes,
        kv_cache_bytes,
        estimated_total_bytes,
        available_bytes,
        fits: estimated_total_bytes <= available_bytes,
    })
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

fn pipe_logs(app: AppHandle, stream: impl Read + Send + 'static, source: &'static str) {
    thread::spawn(move || {
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            let _ = app.emit(SERVER_LOG_EVENT, format!("[{source}] {line}"));
        }
    });
}

#[tauri::command]
pub async fn start_llama_server(
    app: AppHandle,
    state: State<'_, RuntimeState>,
    model_path: String,
    context_size: u32,
    gpu_layers: u32,
) -> Result<u16, String> {
    let model = PathBuf::from(model_path);
    if !model.is_file()
        || !model
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("gguf"))
    {
        return Err("Select an existing GGUF model file".to_string());
    }
    if !(512..=65_536).contains(&context_size) {
        return Err("Context size must be between 512 and 65536 tokens".to_string());
    }
    if gpu_layers > 99 {
        return Err("GPU layer count cannot exceed 99".to_string());
    }
    {
        let mut child = state.child.lock().map_err(|error| error.to_string())?;
        if let Some(process) = child.as_mut() {
            if process
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                return Err("llama-server is already running".to_string());
            }
            *child = None;
        }
    }

    let executable = find_server(&install_dir(&app)?)
        .ok_or_else(|| "Install llama.cpp before starting a model".to_string())?;
    let port = free_port()?;
    let mut command = Command::new(executable);
    command
        .arg("-m")
        .arg(&model)
        .args(["--host", "127.0.0.1", "--port"])
        .arg(port.to_string())
        .args(["-c"])
        .arg(context_size.to_string())
        .args(["-ngl"])
        .arg(gpu_layers.to_string())
        .arg("--jinja")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden_command(&mut command);
    let mut process = command.spawn().map_err(|error| error.to_string())?;
    if let Some(stdout) = process.stdout.take() {
        pipe_logs(app.clone(), stdout, "stdout");
    }
    if let Some(stderr) = process.stderr.take() {
        pipe_logs(app.clone(), stderr, "stderr");
    }
    *state.child.lock().map_err(|error| error.to_string())? = Some(process);
    *state.port.lock().map_err(|error| error.to_string())? = Some(port);

    let health_url = format!("http://127.0.0.1:{port}/health");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|error| error.to_string())?;
    for _ in 0..450 {
        if client
            .get(&health_url)
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
        {
            return Ok(port);
        }
        let exited = {
            let mut child = state.child.lock().map_err(|error| error.to_string())?;
            if let Some(process) = child.as_mut() {
                if process
                    .try_wait()
                    .map_err(|error| error.to_string())?
                    .is_some()
                {
                    *child = None;
                    *state.port.lock().map_err(|error| error.to_string())? = None;
                    true
                } else {
                    false
                }
            } else {
                true
            }
        };
        if exited {
            return Err("llama-server exited before /health became ready".to_string());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    stop_runtime(state.inner())?;
    Err("llama-server did not become healthy within 90 seconds".to_string())
}

fn stop_runtime(state: &RuntimeState) -> Result<(), String> {
    let mut child = state.child.lock().map_err(|error| error.to_string())?;
    if let Some(mut process) = child.take() {
        let _ = process.kill();
        let _ = process.wait();
    }
    *state.port.lock().map_err(|error| error.to_string())? = None;
    Ok(())
}

#[tauri::command]
pub fn stop_llama_server(state: State<'_, RuntimeState>) -> Result<(), String> {
    stop_runtime(state.inner())
}

#[tauri::command]
pub async fn get_model_capabilities(
    port: u16,
    model_path: String,
) -> Result<ModelCapabilities, String> {
    let props: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{port}/props"))
        .await
        .map_err(|error| error.to_string())?
        .error_for_status()
        .map_err(|error| error.to_string())?
        .json()
        .await
        .map_err(|error| error.to_string())?;
    let description = props.to_string().to_ascii_lowercase();
    let model_name = Path::new(&model_path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let metadata = read_gguf_metadata(Path::new(&model_path)).unwrap_or_default();
    let context_length = ["n_ctx_train", "context_length", "n_ctx"]
        .iter()
        .find_map(|key| find_number(&props, key))
        .or(metadata.context_length);
    Ok(ModelCapabilities {
        context_length,
        tool_calling: description.contains("chat_template")
            || description.contains("tool")
            || metadata.tool_calling,
        vision: description.contains("vision")
            || description.contains("clip")
            || metadata.vision
            || model_name.contains("vision"),
        props,
    })
}

#[derive(Default)]
struct GgufMetadata {
    context_length: Option<u64>,
    tool_calling: bool,
    vision: bool,
}

fn read_u32(reader: &mut impl Read) -> std::io::Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read) -> std::io::Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

fn read_gguf_string(reader: &mut (impl Read + Seek)) -> std::io::Result<String> {
    let length = read_u64(reader)?;
    if length > 16 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "GGUF string is too large",
        ));
    }
    let mut bytes = vec![0; length as usize];
    reader.read_exact(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn skip_gguf_value(reader: &mut (impl Read + Seek), value_type: u32) -> std::io::Result<()> {
    let bytes = match value_type {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        8 => {
            let length = read_u64(reader)?;
            reader.seek(SeekFrom::Current(length as i64))?;
            return Ok(());
        }
        9 => {
            let element_type = read_u32(reader)?;
            let count = read_u64(reader)?;
            if count > 10_000_000 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "GGUF array is too large",
                ));
            }
            for _ in 0..count {
                skip_gguf_value(reader, element_type)?;
            }
            return Ok(());
        }
        _ => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Unknown GGUF metadata type",
            ));
        }
    };
    reader.seek(SeekFrom::Current(bytes))?;
    Ok(())
}

fn read_gguf_number(reader: &mut impl Read, value_type: u32) -> std::io::Result<Option<u64>> {
    match value_type {
        4 => Ok(Some(u64::from(read_u32(reader)?))),
        5 => Ok(Some(read_u32(reader)? as i32 as u64)),
        10 => read_u64(reader).map(Some),
        11 => read_u64(reader).map(|value| Some(value as i64 as u64)),
        _ => Ok(None),
    }
}

fn read_gguf_metadata(path: &Path) -> std::io::Result<GgufMetadata> {
    let mut reader = std::io::BufReader::new(File::open(path)?);
    let mut magic = [0; 4];
    reader.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Invalid GGUF header",
        ));
    }
    let _version = read_u32(&mut reader)?;
    let _tensor_count = read_u64(&mut reader)?;
    let metadata_count = read_u64(&mut reader)?;
    if metadata_count > 1_000_000 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "GGUF metadata table is too large",
        ));
    }
    let mut metadata = GgufMetadata::default();
    for _ in 0..metadata_count {
        let key = read_gguf_string(&mut reader)?;
        let value_type = read_u32(&mut reader)?;
        let normalized_key = key.to_ascii_lowercase();
        if normalized_key.ends_with(".context_length") {
            metadata.context_length = read_gguf_number(&mut reader, value_type)?;
        } else if normalized_key == "tokenizer.chat_template" && value_type == 8 {
            let template = read_gguf_string(&mut reader)?.to_ascii_lowercase();
            metadata.tool_calling = template.contains("tools") || template.contains("tool_calls");
        } else if normalized_key.contains("vision")
            || normalized_key.starts_with("clip.")
            || normalized_key.contains("image")
        {
            metadata.vision = true;
            skip_gguf_value(&mut reader, value_type)?;
        } else {
            skip_gguf_value(&mut reader, value_type)?;
        }
    }
    Ok(metadata)
}

fn find_number(value: &serde_json::Value, wanted: &str) -> Option<u64> {
    match value {
        serde_json::Value::Object(object) => object.iter().find_map(|(key, child)| {
            if key.eq_ignore_ascii_case(wanted) {
                child.as_u64()
            } else {
                find_number(child, wanted)
            }
        }),
        serde_json::Value::Array(values) => {
            values.iter().find_map(|child| find_number(child, wanted))
        }
        _ => None,
    }
}

pub fn stop_on_exit(app: &AppHandle) {
    if let Some(state) = app.try_state::<RuntimeState>() {
        let _ = stop_runtime(state.inner());
    }
}

#[cfg(test)]
mod tests {
    use super::{GitHubAsset, GitHubRelease, select_asset};

    #[test]
    fn picks_vulkan_windows_asset_without_cuda_runtime() {
        let release = GitHubRelease {
            tag_name: "b1".to_string(),
            assets: vec![
                GitHubAsset {
                    name: "llama-b1-bin-win-cudart-x64.zip".to_string(),
                    browser_download_url: String::new(),
                    size: 0,
                },
                GitHubAsset {
                    name: "llama-b1-bin-win-vulkan-x64.zip".to_string(),
                    browser_download_url: String::new(),
                    size: 0,
                },
            ],
        };
        assert_eq!(
            select_asset(&release, "vulkan").map(|asset| asset.name.as_str()),
            Some("llama-b1-bin-win-vulkan-x64.zip")
        );
    }

    #[test]
    fn picks_cpu_asset_for_cpu_profile() {
        let release = GitHubRelease {
            tag_name: "b1".to_string(),
            assets: vec![GitHubAsset {
                name: "llama-b1-bin-win-cpu-x64.zip".to_string(),
                browser_download_url: String::new(),
                size: 0,
            }],
        };
        assert_eq!(
            select_asset(&release, "cpu").map(|asset| asset.name.as_str()),
            Some("llama-b1-bin-win-cpu-x64.zip")
        );
    }
}
