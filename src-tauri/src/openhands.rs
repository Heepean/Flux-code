use std::fs;
use std::io::{Cursor, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

const SERVER_VERSION: &str = "1.26.0";
const KILO_GATEWAY_BASE: &str = "https://api.kilo.ai/api/gateway";
const UV_RELEASE_API: &str = "https://api.github.com/repos/astral-sh/uv/releases/latest";
const UV_WINDOWS_ARCHIVE: &str = "uv-x86_64-pc-windows-msvc.zip";

#[derive(Default)]
pub struct AgentServerState {
    child: Mutex<Option<Child>>,
    port: Mutex<Option<u16>>,
    session_key: Mutex<Option<String>>,
    config: Mutex<Option<(String, String, String, String, String)>>,
    log: Arc<Mutex<String>>,
}

#[derive(Serialize)]
pub struct AgentServerStatus {
    pub running: bool,
    pub url: Option<String>,
    pub version: &'static str,
    pub log: String,
}

#[derive(Serialize, Clone)]
struct AgentEvent {
    delta: String,
    done: bool,
    conversation_id: Option<String>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct UvRelease {
    assets: Vec<UvAsset>,
}

#[derive(Deserialize)]
struct UvAsset {
    name: String,
    browser_download_url: String,
}

fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("openhands"))
        .map_err(|error| error.to_string())
}

fn hidden(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
}

fn installed_uvx(app: &AppHandle) -> Option<PathBuf> {
    let local = app
        .path()
        .app_data_dir()
        .ok()?
        .join("bin")
        .join(if cfg!(windows) { "uvx.exe" } else { "uvx" });
    if local.is_file() {
        return Some(local);
    }
    let path = if cfg!(windows) { "uvx.exe" } else { "uvx" };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|entry| entry.join(path))
        .find(|path| path.is_file())
}

async fn install_uvx(app: &AppHandle) -> Result<PathBuf, String> {
    if let Some(path) = installed_uvx(app) {
        return Ok(path);
    }
    let client = reqwest::Client::builder()
        .user_agent(concat!("Flux-Hands/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let release: UvRelease = client
        .get(UV_RELEASE_API)
        .send()
        .await
        .map_err(|error| format!("Не удалось проверить runtime uv: {error}"))?
        .error_for_status()
        .map_err(|error| format!("GitHub отклонил запрос uv: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Некорректный ответ GitHub для uv: {error}"))?;
    let asset = release
        .assets
        .into_iter()
        .find(|asset| asset.name == UV_WINDOWS_ARCHIVE)
        .ok_or_else(|| "В последнем релизе uv нет архива для Windows x64".to_string())?;
    let bytes = client
        .get(asset.browser_download_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось скачать runtime uv: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Скачивание uv завершилось ошибкой: {error}"))?
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("bin");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| error.to_string())?;
    for name in ["uv.exe", "uvx.exe"] {
        let entry_name = format!("uv-x86_64-pc-windows-msvc/{name}");
        let mut entry = archive
            .by_name(&entry_name)
            .map_err(|error| format!("Архив uv не содержит {name}: {error}"))?;
        let target = directory.join(name);
        let temporary = directory.join(format!("{name}.installing"));
        let mut output = fs::File::create(&temporary).map_err(|error| error.to_string())?;
        std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
        fs::rename(temporary, target).map_err(|error| error.to_string())?;
    }
    Ok(directory.join("uvx.exe"))
}

fn free_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|socket| socket.local_addr())
        .map(|address| address.port())
        .map_err(|error| error.to_string())
}

fn saved_session_key(app: &AppHandle) -> Result<String, String> {
    let directory = data_dir(app)?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let path = directory.join("session-api-key");
    if let Ok(key) = fs::read_to_string(&path) {
        let key = key.trim();
        if key.len() >= 32 {
            return Ok(key.to_owned());
        }
    }
    let key =
        Uuid::new_v4().to_string().replace('-', "") + &Uuid::new_v4().to_string().replace('-', "");
    fs::write(path, &key).map_err(|error| error.to_string())?;
    Ok(key)
}

