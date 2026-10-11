use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Manager, State};
use tokio::io::AsyncWriteExt;
mod openhands;

struct Runtime(Mutex<Option<Child>>, AtomicU64);

#[derive(Serialize)]
struct Entry {
    name: String,
    path: String,
    is_dir: bool,
}

#[derive(Deserialize)]
struct StartModel {
    model_path: String,
    executable: String,
    context: Option<u32>,
    gpu_layers: Option<i32>,
}

#[tauri::command]
fn browse_directory() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn browse_gguf() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .add_filter("GGUF model", &["gguf"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn browse_executable() -> Result<Option<String>, String> {
    Ok(rfd::FileDialog::new()
        .add_filter("llama-server executable", &["exe"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn scan_models(path: String) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for item in std::fs::read_dir(path)
        .map_err(|e| e.to_string())?
        .flatten()
    {
        if item
            .path()
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("gguf"))
            .unwrap_or(false)
        {
            out.push(Entry {
                name: item.file_name().to_string_lossy().into_owned(),
                path: item.path().to_string_lossy().into_owned(),
                is_dir: false,
            });
        }
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}
#[tauri::command]
fn list_directory(path: String) -> Result<Vec<Entry>, String> {
    let path = PathBuf::from(path);
    let entries = std::fs::read_dir(path).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for item in entries.flatten() {
        let meta = item.metadata().map_err(|e| e.to_string())?;
        out.push(Entry {
            name: item.file_name().to_string_lossy().into_owned(),
            path: item.path().to_string_lossy().into_owned(),
            is_dir: meta.is_dir(),
        });
    }
    out.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

#[tauri::command]
fn read_text_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<(), String> {
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}

#[tauri::command]
fn start_local_model(config: StartModel, state: State<Runtime>) -> Result<String, String> {
    state.1.fetch_add(1, Ordering::SeqCst);
    state.1.fetch_add(1, Ordering::SeqCst);
    let mut active = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = active.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let mut cmd = Command::new(&config.executable);
    cmd.args([
        "--model",
        &config.model_path,
        "--host",
        "127.0.0.1",
        "--port",
        "8080",
    ])
    .arg("--ctx-size")
    .arg(config.context.unwrap_or(32768).to_string())
    .arg("--n-gpu-layers")
    .arg(config.gpu_layers.unwrap_or(-1).to_string())
    .args(["--alias", "local-model"])
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    let child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start llama.cpp at {}: {e}", config.executable))?;
    *active = Some(child);
    let generation = state.1.fetch_add(1, Ordering::SeqCst) + 1;
    Ok(generation.to_string())
}

#[tauri::command]
fn stop_local_model(state: State<Runtime>) -> Result<(), String> {
    state.1.fetch_add(1, Ordering::SeqCst);
    state.1.fetch_add(1, Ordering::SeqCst);
    let mut active = state.0.lock().map_err(|e| e.to_string())?;
    if let Some(mut child) = active.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    Ok(())
}

#[tauri::command]
async fn fetch_huggingface(
    query: String,
    token: Option<String>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let mut req = client.get("https://huggingface.co/api/models").query(&[
        ("search", query.as_str()),
        ("filter", "gguf"),
        ("sort", "downloads"),
        ("limit", "30"),
    ]);
    if let Some(token) = token.filter(|v| !v.is_empty()) {
        req = req.bearer_auth(token);
    }
    let response = req.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Hugging Face returned {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn fetch_model_files(
    repo: String,
    token: Option<String>,
) -> Result<serde_json::Value, String> {
    let client = reqwest::Client::new();
    let mut request = client.get(format!(
        "https://huggingface.co/api/models/{repo}/tree/main?recursive=true&expand=true"
    ));
    if let Some(token) = token.filter(|v| !v.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Hugging Face returned {}", response.status()));
    }
    let files: Vec<serde_json::Value> = response.json().await.map_err(|e| e.to_string())?;
    Ok(serde_json::Value::Array(
        files
            .into_iter()
            .filter(|f| {
                f.get("path")
                    .and_then(|v| v.as_str())
                    .map(|p| p.to_lowercase().ends_with(".gguf"))
                    .unwrap_or(false)
            })
            .collect(),
    ))
}
#[tauri::command]
async fn download_model(
    repo: String,
    file: String,
    destination: String,
    token: Option<String>,
) -> Result<String, String> {
    let target = PathBuf::from(destination).join(&file);
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| e.to_string())?;
    }
    let url = format!("https://huggingface.co/{repo}/resolve/main/{file}");
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(token) = token.filter(|v| !v.is_empty()) {
        request = request.bearer_auth(token);
    }
    let response = request.send().await.map_err(|e| e.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Download failed: {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut output = tokio::fs::File::create(&target)
        .await
        .map_err(|e| e.to_string())?;
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        output.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    output.flush().await.map_err(|e| e.to_string())?;
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
async fn wait_for_model(state: State<'_, Runtime>, generation: u64) -> Result<(), String> {
    for _ in 0..600 {
        if state.1.load(Ordering::SeqCst) != generation {
            return Err("Model start cancelled".into());
        }
        if reqwest::get("http://127.0.0.1:8080/health")
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Err("Model server did not become ready within 5 minutes".into())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(Runtime(Mutex::new(None), AtomicU64::new(0)))
        .manage(openhands::AgentServerState::default())
        .invoke_handler(tauri::generate_handler![
            list_directory,
            browse_directory,
            browse_gguf,
            browse_executable,
            scan_models,
            read_text_file,
            write_text_file,
            start_local_model,
            stop_local_model,
            fetch_huggingface,
            fetch_model_files,
            wait_for_model,
            download_model,
            openhands::start_agent_server,
            openhands::stop_agent_server,
            openhands::get_agent_server_status,
            openhands::send_agent_message,
            openhands::save_model_provider_key,
            openhands::remove_model_provider_key,
            openhands::model_provider_key_configured
        ])
        .setup(|_| Ok(()))
        .build(tauri::generate_context!())
        .expect("error while starting Flux Hands");
    application.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            openhands::stop_on_exit(app);
            if let Some(mut child) = app
                .state::<Runtime>()
                .0
                .lock()
                .ok()
                .and_then(|mut child| child.take())
            {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    });
}
