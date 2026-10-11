use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager, State};
use tokio::io::AsyncWriteExt;
mod openhands;

struct Runtime {
    child: Mutex<Option<Child>>,
    generation: AtomicU64,
    log: Arc<Mutex<String>>,
    llama_install_lock: tokio::sync::Mutex<()>,
}

#[derive(Deserialize)]
struct LlamaRelease {
    tag_name: String,
    assets: Vec<LlamaAsset>,
}

#[derive(Deserialize)]
struct LlamaAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

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

#[cfg(windows)]
fn available_physical_memory() -> Result<u64, String> {
    use std::mem::size_of;
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut memory = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut memory) } == 0 {
        return Err("Windows не смог определить доступную оперативную память".into());
    }
    Ok(memory.ullAvailPhys)
}

#[tauri::command]
fn browse_directory() -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    return Err(
        "Выбор папки доступен на desktop; на Android укажите workspace на удалённом Agent Server"
            .into(),
    );
    #[cfg(not(target_os = "android"))]
    Ok(rfd::FileDialog::new()
        .pick_folder()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn browse_gguf() -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    return Err("Локальные GGUF-модели доступны только в desktop-сборке".into());
    #[cfg(not(target_os = "android"))]
    Ok(rfd::FileDialog::new()
        .add_filter("GGUF model", &["gguf"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn browse_executable() -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    return Err("llama.cpp доступен только в desktop-сборке".into());
    #[cfg(not(target_os = "android"))]
    Ok(rfd::FileDialog::new()
        .add_filter("llama-server executable", &["exe"])
        .pick_file()
        .map(|p| p.to_string_lossy().into_owned()))
}

#[tauri::command]
fn default_workspace(app: AppHandle) -> Result<String, String> {
    let path = app
        .path()
        .document_dir()
        .map_err(|error| error.to_string())?
        .join("Flux Hands Workspace");
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
fn path_is_file(path: String) -> bool {
    Path::new(&path).is_file()
}

#[tauri::command]
async fn install_llama_runtime(
    app: AppHandle,
    state: State<'_, Runtime>,
) -> Result<String, String> {
    let _install_guard = state.llama_install_lock.lock().await;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?;
    let runtime_root = app_data.join("runtimes").join("llama.cpp");
    let executable = runtime_root.join("active").join("llama-server.exe");
    if executable.is_file() {
        return Ok(executable.to_string_lossy().into_owned());
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("Flux-Hands/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|error| error.to_string())?;
    let releases: Vec<LlamaRelease> = client
        .get("https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=30")
        .send()
        .await
        .map_err(|error| format!("Не удалось проверить релизы llama.cpp: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub отклонил запрос релизов llama.cpp: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Некорректный список релизов llama.cpp: {error}"))?;
    let (release_tag, asset) = releases
        .into_iter()
        .find_map(|release| {
            release
                .assets
                .into_iter()
                .find(|asset| {
                    asset.name.starts_with("llama-") && asset.name.ends_with("-bin-win-cpu-x64.zip")
                })
                .map(|asset| (release.tag_name, asset))
        })
        .ok_or_else(|| "В последних релизах llama.cpp нет сборки Windows x64 CPU".to_string())?;
    let expected_digest = asset
        .digest
        .as_deref()
        .and_then(|digest| digest.strip_prefix("sha256:"))
        .ok_or_else(|| {
            "GitHub не предоставил SHA-256 для архива llama.cpp; загрузка отменена".to_string()
        })?;
    let response = client
        .get(asset.browser_download_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось скачать llama.cpp: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Скачивание llama.cpp завершилось ошибкой: {error}"))?;
    if response.content_length().unwrap_or_default() > 512 * 1024 * 1024 {
        return Err("Архив llama.cpp превышает безопасный предел 512 MB".into());
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|error| format!("Не удалось прочитать архив llama.cpp: {error}"))?;
    if bytes.len() < 1_000_000 || bytes.len() > 512 * 1024 * 1024 {
        return Err("Архив llama.cpp имеет недопустимый размер; загрузка отменена".into());
    }
    let actual_digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    if !actual_digest.eq_ignore_ascii_case(expected_digest) {
        return Err("SHA-256 архива llama.cpp не совпадает с контрольной суммой GitHub".into());
    }

    fs::create_dir_all(&runtime_root).map_err(|error| error.to_string())?;
    let staging = runtime_root.join(format!("installing-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&staging).map_err(|error| error.to_string())?;
    let result = (|| -> Result<PathBuf, String> {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|error| format!("Не удалось открыть архив llama.cpp: {error}"))?;
        let mut unpacked_bytes = 0u64;
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
            unpacked_bytes = unpacked_bytes.saturating_add(entry.size());
            if unpacked_bytes > 1024 * 1024 * 1024 {
                return Err("Распакованный llama.cpp runtime превышает предел 1 GB".into());
            }
            let relative = entry
                .enclosed_name()
                .ok_or_else(|| "Архив llama.cpp содержит недопустимый путь".to_string())?
                .to_path_buf();
            if entry
                .unix_mode()
                .map(|mode| mode & 0o170000 == 0o120000)
                .unwrap_or(false)
            {
                return Err("Архив llama.cpp содержит недопустимую символическую ссылку".into());
            }
            let target = staging.join(relative);
            if entry.is_dir() {
                fs::create_dir_all(&target).map_err(|error| error.to_string())?;
            } else {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
                }
                let mut output = fs::File::create(&target).map_err(|error| error.to_string())?;
                std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
            }
        }
        let staged_server = staging.join("llama-server.exe");
        if !staged_server.is_file() {
            return Err("Официальный архив не содержит llama-server.exe".into());
        }
        fs::write(staging.join("VERSION"), &release_tag).map_err(|error| error.to_string())?;
        let active = runtime_root.join("active");
        if active.exists() {
            fs::remove_dir_all(&active).map_err(|error| {
                format!("Не удалось заменить повреждённую llama.cpp сборку: {error}")
            })?;
        }
        fs::rename(&staging, &active).map_err(|error| error.to_string())?;
        Ok(active.join("llama-server.exe"))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|path| path.to_string_lossy().into_owned())
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
    if !Path::new(&config.model_path).is_file() {
        return Err(format!("Файл модели не найден: {}", config.model_path));
    }
    if !Path::new(&config.executable).is_file() {
        return Err(format!("llama-server.exe не найден: {}", config.executable));
    }
    if !config.model_path.to_ascii_lowercase().ends_with(".gguf") {
        return Err("Поддерживаются только файлы модели GGUF".into());
    }
    #[cfg(windows)]
    {
        let model_bytes = fs::metadata(&config.model_path)
            .map_err(|error| format!("Не удалось проверить размер модели: {error}"))?
            .len();
        let required_bytes = model_bytes.saturating_add(2 * 1024 * 1024 * 1024);
        let available_bytes = available_physical_memory()?;
        if available_bytes < required_bytes {
            return Err(format!(
                "Недостаточно свободной RAM для безопасного запуска модели. Модель занимает около {:.1} ГБ; требуется примерно {:.1} ГБ свободной RAM, сейчас доступно {:.1} ГБ. Закройте другие программы или выберите меньшую модель.",
                model_bytes as f64 / 1024f64.powi(3),
                required_bytes as f64 / 1024f64.powi(3),
                available_bytes as f64 / 1024f64.powi(3)
            ));
        }
    }
    let context = config.context.unwrap_or(4096);
    if !(512..=131072).contains(&context) {
        return Err("Размер контекста должен быть от 512 до 131072 токенов".into());
    }
    let gpu_layers = config.gpu_layers.unwrap_or(0);
    if !(-1..=999).contains(&gpu_layers) {
        return Err("Число GPU-слоёв должно быть от 0 до 999 или -1 для всех слоёв".into());
    }
    let generation = state.generation.fetch_add(1, Ordering::SeqCst) + 1;
    let mut active = state.child.lock().map_err(|e| e.to_string())?;
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
    .arg(context.to_string())
    .arg("--n-gpu-layers")
    .arg(gpu_layers.to_string())
    .args(["--alias", "local-model"])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start llama.cpp at {}: {e}", config.executable))?;
    *state.log.lock().map_err(|e| e.to_string())? = String::new();
    for mut stream in [
        child
            .stdout
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        child
            .stderr
            .take()
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let log = state.log.clone();
        std::thread::spawn(move || {
            let mut bytes = [0u8; 2048];
            while let Ok(count) = stream.read(&mut bytes) {
                if count == 0 {
                    break;
                }
                if let Ok(mut log) = log.lock() {
                    log.push_str(&String::from_utf8_lossy(&bytes[..count]));
                    if log.len() > 24_000 {
                        let excess = log.len() - 24_000;
                        log.drain(..excess);
                    }
                }
            }
        });
    }
    *active = Some(child);
    Ok(generation.to_string())
}

#[tauri::command]
fn stop_local_model(state: State<Runtime>) -> Result<(), String> {
    state.generation.fetch_add(1, Ordering::SeqCst);
    let mut active = state.child.lock().map_err(|e| e.to_string())?;
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
    if !valid_hf_repo(&repo) {
        return Err("Введите корректный Hugging Face repository ID".into());
    }
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

fn valid_hf_repo(repo: &str) -> bool {
    let parts: Vec<_> = repo.split('/').collect();
    parts.len() == 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
        })
}

#[tauri::command]
async fn download_model(
    repo: String,
    file: String,
    destination: String,
    token: Option<String>,
) -> Result<String, String> {
    if !valid_hf_repo(&repo) {
        return Err("Введите корректный Hugging Face repository ID".into());
    }
    let relative_file = PathBuf::from(&file);
    if !file.to_ascii_lowercase().ends_with(".gguf")
        || file.is_empty()
        || relative_file
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Путь GGUF внутри Hugging Face repository недопустим".into());
    }
    let destination = PathBuf::from(destination);
    tokio::fs::create_dir_all(&destination)
        .await
        .map_err(|e| format!("Не удалось подготовить папку моделей: {e}"))?;
    let target = destination.join(relative_file);
    if tokio::fs::try_exists(&target)
        .await
        .map_err(|error| error.to_string())?
    {
        return Err(format!(
            "Файл уже существует, перезапись отменена: {}",
            target.display()
        ));
    }
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
    let partial = target.with_file_name(format!(
        ".{}.partial-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("model.gguf"),
        uuid::Uuid::new_v4()
    ));
    let mut output = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial)
        .await
        .map_err(|e| format!("Не удалось создать временный файл модели: {e}"))?;
    use futures_util::StreamExt;
    let download_result = async {
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Загрузка GGUF прервана: {e}"))?;
            output
                .write_all(&chunk)
                .await
                .map_err(|e| format!("Не удалось записать GGUF: {e}"))?;
        }
        output
            .flush()
            .await
            .map_err(|e| format!("Не удалось завершить запись GGUF: {e}"))
    }
    .await;
    drop(output);
    if let Err(error) = download_result {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&partial, &target).await {
        let _ = tokio::fs::remove_file(&partial).await;
        return Err(format!("Не удалось сохранить скачанную модель: {error}"));
    }
    Ok(target.to_string_lossy().to_string())
}

#[tauri::command]
async fn wait_for_model(state: State<'_, Runtime>, generation: u64) -> Result<(), String> {
    for _ in 0..600 {
        if state.generation.load(Ordering::SeqCst) != generation {
            return Err("Model start cancelled".into());
        }
        let exit = state
            .child
            .lock()
            .map_err(|error| error.to_string())?
            .as_mut()
            .map(Child::try_wait)
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten();
        if let Some(exit) = exit {
            let log = state.log.lock().map_err(|error| error.to_string())?;
            return Err(format!(
                "llama-server завершился {exit}. Вывод сервера:\n{log}"
            ));
        }
        if let Ok(response) = reqwest::get("http://127.0.0.1:8080/health").await {
            if response.status().is_success() {
                // A previous llama-server may briefly keep responding while a new
                // generation is starting. Confirm the generation is still current
                // after the network await before reporting readiness.
                if state.generation.load(Ordering::SeqCst) != generation {
                    return Err("Model start cancelled".into());
                }
                let child_is_running = state
                    .child
                    .lock()
                    .map_err(|error| error.to_string())?
                    .as_mut()
                    .map(Child::try_wait)
                    .transpose()
                    .map_err(|error| error.to_string())?
                    .is_some_and(|status| status.is_none());
                if child_is_running {
                    return Ok(());
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let log = state.log.lock().map_err(|error| error.to_string())?;
    Err(format!(
        "llama-server не запустился за 5 минут. Вывод сервера:\n{log}"
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(not(target_os = "android"))]
    let builder = builder
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init());
    let application = builder
        .manage(Runtime {
            child: Mutex::new(None),
            generation: AtomicU64::new(0),
            log: Arc::new(Mutex::new(String::new())),
            llama_install_lock: tokio::sync::Mutex::new(()),
        })
        .manage(openhands::AgentServerState::default())
        .invoke_handler(tauri::generate_handler![
            list_directory,
            browse_directory,
            browse_gguf,
            browse_executable,
            default_workspace,
            path_is_file,
            install_llama_runtime,
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
            openhands::connect_remote_agent_server,
            openhands::stop_agent_server,
            openhands::get_agent_server_status,
            openhands::send_agent_message,
            openhands::save_model_provider_key,
            openhands::remove_model_provider_key,
            openhands::model_provider_key_configured,
            openhands::save_remote_agent_key,
            openhands::get_remote_agent_key,
            openhands::remove_remote_agent_key,
            openhands::fetch_kilo_free_models
        ])
        .setup(|_| Ok(()))
        .build(tauri::generate_context!())
        .expect("error while starting Flux Hands");
    application.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            openhands::stop_on_exit(app);
            if let Some(mut child) = app
                .state::<Runtime>()
                .child
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