fn status(state: &AgentServerState) -> AgentServerStatus {
    let running = state
        .child
        .lock()
        .ok()
        .and_then(|mut child| {
            child.as_mut().map(|child| match child.try_wait() {
                Ok(Some(_)) => false,
                Ok(None) => true,
                Err(_) => false,
            })
        })
        .unwrap_or(false);
    let port = state.port.lock().ok().and_then(|port| *port);
    let log = state.log.lock().map(|log| log.clone()).unwrap_or_default();
    AgentServerStatus {
        running,
        url: if running {
            port.map(|port| format!("http://127.0.0.1:{port}"))
        } else {
            None
        },
        version: SERVER_VERSION,
        log,
    }
}

fn remote_key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("Flux Hands Model Provider", "openai-compatible")
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_model_provider_key(api_key: String) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("Введите API-ключ или оставьте поле пустым для локального endpoint".into());
    }
    remote_key_entry()?
        .set_password(api_key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn remove_model_provider_key() -> Result<(), String> {
    match remote_key_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn model_provider_key_configured() -> Result<bool, String> {
    Ok(remote_key_entry()?.get_password().is_ok())
}

fn normalize_endpoint(endpoint: &str) -> Result<String, String> {
    let mut parsed = reqwest::Url::parse(endpoint.trim())
        .map_err(|_| "Введите корректный URL OpenAI-compatible endpoint".to_string())?;
    let host = parsed.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && loopback) {
        return Err(
            "Удалённый endpoint должен использовать HTTPS. HTTP разрешён только для localhost."
                .into(),
        );
    }
    let mut path = parsed.path().trim_end_matches('/').to_string();
    if path.ends_with("/chat/completions") {
        path.truncate(path.len() - "/chat/completions".len());
    }
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    parsed.set_path(&path);
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn kilo_model_is_free_text(model: &serde_json::Value) -> bool {
    let zero_price = |value: Option<&serde_json::Value>| {
        value.is_some_and(|value| match value {
            serde_json::Value::Number(number) => number.as_f64() == Some(0.0),
            serde_json::Value::String(price) => price.parse::<f64>().ok() == Some(0.0),
            _ => false,
        })
    };
    let is_free = model.get("isFree").and_then(serde_json::Value::as_bool) == Some(true)
        || (zero_price(model.pointer("/pricing/prompt"))
            && zero_price(model.pointer("/pricing/completion")));
    let supports_text = model
        .pointer("/architecture/input_modalities")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("text")))
        && model
            .pointer("/architecture/output_modalities")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some("text")));
    is_free && supports_text && model.get("id").is_some()
}

#[tauri::command]
pub async fn fetch_kilo_free_models() -> Result<Vec<serde_json::Value>, String> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("Flux-Hands/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!("{KILO_GATEWAY_BASE}/models"))
        .send()
        .await
        .map_err(|error| format!("Не удалось получить каталог Kilo: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Kilo Gateway отклонил запрос каталога: {error}"))?;
    let catalog: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("Kilo вернул некорректный каталог: {error}"))?;
    let models = catalog
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| catalog.as_array())
        .ok_or_else(|| "Формат каталога Kilo не содержит список моделей".to_string())?;
    let mut free_models = models
        .iter()
        .filter(|model| kilo_model_is_free_text(model))
        .map(|model| {
            serde_json::json!({
                "id": model.get("id"),
                "name": model.get("name").and_then(serde_json::Value::as_str).unwrap_or_else(|| model.get("id").and_then(serde_json::Value::as_str).unwrap_or("Kilo model")),
                "description": model.get("description").and_then(serde_json::Value::as_str).unwrap_or_default(),
                "contextLength": model.get("context_length").or_else(|| model.pointer("/top_provider/context_length")),
                "mayTrainOnYourPrompts": model.get("mayTrainOnYourPrompts").and_then(serde_json::Value::as_bool).unwrap_or(false),
                "supportsTools": model.get("supported_parameters").and_then(serde_json::Value::as_array).is_some_and(|items| items.iter().any(|item| item.as_str() == Some("tools")))
            })
        })
        .collect::<Vec<_>>();
    free_models.sort_by(|a, b| {
        a.get("name")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_lowercase()
            .cmp(
                &b.get("name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_lowercase(),
            )
    });
    Ok(free_models)
}

#[tauri::command]
pub fn get_agent_server_status(state: State<'_, AgentServerState>) -> AgentServerStatus {
    status(&state)
}

