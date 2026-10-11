use std::fs;
use std::io::{Cursor, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use uuid::Uuid;

const SERVER_VERSION: &str = "1.26.0";
const UV_RELEASE_API: &str = "https://api.github.com/repos/astral-sh/uv/releases/latest";
const UV_WINDOWS_ARCHIVE: &str = "uv-x86_64-pc-windows-msvc.zip";

#[derive(Default)]
pub struct AgentServerState {
    child: Mutex<Option<Child>>,
    port: Mutex<Option<u16>>,
    session_key: Mutex<Option<String>>,
    config: Mutex<Option<(String, String, String, String)>>,
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

fn take_sse_lines(pending: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    pending.extend_from_slice(chunk);
    let mut lines = Vec::new();
    while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
        lines.push(
            String::from_utf8_lossy(&pending.drain(..=end).collect::<Vec<_>>())
                .trim()
                .to_string(),
        );
    }
    lines
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
    if provider != "local" && provider != "remote" {
        return Err("Выберите локальную модель или OpenAI-compatible endpoint".into());
    }
    let endpoint = if provider == "local" {
        "http://127.0.0.1:8080/v1".to_string()
    } else {
        normalize_endpoint(&endpoint)?
    };
    let api_key = if provider == "local" {
        "local".to_string()
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
                let profile = serde_json::json!({
                    "llm": {
                        "model": &model_id,
                        "api_key": &api_key,
                        "base_url": &endpoint
                    },
                    "include_secrets": true
                });
                let profile_url = format!("http://127.0.0.1:{port}/api/profiles/flux_hands");
                let _ = client
                    .delete(&profile_url)
                    .header("X-Session-API-Key", &session_key)
                    .send()
                    .await;
                let response = client
                    .post(profile_url)
                    .header("X-Session-API-Key", &session_key)
                    .json(&profile)
                    .send()
                    .await
                    .map_err(|error| format!("Не удалось настроить модель OpenHands: {error}"))?;
                if !response.status().is_success() && response.status().as_u16() != 409 {
                    let details = response.text().await.unwrap_or_default();
                    return Err(format!(
                        "OpenHands не принял конфигурацию локальной модели: {details}"
                    ));
                }
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
    let (port, key) = {
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
        (port, key)
    };
    let url = format!("http://127.0.0.1:{port}/v1/chat/completions");
    let mut request = reqwest::Client::new()
        .post(url)
        .bearer_auth(&key)
        .json(&serde_json::json!({
            "model": "openhands_flux_hands",
            "stream": true,
            "messages": [{"role":"user", "content":message}]
        }));
    if let Some(id) = conversation_id.as_deref() {
        request = request.header("X-OpenHands-ServerConversation-ID", id);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Не удалось отправить задачу OpenHands: {error}"))?;
    if !response.status().is_success() {
        let details = response.text().await.unwrap_or_default();
        let _ = app.emit(
            "openhands-agent-event",
            AgentEvent {
                delta: String::new(),
                done: true,
                conversation_id: None,
                error: Some(details.clone()),
            },
        );
        return Err(format!("OpenHands Agent Server отклонил задачу: {details}"));
    }
    let conversation_id = response
        .headers()
        .get("X-OpenHands-ServerConversation-ID")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
        .or(conversation_id);
    let mut stream = response.bytes_stream();
    // Keep raw bytes until a complete SSE line is available. Converting each
    // network chunk separately can corrupt UTF-8 characters split between
    // chunks (especially Russian text).
    let mut pending = Vec::new();
    let mut answer = String::new();
    let mut announced_tool_calls = std::collections::HashSet::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| format!("Поток OpenHands прерван: {error}"))?;
        for line in take_sse_lines(&mut pending, &chunk) {
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                continue;
            }
            let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else {
                continue;
            };
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(|value| value.as_str())
            {
                if !delta.is_empty() {
                    answer.push_str(delta);
                    let _ = app.emit(
                        "openhands-agent-event",
                        AgentEvent {
                            delta: delta.to_owned(),
                            done: false,
                            conversation_id: conversation_id.clone(),
                            error: None,
                        },
                    );
                }
            }
            // OpenHands can spend an entire response invoking tools without
            // producing ordinary assistant text. Surface those calls in the
            // same live stream so the UI does not look frozen.
            if let Some(tool_calls) = value
                .pointer("/choices/0/delta/tool_calls")
                .and_then(serde_json::Value::as_array)
            {
                for tool_call in tool_calls {
                    let index = tool_call.get("index").and_then(serde_json::Value::as_u64);
                    let name = tool_call
                        .pointer("/function/name")
                        .and_then(serde_json::Value::as_str);
                    if let (Some(index), Some(name)) = (index, name) {
                        if announced_tool_calls.insert(index) {
                            let activity = format!("\n\n[OpenHands] Запускает инструмент: {name}");
                            answer.push_str(&activity);
                            let _ = app.emit(
                                "openhands-agent-event",
                                AgentEvent {
                                    delta: activity,
                                    done: false,
                                    conversation_id: conversation_id.clone(),
                                    error: None,
                                },
                            );
                        }
                    }
                }
            }
        }
    }
    if !pending.is_empty() {
        let line = String::from_utf8_lossy(&pending).trim().to_string();
        if let Some(data) = line.strip_prefix("data:").map(str::trim) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(data) {
                if let Some(delta) = value
                    .pointer("/choices/0/delta/content")
                    .and_then(serde_json::Value::as_str)
                {
                    answer.push_str(delta);
                    let _ = app.emit(
                        "openhands-agent-event",
                        AgentEvent {
                            delta: delta.to_owned(),
                            done: false,
                            conversation_id: conversation_id.clone(),
                            error: None,
                        },
                    );
                }
            }
        }
    }
    if answer.trim().is_empty() && !announced_tool_calls.is_empty() {
        answer = "OpenHands выполнил запрос через инструменты без отдельного текстового ответа. Проверьте изменения в проекте.".into();
    }
    let _ = app.emit(
        "openhands-agent-event",
        AgentEvent {
            delta: String::new(),
            done: true,
            conversation_id,
            error: None,
        },
    );
    if answer.trim().is_empty() {
        return Err("OpenHands завершил задачу без текстового ответа".into());
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::take_sse_lines;

    #[test]
    fn sse_parser_preserves_utf8_split_between_network_chunks() {
        let bytes = "data: {\"choices\":[{\"delta\":{\"content\":\"Привет\"}}]}\n".as_bytes();
        let split = bytes
            .iter()
            .enumerate()
            .find_map(|(index, byte)| (*byte & 0b1100_0000 == 0b1000_0000).then_some(index))
            .expect("Russian text contains a UTF-8 continuation byte");
        let mut pending = Vec::new();
        assert!(take_sse_lines(&mut pending, &bytes[..split]).is_empty());
        let lines = take_sse_lines(&mut pending, &bytes[split..]);
        assert_eq!(
            lines,
            ["data: {\"choices\":[{\"delta\":{\"content\":\"Привет\"}}]}".to_string()]
        );
        assert!(pending.is_empty());
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