#[tauri::command]
pub async fn start_agent_server(
    app: AppHandle,
    state: State<'_, AgentServerState>,
    workspace: String,
    provider: String,
    model: String,
    endpoint: String,
) -> Result<AgentServerStatus, String> {
    let workspace = Path::new(&workspace)
        .canonicalize()
        .map_err(|error| format!("Не удалось открыть проект: {error}"))?;
    if !workspace.is_dir() {
        return Err("Путь проекта не является папкой".into());
    }
    let model = model.trim();
    if model.is_empty() {
        return Err("Сначала выберите модель".into());
    }
    let provider = provider.trim().to_ascii_lowercase();
    if provider != "local" && provider != "remote" && provider != "kilo" {
        return Err("Выберите локальную модель, Kilo Free или OpenAI-compatible endpoint".into());
    }
    let endpoint = if provider == "local" {
        "http://127.0.0.1:8080/v1".to_string()
    } else if provider == "kilo" {
        KILO_GATEWAY_BASE.to_string()
    } else {
        normalize_endpoint(&endpoint)?
    };
    let api_key = if provider == "local" {
        "local".to_string()
    } else if provider == "kilo" {
        // LiteLLM rejects an empty credential before it sends the anonymous
        // request. Kilo accepts an arbitrary bearer value for its free models.
        "anonymous".to_string()
    } else {
        remote_key_entry()
            .ok()
            .and_then(|entry| entry.get_password().ok())
            .unwrap_or_default()
    };
    let is_loopback = reqwest::Url::parse(&endpoint)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host == "localhost" || host == "127.0.0.1" || host == "::1")
        })
        .unwrap_or(false);
    if provider == "remote" && api_key.is_empty() && !is_loopback {
        return Err("Сохраните API-ключ в разделе моделей. Для локальных Ollama/LM Studio ключ можно не задавать.".into());
    }
    let api_key = if api_key.is_empty() {
        "local".to_string()
    } else {
        api_key
    };
    let model_id = if provider == "local" {
        "openai/local-model".to_string()
    } else {
        format!("openai/{model}")
    };
    let config = (
        workspace.to_string_lossy().into_owned(),
        provider.clone(),
        endpoint.clone(),
        model.to_owned(),
        api_key.clone(),
    );
    {
        let mut child = state.child.lock().map_err(|error| error.to_string())?;
        if let Some(current) = child.as_mut() {
            if current
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
            {
                if state
                    .config
                    .lock()
                    .map_err(|error| error.to_string())?
                    .as_ref()
                    == Some(&config)
                {
                    return Ok(status(&state));
                }
                let _ = current.kill();
                let _ = current.wait();
                *child = None;
            }
        }
    }
    let executable = install_uvx(&app).await?;
    let port = free_port()?;
    let vscode_port = free_port()?;
    let session_key = saved_session_key(&app)?;
    let directory = data_dir(&app)?;
    let mut command = Command::new(executable);
    command
        .args([
            "--from",
            "openhands-agent-server==1.26.0",
            "--with",
            "openhands-sdk==1.26.0",
            "--with",
            "openhands-tools==1.26.0",
            "--with",
            "openhands-workspace==1.26.0",
            "--with",
            "posthog>=6,<7",
            "agent-server",
            "--host",
            "127.0.0.1",
            "--port",
        ])
        .arg(port.to_string())
        .current_dir(&workspace)
        .env("OH_CONVERSATION_RUNTIME", "local")
        .env("OH_SESSION_API_KEYS_0", &session_key)
        .env("OH_SECRET_KEY", &session_key)
        .env("OH_PERSISTENCE_DIR", directory.join("persistence"))
        .env("OH_CONVERSATIONS_PATH", directory.join("conversations"))
        .env("OH_BASH_EVENTS_DIR", directory.join("bash-events"))
        .env("OH_VSCODE_PORT", vscode_port.to_string())
        .env("OH_WORKING_DIR", &workspace)
        .env("LLM_MODEL", &model_id)
        .env("LLM_API_KEY", &api_key)
        .env("LLM_BASE_URL", &endpoint)
        .env("PYTHONUTF8", "1")
        .env("DO_NOT_TRACK", "1")
        .env("VITE_DO_NOT_TRACK", "1")
        .env("OH_LOG_LEVEL", "INFO")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hidden(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| format!("Не удалось запустить OpenHands Agent Server: {error}"))?;
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    *state.log.lock().map_err(|error| error.to_string())? = String::new();
    for mut stream in [
        stdout.map(|stream| Box::new(stream) as Box<dyn Read + Send>),
        stderr.map(|stream| Box::new(stream) as Box<dyn Read + Send>),
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
                    if log.len() > 32_000 {
                        let excess = log.len() - 32_000;
                        log.drain(..excess);
                    }
                }
            }
        });
    }
    *state.child.lock().map_err(|error| error.to_string())? = Some(child);
    *state.port.lock().map_err(|error| error.to_string())? = Some(port);
    *state
        .session_key
        .lock()
        .map_err(|error| error.to_string())? = Some(session_key.clone());
    let client = reqwest::Client::new();
    for _ in 0..600 {
        let child_exited = {
            let result = if let Ok(mut slot) = state.child.lock() {
                slot.as_mut()
                    .and_then(|child| child.try_wait().ok().flatten())
            } else {
                None
            };
            result
        };
        if let Some(exit) = child_exited {
            return Err(format!(
                "OpenHands Agent Server завершился с кодом {exit}. {}",
                state.log.lock().map(|log| log.clone()).unwrap_or_default()
            ));
        }
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{port}/health"))
            .header("X-Session-API-Key", &session_key)
            .send()
            .await
        {
            if response.status().is_success() {
                *state.config.lock().map_err(|error| error.to_string())? = Some(config.clone());
                return Ok(status(&state));
            }
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    Err(format!(
        "OpenHands Agent Server не запустился за 10 минут. {}",
        state.log.lock().map(|log| log.clone()).unwrap_or_default()
    ))
}

#[tauri::command]
pub fn stop_agent_server(state: State<'_, AgentServerState>) -> Result<(), String> {
    if let Some(mut child) = state
        .child
        .lock()
        .map_err(|error| error.to_string())?
        .take()
    {
        child.kill().map_err(|error| error.to_string())?;
        let _ = child.wait();
    }
    *state.port.lock().map_err(|error| error.to_string())? = None;
    *state
        .session_key
        .lock()
        .map_err(|error| error.to_string())? = None;
    Ok(())
}

#[tauri::command]
pub async fn send_agent_message(
    app: AppHandle,
    state: State<'_, AgentServerState>,
    message: String,
    conversation_id: Option<String>,
) -> Result<String, String> {
    let (port, key, config) = {
        let port = state
            .port
            .lock()
            .map_err(|error| error.to_string())?
            .ok_or("OpenHands Agent Server is not running")?;
        let key = state
            .session_key
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or("OpenHands session key is missing")?;
        let config = state
            .config
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or("OpenHands model configuration is missing")?;
        (port, key, config)
    };
    let (workspace, provider, endpoint, model, api_key) = config;
    let model_id = if provider == "local" {
        "openai/local-model".to_string()
    } else {
        format!("openai/{model}")
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let base = format!("http://127.0.0.1:{port}/api/conversations");
    let headers = [("X-Session-API-Key", key)];
    let mut conversation_id = conversation_id;
    if let Some(id) = conversation_id.as_deref() {
        let conversation = client
            .get(format!("{base}/{id}"))
            .header(headers[0].0, &headers[0].1)
            .send()
            .await
            .map_err(|error| format!("Не удалось открыть разговор OpenHands: {error}"))?;
        if conversation.status().as_u16() == 404 {
            conversation_id = None;
        } else if !conversation.status().is_success() {
            let details = conversation.text().await.unwrap_or_default();
            return Err(format!("OpenHands не открыл разговор: {details}"));
        } else {
            let switched = client
                .post(format!("{base}/{id}/switch_llm"))
                .header(headers[0].0, &headers[0].1)
                .json(&serde_json::json!({
                    "llm": {
                        "model": model_id,
                        "api_key": api_key,
                        "base_url": endpoint,
                        "reasoning_effort": "high"
                    }
                }))
                .send()
                .await
                .map_err(|error| format!("Не удалось переключить модель разговора: {error}"))?;
            ensure_success(switched, "OpenHands не переключил модель разговора").await?;
        }
    }
    let (id, mut seen) = if let Some(id) = conversation_id {
        let existing = client
            .get(format!(
                "{base}/{id}/events/search?limit=1&sort_order=TIMESTAMP_DESC"
            ))
            .header(headers[0].0, &headers[0].1)
            .send()
            .await
            .map_err(|error| format!("Не удалось проверить историю OpenHands: {error}"))?;
        let existing = ensure_success_json(existing, "OpenHands не вернул историю").await?;
        let last = existing
            .pointer("/items/0/id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        let response = client
            .post(format!("{base}/{id}/events"))
            .header(headers[0].0, &headers[0].1)
            .json(&serde_json::json!({
                "role": "user",
                "content": [{"type":"text", "text":message}],
                "run": true
            }))
            .send()
            .await
            .map_err(|error| format!("Не удалось добавить задачу в OpenHands: {error}"))?;
        ensure_success(response, "OpenHands не принял задачу").await?;
        (
            id,
            last.into_iter().collect::<std::collections::HashSet<_>>(),
        )
    } else {
        let response = client
            .post(&base)
            .header(headers[0].0, &headers[0].1)
            .json(&serde_json::json!({
                "workspace": {"working_dir": workspace, "kind": "LocalWorkspace"},
                "agent": {
                    "kind": "Agent",
                    "llm": {"model": model_id, "api_key": api_key, "base_url": endpoint, "reasoning_effort": "high"},
                    "tools": [
                        {"name":"terminal","params":{}},
                        {"name":"file_editor","params":{}},
                        {"name":"task_tracker","params":{}},
                        {"name":"task_tool_set","params":{}}
                    ]
                },
                "confirmation_policy": {"kind":"NeverConfirm"},
                "initial_message": {"role":"user", "content":[{"type":"text","text":message}], "run":true},
                "autotitle": false
            }))
            .send()
            .await
            .map_err(|error| format!("Не удалось создать разговор OpenHands: {error}"))?;
        let value = ensure_success_json(response, "OpenHands не создал разговор").await?;
        let id = value
            .get("id")
            .and_then(serde_json::Value::as_str)
            .ok_or("OpenHands не вернул ID разговора")?
            .to_owned();
        let last_user_message = value
            .get("last_user_message_id")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        (
            id,
            last_user_message
                .into_iter()
                .collect::<std::collections::HashSet<_>>(),
        )
    };
    let _ = app.emit(
        "openhands-agent-event",
        AgentEvent {
            delta: String::new(),
            done: false,
            conversation_id: Some(id.clone()),
            error: None,
        },
    );
    let mut answer = String::new();
    let mut final_status = String::new();
    let mut assistant_messages = std::collections::HashSet::new();
    for _ in 0..600 {
        let mut new_events = Vec::new();
        let mut page_id = None;
        loop {
            let mut url = format!("{base}/{id}/events/search?limit=100&sort_order=TIMESTAMP_DESC");
            if let Some(page_id) = page_id.as_deref() {
                url.push_str("&page_id=");
                url.push_str(page_id);
            }
            let events = client
                .get(url)
                .header(headers[0].0, &headers[0].1)
                .send()
                .await
                .map_err(|error| format!("Не удалось получить события OpenHands: {error}"))?;
            let events = ensure_success_json(events, "OpenHands не вернул события").await?;
            let items = events
                .get("items")
                .and_then(serde_json::Value::as_array)
                .ok_or("Формат событий OpenHands неизвестен")?;
            let mut reached_seen_event = false;
            for event in items {
                let event_id = event
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if event_id.is_empty() || seen.contains(event_id) {
                    reached_seen_event = true;
                    break;
                }
                new_events.push(event.clone());
            }
            if reached_seen_event {
                break;
            }
            page_id = events
                .get("next_page_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            if page_id.is_none() || items.is_empty() {
                break;
            }
        }
        new_events.reverse();
        for event in &new_events {
            let event_id = event
                .get("id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if event_id.is_empty() || !seen.insert(event_id.to_owned()) {
                continue;
            }
            let kind = event
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let source = event
                .get("source")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if kind == "MessageEvent"
                && source == "agent"
                && !assistant_messages.insert(event_id.to_owned())
            {
                continue;
            }
            let text = if kind == "MessageEvent" && source == "agent" {
                event
                    .pointer("/llm_message/content")
                    .and_then(serde_json::Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
            } else if kind == "StreamingDeltaEvent" && source == "agent" {
                event
                    .get("content")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            } else {
                None
            };
            if let Some(text) = text.filter(|text| !text.is_empty()) {
                if !answer.ends_with(&text) {
                    answer.push_str(&text);
                    let _ = app.emit(
                        "openhands-agent-event",
                        AgentEvent {
                            delta: text,
                            done: false,
                            conversation_id: Some(id.clone()),
                            error: None,
                        },
                    );
                }
            }
            if kind == "ActionEvent" {
                let name = event
                    .get("tool_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool");
                let activity = format!("\n\n[OpenHands] Запускает инструмент: {name}\n");
                answer.push_str(&activity);
                let _ = app.emit(
                    "openhands-agent-event",
                    AgentEvent {
                        delta: activity,
                        done: false,
                        conversation_id: Some(id.clone()),
                        error: None,
                    },
                );
            }
            if kind == "ObservationEvent" {
                let output = event
                    .pointer("/observation/content")
                    .and_then(serde_json::Value::as_array)
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                if !output.is_empty() {
                    let activity = format!("\n{output}\n");
                    answer.push_str(&activity);
                    let _ = app.emit(
                        "openhands-agent-event",
                        AgentEvent {
                            delta: activity,
                            done: false,
                            conversation_id: Some(id.clone()),
                            error: None,
                        },
                    );
                }
            }
            if kind == "ConversationStateUpdateEvent" {
                let key = event.get("key").and_then(serde_json::Value::as_str);
                if key == Some("execution_status") {
                    final_status = event
                        .get("value")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                } else if key == Some("full_state") {
                    final_status = event
                        .pointer("/value/execution_status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                }
            }
            if kind == "ConversationErrorEvent"
                || kind == "AgentErrorEvent"
                || kind == "ServerErrorEvent"
            {
                let details = event
                    .get("error")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| event.get("message").and_then(serde_json::Value::as_str))
                    .or_else(|| event.get("detail").and_then(serde_json::Value::as_str))
                    .unwrap_or("OpenHands agent error");
                return Err(details.to_owned());
            }
        }
        if ["idle", "finished", "paused", "error"].contains(&final_status.as_str()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if final_status.is_empty() || final_status == "running" {
        return Err("Превышено время ожидания выполнения задачи OpenHands".into());
    }
    let _ = app.emit(
        "openhands-agent-event",
        AgentEvent {
            delta: String::new(),
            done: true,
            conversation_id: Some(id),
            error: None,
        },
    );
    if answer.trim().is_empty() {
        return Err(format!(
            "OpenHands завершил задачу со статусом {final_status} без текстового ответа"
        ));
    }
    Ok(answer)
}

async fn ensure_success(response: reqwest::Response, context: &str) -> Result<(), String> {
    if response.status().is_success() {
        return Ok(());
    }
    Err(format!(
        "{context}: {}",
        response.text().await.unwrap_or_default()
    ))
}

async fn ensure_success_json(
    response: reqwest::Response,
    context: &str,
) -> Result<serde_json::Value, String> {
    if response.status().is_success() {
        return response
            .json()
            .await
            .map_err(|error| format!("{context}: {error}"));
    }
    Err(format!(
        "{context}: {}",
        response.text().await.unwrap_or_default()
    ))
}

#[cfg(test)]
mod tests {
    use super::kilo_model_is_free_text;

    #[test]
    fn kilo_catalog_keeps_free_text_models_including_chat_only_models() {
        let eligible = serde_json::json!({
            "id": "provider/model:free",
            "isFree": true,
            "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
            "supported_parameters": ["tools"]
        });
        assert!(kilo_model_is_free_text(&eligible));

        let paid = serde_json::json!({
            "id": "provider/paid",
            "pricing": {"prompt": "0.0001", "completion": "0.0002"},
            "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
            "supported_parameters": ["tools"]
        });
        assert!(!kilo_model_is_free_text(&paid));

        let no_tools = serde_json::json!({
            "id": "provider/no-tools:free",
            "isFree": true,
            "architecture": {"input_modalities": ["text"], "output_modalities": ["text"]},
            "supported_parameters": ["temperature"]
        });
        assert!(kilo_model_is_free_text(&no_tools));
    }
}

pub fn stop_on_exit(app: &AppHandle) {
    let state = app.state::<AgentServerState>();
    if let Ok(mut child) = state.child.lock() {
        if let Some(mut child) = child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    };
}
