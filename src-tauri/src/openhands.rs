use std::fs;
use std::io::{Cursor, Read};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{terminate_process_tree, WorkspaceState};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::AsyncReadExt;
use uuid::Uuid;

const SERVER_VERSION: &str = "1.26.0";
const KILO_GATEWAY_BASE: &str = "https://api.kilo.ai/api/gateway";
const UV_RELEASE_API: &str = "https://api.github.com/repos/astral-sh/uv/releases/latest";
const UV_RELEASE_DOWNLOAD: &str = "https://github.com/astral-sh/uv/releases/download";
const UV_WINDOWS_ARCHIVE: &str = "uv-x86_64-pc-windows-msvc.zip";

#[derive(Default)]
pub struct DirectModelState {
    generation: Arc<AtomicU64>,
    approvals:
        tokio::sync::Mutex<std::collections::HashMap<String, tokio::sync::oneshot::Sender<bool>>>,
}
type AgentConfig = (String, String, String, String, String);

#[derive(Deserialize)]
pub struct DirectChatMessage {
    role: String,
    content: String,
}

#[derive(Default)]
pub struct AgentServerState {
    child: Mutex<Option<Child>>,
    port: Mutex<Option<u16>>,
    session_key: Mutex<Option<String>>,
    config: Mutex<Option<AgentConfig>>,
    remote_base: Mutex<Option<String>>,
    log: Arc<Mutex<String>>,
}

#[derive(Serialize)]
pub struct AgentServerStatus {
    pub running: bool,
    pub url: Option<String>,
    pub version: &'static str,
    pub log: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAgentConnection {
    pub url: String,
    pub session_api_key: String,
    pub workspace: String,
    pub provider: String,
    pub model: String,
    pub endpoint: String,
}

#[derive(Serialize, Clone)]
struct AgentEvent {
    delta: String,
    done: bool,
    conversation_id: Option<String>,
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_approvals: Option<Vec<PendingApproval>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file_changes: Option<Vec<AgentFileChange>>,
}

#[derive(Serialize, Clone)]
struct PendingApproval {
    tool_name: String,
    security_risk: String,
    action: serde_json::Value,
}

#[derive(Serialize, Clone)]
struct AgentFileChange {
    id: String,
    path: String,
    before: Option<String>,
    after: String,
    existed_before: bool,
}

#[derive(Deserialize)]
struct UvRelease {
    tag_name: String,
    assets: Vec<UvAsset>,
}

#[derive(Deserialize)]
struct UvAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
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
    #[cfg(not(windows))]
    let _ = command;
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
    let archive_response = client
        .get(asset.browser_download_url)
        .send()
        .await
        .map_err(|error| format!("Не удалось скачать runtime uv: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Скачивание uv завершилось ошибкой: {error}"))?;
    if archive_response
        .content_length()
        .is_some_and(|length| length > 128 * 1024 * 1024)
    {
        return Err("Архив uv превышает безопасный предел 128 MB".into());
    }
    let bytes = archive_response
        .bytes()
        .await
        .map_err(|error| error.to_string())?;
    if bytes.is_empty() || bytes.len() > 128 * 1024 * 1024 {
        return Err("Архив uv имеет недопустимый размер; установка отменена".into());
    }
    let expected_digest =
        if let Some(digest) = asset.digest.as_deref().and_then(parse_sha256_digest) {
            digest
        } else {
            let checksum_url = format!(
                "{UV_RELEASE_DOWNLOAD}/{}/{}.sha256",
                release.tag_name, UV_WINDOWS_ARCHIVE
            );
            let checksum = client
                .get(checksum_url)
                .send()
                .await
                .map_err(|error| format!("Не удалось проверить checksum runtime uv: {error}"))?
                .error_for_status()
                .map_err(|error| format!("Источник checksum uv отклонил запрос: {error}"))?
                .text()
                .await
                .map_err(|error| format!("Не удалось прочитать checksum runtime uv: {error}"))?;
            checksum
                .split_whitespace()
                .next()
                .and_then(parse_sha256_digest)
                .ok_or_else(|| "Источник uv не вернул SHA-256 checksum".to_string())?
        };
    let actual_digest = format!("{:x}", sha2::Sha256::digest(&bytes));
    if !actual_digest.eq_ignore_ascii_case(&expected_digest) {
        return Err(
            "SHA-256 архива uv не совпадает с официальной checksum; установка отменена".into(),
        );
    }

    let directory = app
        .path()
        .app_data_dir()
        .map_err(|error| error.to_string())?
        .join("bin");
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let staging = directory.join(format!("uv-installing-{}", Uuid::new_v4()));
    fs::create_dir(&staging).map_err(|error| error.to_string())?;
    let result = (|| -> Result<(), String> {
        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
            .map_err(|error| format!("Не удалось открыть проверенный архив uv: {error}"))?;
        for name in ["uv.exe", "uvx.exe"] {
            let entry_index = find_uv_executable_index(&mut archive, name)
                .ok_or_else(|| format!("Архив uv не содержит {name}"))?;
            let mut entry = archive
                .by_index(entry_index)
                .map_err(|error| format!("Не удалось прочитать {name} из архива uv: {error}"))?;
            if entry.size() == 0 || entry.size() > 64 * 1024 * 1024 {
                return Err(format!("Файл {name} в архиве uv имеет недопустимый размер"));
            }
            let mut output =
                fs::File::create(staging.join(name)).map_err(|error| error.to_string())?;
            std::io::copy(&mut entry, &mut output).map_err(|error| error.to_string())?;
            output.sync_all().map_err(|error| error.to_string())?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = activate_uv_binaries(&directory, &staging, ["uv.exe", "uvx.exe"]) {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!("Не удалось активировать runtime uv: {error}"));
    }
    fs::remove_dir(&staging).map_err(|error| error.to_string())?;
    Ok(directory.join("uvx.exe"))
}

fn agent_server_runtime_args() -> Vec<String> {
    let mut args = vec![
        "--from".into(),
        format!("openhands-agent-server=={SERVER_VERSION}"),
    ];
    let requirements = [
        format!("openhands-sdk=={SERVER_VERSION}"),
        format!("openhands-tools=={SERVER_VERSION}"),
        format!("openhands-workspace=={SERVER_VERSION}"),
        // OpenHands 1.26 allows FastMCP 4, which selects MCP 2.x. The
        // bundled browser-use MCP server still uses the MCP 1.x decorator API.
        "fastmcp>=3,<4".to_string(),
        "mcp<2".to_string(),
        "posthog>=6,<7".to_string(),
    ];
    for requirement in requirements {
        args.extend(["--with".into(), requirement]);
    }
    args.extend(
        ["agent-server", "--host", "127.0.0.1", "--port"]
            .into_iter()
            .map(str::to_string),
    );
    args
}

fn activate_uv_binaries<const N: usize>(
    directory: &Path,
    staging: &Path,
    names: [&str; N],
) -> Result<(), String> {
    let backups = names.map(|name| directory.join(format!("{name}.previous")));
    let targets = names.map(|name| directory.join(name));
    let sources = names.map(|name| staging.join(name));
    let mut backed_up = 0;
    let mut activated = 0;

    let activation = (|| -> Result<(), String> {
        for (target, backup) in targets.iter().zip(&backups) {
            if target.exists() {
                if backup.exists() {
                    fs::remove_file(backup).map_err(|error| error.to_string())?;
                }
                fs::rename(target, backup).map_err(|error| error.to_string())?;
            }
            backed_up += 1;
        }
        for (source, target) in sources.iter().zip(&targets) {
            fs::rename(source, target).map_err(|error| error.to_string())?;
            activated += 1;
        }
        Ok(())
    })();

    if let Err(error) = activation {
        for target in targets.iter().take(activated) {
            let _ = fs::remove_file(target);
        }
        for (target, backup) in targets.iter().zip(&backups).take(backed_up) {
            if backup.exists() {
                let _ = fs::rename(backup, target);
            }
        }
        return Err(error);
    }

    for backup in backups {
        if backup.exists() {
            fs::remove_file(backup).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn parse_sha256_digest(value: &str) -> Option<String> {
    let digest = value.strip_prefix("sha256:").unwrap_or(value);
    (digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

fn find_uv_executable_index<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<usize> {
    (0..archive.len()).find(|index| {
        archive
            .by_index(*index)
            .ok()
            .map(|entry| entry.name().replace('\\', "/").rsplit('/').next() == Some(name))
            .unwrap_or(false)
    })
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
    let connected_remote = state
        .remote_base
        .lock()
        .ok()
        .is_some_and(|base| base.is_some());
    let running = connected_remote
        || state
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
        url: if connected_remote {
            state.remote_base.lock().ok().and_then(|base| base.clone())
        } else if running {
            port.map(|port| format!("http://127.0.0.1:{port}"))
        } else {
            None
        },
        version: SERVER_VERSION,
        log,
    }
}

fn reuse_or_stop_existing_agent(
    state: &AgentServerState,
    config: &AgentConfig,
) -> Result<bool, String> {
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
                == Some(config)
            {
                return Ok(true);
            }
            terminate_process_tree(current)?;
            *child = None;
        }
    }
    Ok(false)
}

fn remote_key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("Flux Hands Model Provider", "openai-compatible")
        .map_err(|error| error.to_string())
}

fn remote_agent_key_entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("Flux Hands Agent Server", "remote-session-api-key")
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_remote_agent_key(api_key: String) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.len() < 16 {
        return Err("Введите действующий session API key OpenHands".into());
    }
    remote_agent_key_entry()?
        .set_password(api_key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn get_remote_agent_key() -> Result<Option<String>, String> {
    match remote_agent_key_entry()?.get_password() {
        Ok(key) => Ok(Some(key)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn remove_remote_agent_key() -> Result<(), String> {
    match remote_agent_key_entry()?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
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

fn completion_text(value: &serde_json::Value) -> Result<String, String> {
    if let Some(error) = value.get("error") {
        return Err(provider_error_message(error));
    }
    let content = value
        .pointer("/choices/0/message/content")
        .ok_or("Модель вернула ответ без текста")?;
    if let Some(text) = content.as_str() {
        return Ok(text.to_owned());
    }
    if let Some(parts) = content.as_array() {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(serde_json::Value::as_str))
            .collect::<String>();
        if !text.is_empty() {
            return Ok(text);
        }
    }
    Err("Модель вернула ответ без поддерживаемого текстового содержимого".into())
}

fn provider_error_message(error: &serde_json::Value) -> String {
    let message = error
        .get("message")
        .and_then(serde_json::Value::as_str)
        .or_else(|| error.as_str())
        .filter(|message| !message.trim().is_empty())
        .unwrap_or("провайдер не указал причину");
    let nested = error
        .pointer("/metadata/previous_errors/0/raw")
        .and_then(serde_json::Value::as_str)
        .and_then(|raw| {
            serde_json::from_str::<serde_json::Value>(raw)
                .ok()
                .and_then(|value| {
                    value
                        .get("message")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
                .or_else(|| Some(raw.to_owned()))
        })
        .or_else(|| {
            let raw = error.pointer("/metadata/raw")?.as_str()?;
            let raw: serde_json::Value = serde_json::from_str(raw).ok()?;
            raw.get("message")?.as_str().map(str::to_owned)
        })
        .filter(|detail| !detail.trim().is_empty() && !message.contains(detail.as_str()));
    let detail = nested.map_or_else(
        || message.to_owned(),
        |nested| format!("{message}: {nested}"),
    );
    let detail = detail.chars().take(512).collect::<String>();
    let code = error
        .get("code")
        .map(|code| match code {
            serde_json::Value::String(code) => code.clone(),
            _ => code.to_string(),
        })
        .filter(|code| !code.is_empty() && code != "null")
        .map(|code| format!(" ({code})"))
        .unwrap_or_default();
    format!("Ошибка удалённого провайдера{code}: {detail}")
}

fn is_rate_limit_failure(status: reqwest::StatusCode, body: &[u8]) -> bool {
    if status.as_u16() == 429 {
        return true;
    }
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) else {
        return false;
    };
    let code = value.pointer("/error/code");
    code.is_some_and(|code| code.as_u64() == Some(429) || code.as_str() == Some("429"))
        || value
            .pointer("/error/metadata/raw")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|raw| raw.contains("RATE_LIMIT_EXCEEDED"))
}

fn rate_limit_error_from_text(error: &str) -> bool {
    error.contains("HTTP 429")
        || error.contains("провайдера (429)")
        || error.contains("RATE_LIMIT_EXCEEDED")
}

fn unique_fallback_models(
    selected: &str,
    candidates: &[String],
    require_tools: bool,
) -> Vec<String> {
    let mut models = vec![selected.to_owned()];
    for candidate in candidates {
        let candidate = candidate.trim();
        if candidate.is_empty()
            || candidate == selected
            || models.iter().any(|item| item == candidate)
        {
            continue;
        }
        models.push(candidate.to_owned());
        if models.len() == if require_tools { 3 } else { 4 } {
            break;
        }
    }
    models
}

#[tauri::command]
pub async fn send_direct_model_message(
    state: State<'_, DirectModelState>,
    provider: String,
    model: String,
    endpoint: String,
    api_key: Option<String>,
    fallback_models: Vec<String>,
    messages: Vec<DirectChatMessage>,
) -> Result<String, String> {
    let provider = provider.trim().to_ascii_lowercase();
    if provider != "remote" && provider != "kilo" {
        return Err("Прямой запрос поддерживает только удалённые модели".into());
    }
    let model = model.trim();
    if model.is_empty() {
        return Err("Выберите модель перед отправкой".into());
    }
    if messages.is_empty() || messages.len() > 40 {
        return Err("В чате должно быть от 1 до 40 сообщений".into());
    }
    let mut total_chars = 0usize;
    let mut request_messages = Vec::with_capacity(messages.len());
    for message in messages {
        if message.role != "user" && message.role != "assistant" {
            return Err("История чата содержит неизвестную роль сообщения".into());
        }
        total_chars = total_chars.saturating_add(message.content.chars().count());
        if total_chars > 64_000 {
            return Err("История чата превышает лимит 64000 символов".into());
        }
        request_messages
            .push(serde_json::json!({"role": message.role, "content": message.content}));
    }
    let (base, api_key) = if provider == "kilo" {
        (KILO_GATEWAY_BASE.to_string(), String::new())
    } else {
        let base = normalize_endpoint(&endpoint)?;
        #[cfg(target_os = "android")]
        let api_key = api_key.unwrap_or_default().trim().to_owned();
        #[cfg(not(target_os = "android"))]
        let api_key = api_key
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                remote_key_entry()
                    .ok()
                    .and_then(|entry| entry.get_password().ok())
            })
            .unwrap_or_default()
            .trim()
            .to_owned();
        let is_loopback = reqwest::Url::parse(&base)
            .ok()
            .and_then(|url| {
                url.host_str()
                    .map(|host| host == "localhost" || host == "127.0.0.1" || host == "::1")
            })
            .unwrap_or(false);
        if api_key.is_empty() && !is_loopback {
            return Err("Введите API-ключ этой модели в настройках".into());
        }
        (base, api_key)
    };
    if provider == "kilo" {
        send_kilo_chat_with_fallback(
            Arc::clone(&state.generation),
            base,
            model,
            unique_fallback_models(model, &fallback_models, false),
            request_messages,
        )
        .await
    } else {
        send_openai_compatible_chat(
            Arc::clone(&state.generation),
            base,
            model,
            request_messages,
            api_key,
        )
        .await
    }
}

#[derive(Serialize, Clone)]
struct DirectAgentEvent {
    delta: String,
    done: bool,
    conversation_id: Option<String>,
    error: Option<String>,
    status: Option<String>,
    pending_approvals: Option<Vec<PendingApproval>>,
    file_changes: Option<Vec<AgentFileChange>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectAgentRequest {
    provider: String,
    model: String,
    endpoint: String,
    api_key: Option<String>,
    #[serde(default)]
    fallback_models: Vec<String>,
    messages: Vec<DirectChatMessage>,
    allow_project_access: bool,
}

fn direct_agent_tools() -> serde_json::Value {
    serde_json::json!([
        {"type":"function","function":{"name":"list_files","description":"List files and directories inside the opened project.","parameters":{"type":"object","properties":{"path":{"type":"string","description":"Project-relative directory; empty means project root"}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"read_file","description":"Read a UTF-8 text file from the opened project.","parameters":{"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"search_project","description":"Search text in files in the opened project.","parameters":{"type":"object","properties":{"query":{"type":"string"}},"required":["query"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"write_file","description":"Create a new UTF-8 text file or replace an existing project file. Requires user approval.","parameters":{"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false}}},
        {"type":"function","function":{"name":"run_command","description":"Run a terminal command in the opened project. Requires user approval.","parameters":{"type":"object","properties":{"command":{"type":"string"}},"required":["command"],"additionalProperties":false}}}
    ])
}

#[tauri::command]
pub async fn send_direct_model_agent(
    state: State<'_, DirectModelState>,
    workspace: State<'_, WorkspaceState>,
    app: AppHandle,
    request: DirectAgentRequest,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (&state, &workspace, &app, &request);
        return Err("Инструменты проекта доступны только в desktop-версии".into());
    }
    #[cfg(not(target_os = "android"))]
    {
        if !request.allow_project_access {
            return Err("Сначала разрешите передачу файлов проекта удалённой модели".into());
        }
        let _workspace_guard = workspace.begin_agent_task()?;
        let root = workspace.active_root()?;
        let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
        let provider = request.provider.trim().to_ascii_lowercase();
        let model = request.model.trim();
        if !matches!(provider.as_str(), "remote" | "kilo") || model.is_empty() {
            return Err("Выберите удалённую модель перед отправкой".into());
        }
        if request.messages.is_empty() || request.messages.len() > 40 {
            return Err("В чате должно быть от 1 до 40 сообщений".into());
        }
        let mut total_chars = 0usize;
        let mut request_messages = Vec::with_capacity(request.messages.len());
        for message in request.messages {
            if !matches!(message.role.as_str(), "user" | "assistant") {
                return Err("История чата содержит неизвестную роль сообщения".into());
            }
            total_chars = total_chars.saturating_add(message.content.chars().count());
            if total_chars > 64_000 {
                return Err("История чата превышает лимит 64000 символов".into());
            }
            request_messages
                .push(serde_json::json!({"role":message.role,"content":message.content}));
        }
        let (base, api_key) = if provider == "kilo" {
            (KILO_GATEWAY_BASE.to_string(), String::new())
        } else {
            let base = normalize_endpoint(&request.endpoint)?;
            #[cfg(target_os = "android")]
            let api_key = request.api_key.unwrap_or_default().trim().to_owned();
            #[cfg(not(target_os = "android"))]
            let api_key = request
                .api_key
                .filter(|key| !key.trim().is_empty())
                .or_else(|| {
                    remote_key_entry()
                        .ok()
                        .and_then(|entry| entry.get_password().ok())
                })
                .unwrap_or_default()
                .trim()
                .to_owned();
            if api_key.is_empty()
                && !reqwest::Url::parse(&base)
                    .ok()
                    .and_then(|url| {
                        url.host_str()
                            .map(|host| matches!(host, "localhost" | "127.0.0.1" | "::1"))
                    })
                    .unwrap_or(false)
            {
                return Err("Введите API-ключ этой модели в настройках".into());
            }
            (base, api_key)
        };
        let emitter = DirectAgentEventEmitter(move |event| {
            app.emit("direct-agent-event", event)
                .map_err(|error| error.to_string())
        });
        run_direct_agent_loop(
            &state,
            root,
            base,
            model.to_owned(),
            unique_fallback_models(model, &request.fallback_models, provider == "kilo"),
            api_key,
            request_messages,
            emitter,
        )
        .await
    }
}

struct DirectAgentEventEmitter<F>(F);

impl<F> DirectAgentEventEmitter<F>
where
    F: Fn(DirectAgentEvent) -> Result<(), String>,
{
    fn emit(&self, event: DirectAgentEvent) -> Result<(), String> {
        (self.0)(event)
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_direct_agent_loop<F>(
    state: &DirectModelState,
    root: PathBuf,
    base: String,
    model: String,
    fallback_models: Vec<String>,
    api_key: String,
    mut request_messages: Vec<serde_json::Value>,
    emitter: DirectAgentEventEmitter<F>,
) -> Result<String, String>
where
    F: Fn(DirectAgentEvent) -> Result<(), String> + Send + Sync,
{
    let generation_state = Arc::clone(&state.generation);
    let generation = generation_state.fetch_add(1, Ordering::SeqCst) + 1;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())?;
    let mut file_changes = Vec::new();
    let mut tool_call_count = 0usize;
    for _ in 0..16 {
        let value = request_direct_agent_completion_with_fallback(
            &client,
            &base,
            &model,
            &fallback_models,
            &request_messages,
            &api_key,
            &generation_state,
            generation,
        )
        .await?;
        if let Some(fallback_model) = value
            .get("_flux_fallback_model")
            .and_then(serde_json::Value::as_str)
        {
            let _ = emitter.emit(DirectAgentEvent {
                delta: String::new(),
                done: false,
                conversation_id: None,
                error: None,
                status: Some(format!(
                    "Основная модель Kilo временно ограничена; переключился на {fallback_model}"
                )),
                pending_approvals: None,
                file_changes: None,
            });
        }
        let message = value
            .pointer("/choices/0/message")
            .cloned()
            .ok_or("Модель вернула пустой ответ")?;
        let tool_calls = message
            .get("tool_calls")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        if tool_calls.is_empty() {
            let answer = completion_text(&value)?;
            let _ = emitter.emit(DirectAgentEvent {
                delta: String::new(),
                done: true,
                conversation_id: None,
                error: None,
                status: Some("Удалённая агентская задача завершена".into()),
                pending_approvals: None,
                file_changes: Some(file_changes),
            });
            return Ok(answer);
        }
        if tool_call_count.saturating_add(tool_calls.len()) > 16 {
            return Err(
                "Удалённая модель запросила больше 16 действий инструментов за одну задачу".into(),
            );
        }
        request_messages.push(message);
        for call in tool_calls {
            tool_call_count += 1;
            if generation_state.load(Ordering::SeqCst) != generation {
                return Err("Запрос удалённой модели остановлен".into());
            }
            let id = call
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or("Вызов инструмента не содержит ID")?
                .to_owned();
            let function = call
                .get("function")
                .ok_or("Вызов инструмента не содержит функции")?;
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or("Вызов инструмента не содержит имени")?;
            let args_text = function
                .get("arguments")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("{}");
            if args_text.len() > 2_000_000 {
                return Err("Аргументы инструмента превышают 2 MB".into());
            }
            let args: serde_json::Value = serde_json::from_str(args_text)
                .map_err(|error| format!("Некорректные аргументы инструмента: {error}"))?;
            let result = match name {
                "list_files" => direct_agent_list_files(
                    &root,
                    args.get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                ),
                "read_file" => direct_agent_read_file(
                    &root,
                    args.get("path")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                ),
                "search_project" => direct_agent_search(
                    &root,
                    args.get("query")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or(""),
                ),
                "write_file" | "run_command" => {
                    let (sender, receiver) = tokio::sync::oneshot::channel();
                    let approval_id = Uuid::new_v4().to_string();
                    state
                        .approvals
                        .lock()
                        .await
                        .insert(approval_id.clone(), sender);
                    if let Err(error) = emitter.emit(DirectAgentEvent { delta: String::new(), done: false, conversation_id: None, error: None, status: Some("Ожидает подтверждения действия удалённого агента".into()), pending_approvals: Some(vec![PendingApproval { tool_name: name.to_owned(), security_risk: if name == "run_command" { "HIGH".into() } else { "MODERATE".into() }, action: serde_json::json!({"approval_id":approval_id,"tool":name,"arguments":args}) }]), file_changes: None }) {
                    state.approvals.lock().await.remove(&approval_id);
                    return Err(error.to_string());
                }
                    let approved = tokio::select! {
                        result = receiver => result.unwrap_or(false),
                        _ = wait_for_direct_request_cancel(&generation_state, generation) => { state.approvals.lock().await.remove(&approval_id); return Err("Запрос удалённой модели остановлен".into()); }
                    };
                    if !approved {
                        Ok("Действие отклонено пользователем".into())
                    } else if name == "write_file" {
                        direct_agent_write_file(&root, &args, &mut file_changes)
                    } else {
                        direct_agent_run_command(
                            &root,
                            args.get("command")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(""),
                        )
                        .await
                    }
                }
                _ => Err(format!("Неизвестный инструмент: {name}")),
            };
            let mut output = result.unwrap_or_else(|error| format!("Ошибка инструмента: {error}"));
            if output.len() > 64 * 1024 {
                truncate_utf8(&mut output, 64 * 1024);
                output.push_str("\n[вывод сокращён до 64 KB]");
            }
            request_messages
                .push(serde_json::json!({"role":"tool","tool_call_id":id,"content":output}));
            let _ = emitter.emit(DirectAgentEvent {
                delta: String::new(),
                done: false,
                conversation_id: None,
                error: None,
                status: Some("Удалённый агент работает с проектом".into()),
                pending_approvals: None,
                file_changes: Some(file_changes.clone()),
            });
        }
    }
    Err("Удалённый агент превысил лимит 16 циклов инструментов".into())
}

async fn request_direct_agent_completion(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[serde_json::Value],
    api_key: &str,
    generation_state: &AtomicU64,
    generation: u64,
) -> Result<serde_json::Value, String> {
    if serde_json::to_vec(messages)
        .map_err(|error| error.to_string())?
        .len()
        > 2 * 1024 * 1024
    {
        return Err("Контекст удалённого агента превысил лимит 2 MB".into());
    }
    let mut request = client
        .post(format!("{base}/chat/completions"))
        .json(&serde_json::json!({"model":model,"messages":messages,"tools":direct_agent_tools(),"tool_choice":"auto","stream":false}));
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let mut response = tokio::select! {
        result = request.send() => result.map_err(|error| format!("Не удалось подключиться к модели: {error}"))?,
        _ = wait_for_direct_request_cancel(generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
    };
    let status = response.status();
    if response
        .content_length()
        .is_some_and(|length| length > 4 * 1024 * 1024)
    {
        return Err("Ответ удалённого провайдера превышает предел 4 MB".into());
    }
    let mut body = Vec::new();
    loop {
        let chunk = tokio::select! {
            result = response.chunk() => result.map_err(|error| error.to_string())?,
            _ = wait_for_direct_request_cancel(generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
        };
        let Some(chunk) = chunk else { break };
        if body.len().saturating_add(chunk.len()) > 4 * 1024 * 1024 {
            return Err("Ответ удалённого провайдера превышает предел 4 MB".into());
        }
        body.extend_from_slice(&chunk);
    }
    let rate_limited = is_rate_limit_failure(status, &body);
    if !status.is_success() {
        return Err(format_http_provider_error(status, &body));
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| format!("Провайдер вернул некорректный JSON: {error}"))?;
    if let Some(error) = value.get("error") {
        let message = provider_error_message(error);
        return Err(if rate_limited && !rate_limit_error_from_text(&message) {
            format!("HTTP 429: {message}")
        } else {
            message
        });
    }
    Ok(value)
}

#[allow(clippy::too_many_arguments)]
async fn request_direct_agent_completion_with_fallback(
    client: &reqwest::Client,
    base: &str,
    selected_model: &str,
    models: &[String],
    messages: &[serde_json::Value],
    api_key: &str,
    generation_state: &AtomicU64,
    generation: u64,
) -> Result<serde_json::Value, String> {
    let mut last_error = "Kilo не вернул ответ".to_owned();
    for (index, model) in models.iter().take(3).enumerate() {
        if index > 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                _ = wait_for_direct_request_cancel(generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
            }
        }
        match request_direct_agent_completion(
            client,
            base,
            model,
            messages,
            api_key,
            generation_state,
            generation,
        )
        .await
        {
            Ok(mut value) => {
                if model != selected_model {
                    value["_flux_fallback_model"] = serde_json::Value::String(model.clone());
                }
                return Ok(value);
            }
            Err(error) if rate_limit_error_from_text(&error) && index + 1 < models.len() => {
                last_error = error;
            }
            Err(error) => {
                return Err(if rate_limit_error_from_text(&error) {
                    format!("Бесплатная модель Kilo временно ограничена. Попробуйте ещё раз через минуту или выберите другую бесплатную модель в настройках. ({error})")
                } else {
                    error
                })
            }
        }
    }
    Err(format!(
        "Доступные бесплатные модели Kilo сейчас ограничены по частоте. Повторите задачу через минуту. ({last_error})"
    ))
}

fn direct_agent_target(root: &Path, relative: &str, must_exist: bool) -> Result<PathBuf, String> {
    let candidate = Path::new(relative);
    if candidate.is_absolute() {
        return Err(
            "Инструменты удалённого агента принимают только относительные пути проекта".into(),
        );
    }
    let joined = root.join(candidate);
    let canonical = match std::fs::symlink_metadata(&joined) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(
                    "Инструменты удалённого агента не открывают символические ссылки".into(),
                );
            }
            std::fs::canonicalize(&joined).map_err(|error| error.to_string())?
        }
        Err(error) if !must_exist && error.kind() == std::io::ErrorKind::NotFound => {
            let parent = joined.parent().ok_or("У пути файла отсутствует папка")?;
            let canonical_parent =
                std::fs::canonicalize(parent).map_err(|error| error.to_string())?;
            if !canonical_parent.starts_with(root) {
                return Err("Доступ за пределы проекта запрещён".into());
            }
            canonical_parent.join(joined.file_name().ok_or("Имя файла отсутствует")?)
        }
        Err(error) => return Err(error.to_string()),
    };
    if !canonical.starts_with(root) {
        return Err("Доступ за пределы проекта запрещён".into());
    }
    Ok(canonical)
}

fn direct_agent_list_files(root: &Path, relative: &str) -> Result<String, String> {
    let path = if relative.is_empty() {
        root.to_path_buf()
    } else {
        direct_agent_target(root, relative, true)?
    };
    let mut rows = std::fs::read_dir(path)
        .map_err(|error| error.to_string())?
        .flatten()
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            if kind.is_symlink() {
                return None;
            }
            Some(format!(
                "{}{}",
                entry.file_name().to_string_lossy(),
                if kind.is_dir() { "/" } else { "" }
            ))
        })
        .take(300)
        .collect::<Vec<_>>();
    rows.sort();
    Ok(rows.join("\n"))
}

fn direct_agent_read_file(root: &Path, relative: &str) -> Result<String, String> {
    let path = direct_agent_target(root, relative, true)?;
    if !path.is_file()
        || std::fs::metadata(&path)
            .map_err(|error| error.to_string())?
            .len()
            > 512 * 1024
    {
        return Err("Файл отсутствует, не является обычным файлом или больше 512 KB".into());
    }
    std::fs::read_to_string(path)
        .map_err(|error| format!("Не удалось прочитать текстовый файл: {error}"))
}

fn direct_agent_search(root: &Path, query: &str) -> Result<String, String> {
    let query = query.trim();
    if query.is_empty() || query.len() > 256 {
        return Err("Поисковый запрос должен содержать от 1 до 256 символов".into());
    }
    let query_lower = query.to_lowercase();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut out = Vec::new();
    let mut visited = 0usize;
    let mut scanned_bytes = 0u64;
    while let Some((dir, depth)) = stack.pop() {
        if depth > 12 || out.len() >= 100 || visited >= 4000 || scanned_bytes >= 32 * 1024 * 1024 {
            break;
        }
        for entry in std::fs::read_dir(dir)
            .map_err(|error| error.to_string())?
            .flatten()
        {
            if out.len() >= 100 || visited >= 4000 || scanned_bytes >= 32 * 1024 * 1024 {
                break;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(
                name.as_str(),
                ".git" | "node_modules" | "target" | "dist" | "build"
            ) {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) if !kind.is_symlink() => kind,
                _ => continue,
            };
            if kind.is_dir() {
                stack.push((entry.path(), depth + 1));
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let size = match entry.metadata() {
                Ok(metadata) if metadata.len() <= 1024 * 1024 => metadata.len(),
                _ => continue,
            };
            if scanned_bytes.saturating_add(size) > 32 * 1024 * 1024 {
                break;
            }
            scanned_bytes += size;
            visited += 1;
            let text = match std::fs::read_to_string(entry.path()) {
                Ok(text) => text,
                Err(_) => continue,
            };
            for (line, content) in text.lines().enumerate() {
                if content.to_lowercase().contains(&query_lower) {
                    out.push(format!(
                        "{}:{}: {}",
                        entry
                            .path()
                            .strip_prefix(root)
                            .unwrap_or(&entry.path())
                            .display(),
                        line + 1,
                        content.chars().take(300).collect::<String>()
                    ));
                    if out.len() >= 100 {
                        break;
                    }
                }
            }
        }
    }
    Ok(out.join("\n"))
}

fn direct_agent_write_file(
    root: &Path,
    args: &serde_json::Value,
    changes: &mut Vec<AgentFileChange>,
) -> Result<String, String> {
    let relative = args
        .get("path")
        .and_then(serde_json::Value::as_str)
        .ok_or("Не указан путь файла")?;
    let content = args
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or("Не указано содержимое файла")?;
    if content.len() > 2_000_000 {
        return Err("Содержимое файла превышает 2 MB".into());
    }
    let target = direct_agent_target(root, relative, false)?;
    let before = if target.exists() {
        Some(std::fs::read_to_string(&target).map_err(|error| error.to_string())?)
    } else {
        None
    };
    if target.is_dir() {
        return Err("Нельзя заменить папку файлом".into());
    }
    if let Some(parent) = target.parent() {
        if !parent.starts_with(root) {
            return Err("Папка файла находится за пределами проекта".into());
        }
    }
    if before.as_deref() == Some(content) {
        return Ok("Содержимое уже совпадает, изменений нет".into());
    }
    std::fs::write(&target, content).map_err(|error| error.to_string())?;
    changes.push(AgentFileChange {
        id: Uuid::new_v4().to_string(),
        path: target.to_string_lossy().into_owned(),
        before: before.clone(),
        after: content.to_owned(),
        existed_before: before.is_some(),
    });
    Ok(format!(
        "Файл записан: {}",
        target.strip_prefix(root).unwrap_or(&target).display()
    ))
}

async fn direct_agent_run_command(root: &Path, command: &str) -> Result<String, String> {
    if command.trim().is_empty() || command.len() > 8_000 {
        return Err("Команда пуста или длиннее 8000 символов".into());
    }
    let mut process = if cfg!(windows) {
        let mut cmd = tokio::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-NonInteractive", "-Command", command]);
        cmd
    } else {
        let mut cmd = tokio::process::Command::new("sh");
        cmd.args(["-lc", command]);
        cmd
    };
    process
        .current_dir(root)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = process
        .spawn()
        .map_err(|error| format!("Не удалось запустить команду: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or("Не удалось прочитать stdout команды")?;
    let stderr = child
        .stderr
        .take()
        .ok_or("Не удалось прочитать stderr команды")?;
    let stdout_task = tokio::spawn(read_command_output(stdout, 32_000));
    let stderr_task = tokio::spawn(read_command_output(stderr, 32_000));
    let status = tokio::time::timeout(Duration::from_secs(90), child.wait())
        .await
        .map_err(|_| "Команда остановлена по тайм-ауту 90 секунд".to_string())?
        .map_err(|error| format!("Не удалось запустить команду: {error}"))?;
    let stdout = stdout_task.await.map_err(|error| error.to_string())?;
    let stderr = stderr_task.await.map_err(|error| error.to_string())?;
    let mut text = format!(
        "{}{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    truncate_utf8(&mut text, 64_000);
    Ok(format!(
        "exit_code={}\n{}",
        status.code().unwrap_or(-1),
        text
    ))
}

async fn read_command_output<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Vec<u8> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let read = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let keep = limit.saturating_sub(output.len()).min(read);
        output.extend_from_slice(&chunk[..keep]);
    }
    output
}

fn truncate_utf8(value: &mut String, max_bytes: usize) {
    if value.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value.truncate(boundary);
}

#[tauri::command]
pub async fn respond_to_direct_agent_confirmation(
    state: State<'_, DirectModelState>,
    approval_id: String,
    accept: bool,
) -> Result<(), String> {
    resolve_direct_agent_confirmation(&state, &approval_id, accept).await
}

async fn resolve_direct_agent_confirmation(
    state: &DirectModelState,
    approval_id: &str,
    accept: bool,
) -> Result<(), String> {
    let sender = state
        .approvals
        .lock()
        .await
        .remove(approval_id)
        .ok_or("Подтверждение уже истекло или задача остановлена")?;
    sender
        .send(accept)
        .map_err(|_| "Удалённый агент уже завершил запрос".into())
}

async fn send_openai_compatible_chat(
    generation_state: Arc<AtomicU64>,
    base: String,
    model: &str,
    messages: Vec<serde_json::Value>,
    api_key: String,
) -> Result<String, String> {
    let generation = generation_state.fetch_add(1, Ordering::SeqCst) + 1;
    let client = build_direct_model_client()?;
    send_openai_compatible_chat_attempt(
        &client,
        &base,
        model,
        &messages,
        &api_key,
        &generation_state,
        generation,
    )
    .await
}

fn build_direct_model_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|error| error.to_string())
}

async fn send_openai_compatible_chat_attempt(
    client: &reqwest::Client,
    base: &str,
    model: &str,
    messages: &[serde_json::Value],
    api_key: &str,
    generation_state: &AtomicU64,
    generation: u64,
) -> Result<String, String> {
    let mut request = client
        .post(format!("{base}/chat/completions"))
        .json(&serde_json::json!({
            "model": model,
            "messages": messages,
            "stream": false
        }));
    if !api_key.is_empty() {
        request = request.bearer_auth(api_key);
    }
    let response = tokio::select! {
        response = request.send() => response
            .map_err(|error| format!("Не удалось подключиться к модели: {error}"))?,
        _ = wait_for_direct_request_cancel(generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
    };
    let status = response.status();
    let body = tokio::select! {
        body = response.bytes() => body
            .map_err(|error| format!("Не удалось прочитать ответ модели: {error}"))?,
        _ = wait_for_direct_request_cancel(generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
    };
    let rate_limited = is_rate_limit_failure(status, &body);
    if !status.is_success() {
        return Err(format_http_provider_error(status, &body));
    }
    let value: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|error| format!("Провайдер вернул некорректный JSON: {error}"))?;
    completion_text(&value).map_err(|error| {
        if rate_limited && !rate_limit_error_from_text(&error) {
            format!("HTTP 429: {error}")
        } else {
            error
        }
    })
}

async fn send_kilo_chat_with_fallback(
    generation_state: Arc<AtomicU64>,
    base: String,
    selected_model: &str,
    models: Vec<String>,
    messages: Vec<serde_json::Value>,
) -> Result<String, String> {
    let generation = generation_state.fetch_add(1, Ordering::SeqCst) + 1;
    let client = build_direct_model_client()?;
    let mut last_error = "Kilo не вернул ответ".to_owned();
    let mut candidates = vec![selected_model.to_owned()];
    candidates.extend(
        models
            .into_iter()
            .filter(|model| model != selected_model)
            .take(3),
    );
    for (index, model) in candidates.iter().enumerate() {
        if index > 0 {
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                _ = wait_for_direct_request_cancel(&generation_state, generation) => return Err("Запрос удалённой модели остановлен".into()),
            }
        }
        match send_openai_compatible_chat_attempt(
            &client,
            &base,
            model,
            &messages,
            "",
            &generation_state,
            generation,
        )
        .await
        {
            Ok(answer) if index == 0 => return Ok(answer),
            Ok(answer) => {
                return Ok(format!(
                    "[Основная модель Kilo временно ограничена; ответ получен от бесплатной модели {model}.]\n\n{answer}"
                ))
            }
            Err(error) if rate_limit_error_from_text(&error) && index + 1 < candidates.len() => {
                last_error = error;
            }
            Err(error) => return Err(if rate_limit_error_from_text(&error) {
                format!("Бесплатная модель Kilo временно ограничена. Попробуйте ещё раз через минуту или выберите другую бесплатную модель в настройках. ({error})")
            } else {
                error
            }),
        }
    }
    Err(format!(
        "Доступные бесплатные модели Kilo сейчас ограничены по частоте. Повторите запрос через минуту. ({last_error})"
    ))
}

fn format_http_provider_error(status: reqwest::StatusCode, body: &[u8]) -> String {
    if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(error) = value.get("error") {
            return format!(
                "Провайдер вернул HTTP {status}: {}",
                provider_error_message(error)
            );
        }
    }
    let mut detail = String::from_utf8_lossy(body).into_owned();
    truncate_utf8(&mut detail, 512);
    format!("Провайдер вернул HTTP {status}: {detail}")
}

async fn wait_for_direct_request_cancel(generation_state: &AtomicU64, generation: u64) {
    loop {
        if generation_state.load(Ordering::SeqCst) != generation {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tauri::command]
pub fn cancel_direct_model_message(state: State<'_, DirectModelState>) {
    state.generation.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut approvals) = state.approvals.try_lock() {
        for (_, sender) in approvals.drain() {
            let _ = sender.send(false);
        }
    }
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
    free_models.retain(|model| {
        model.get("id").and_then(serde_json::Value::as_str) != Some("kilo-auto/free")
    });
    free_models.push(serde_json::json!({
        "id": "kilo-auto/free",
        "name": "Kilo Auto Free",
        "description": "Kilo automatically selects an available free model; no account or API key required.",
        "contextLength": null,
        "mayTrainOnYourPrompts": false,
        "supportsTools": true
    }));
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
    workspace_state: State<'_, WorkspaceState>,
    workspace: String,
    provider: String,
    model: String,
    endpoint: String,
) -> Result<AgentServerStatus, String> {
    let workspace = Path::new(&workspace)
        .canonicalize()
        .map_err(|error| format!("Не удалось открыть проект: {error}"))?;
    if workspace != workspace_state.active_root()? {
        return Err("Путь Agent Server не совпадает с открытым проектом Flux Hands".into());
    }
    if !workspace.is_dir() {
        return Err("Путь проекта не является папкой".into());
    }
    *state
        .remote_base
        .lock()
        .map_err(|error| error.to_string())? = None;
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
    if reuse_or_stop_existing_agent(&state, &config)? {
        return Ok(status(&state));
    }
    let executable = install_uvx(&app).await?;
    let port = free_port()?;
    let vscode_port = free_port()?;
    let session_key = saved_session_key(&app)?;
    let directory = data_dir(&app)?;
    let mut command = Command::new(executable);
    command
        .args(agent_server_runtime_args())
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
pub async fn connect_remote_agent_server(
    app: AppHandle,
    state: State<'_, AgentServerState>,
    connection: RemoteAgentConnection,
) -> Result<AgentServerStatus, String> {
    let mut parsed = reqwest::Url::parse(connection.url.trim())
        .map_err(|_| "Введите корректный URL удалённого Agent Server".to_string())?;
    if parsed.host_str().is_none() {
        return Err("В URL Agent Server не указан сервер".into());
    }
    let host = parsed.host_str().unwrap_or_default();
    let is_loopback =
        host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && is_loopback) {
        return Err(
            "Удалённый Agent Server должен использовать HTTPS. HTTP разрешён только для localhost."
                .into(),
        );
    }
    parsed.set_path("");
    parsed.set_query(None);
    parsed.set_fragment(None);
    let key = connection.session_api_key.trim();
    if key.len() < 16 {
        return Err("Введите действующий session API key OpenHands".into());
    }
    if connection.workspace.trim().is_empty() {
        return Err("Укажите путь проекта на удалённом сервере".into());
    }
    let provider = connection.provider.trim().to_ascii_lowercase();
    if provider != "remote" && provider != "kilo" {
        return Err("Android поддерживает только удалённые модели".into());
    }
    let model = connection.model.trim();
    if model.is_empty() {
        return Err("Сначала выберите удалённую модель".into());
    }
    let endpoint = if provider == "kilo" {
        KILO_GATEWAY_BASE.to_string()
    } else {
        normalize_endpoint(&connection.endpoint)?
    };
    let api_key = if provider == "kilo" {
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
        return Err("Сохраните API-ключ провайдера в настройках удалённых моделей".into());
    }
    let api_key = if api_key.is_empty() {
        "local".into()
    } else {
        api_key
    };
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(
            parsed
                .join("/api/conversations/search?limit=1")
                .map_err(|error| error.to_string())?,
        )
        .header("X-Session-API-Key", key)
        .send()
        .await
        .map_err(|error| format!("Не удалось подключиться к Agent Server: {error}"))?;
    if response.status().is_server_error() || response.status().as_u16() == 404 {
        return Err(format!("Agent Server недоступен по адресу {}", parsed));
    }
    if response.status().as_u16() == 401 || response.status().as_u16() == 403 {
        return Err("Agent Server отклонил session API key".into());
    }
    // OpenHands versions may return either a list or a paginated object here;
    // a successful authenticated response is sufficient for connectivity.
    if !response.status().is_success() {
        return Err(format!("Agent Server вернул HTTP {}", response.status()));
    }
    let workspace = connection.workspace.trim().to_owned();
    let config = (workspace, provider, endpoint, model.to_owned(), api_key);
    *state.config.lock().map_err(|error| error.to_string())? = Some(config);
    *state
        .session_key
        .lock()
        .map_err(|error| error.to_string())? = Some(key.to_owned());
    *state.port.lock().map_err(|error| error.to_string())? = None;
    *state
        .remote_base
        .lock()
        .map_err(|error| error.to_string())? =
        Some(parsed.to_string().trim_end_matches('/').to_owned());
    let _ = app;
    Ok(AgentServerStatus {
        running: true,
        url: Some(parsed.to_string().trim_end_matches('/').to_owned()),
        version: SERVER_VERSION,
        log: "Подключено к удалённому OpenHands Agent Server".into(),
    })
}

#[tauri::command]
pub fn stop_agent_server(state: State<'_, AgentServerState>) -> Result<(), String> {
    if let Some(mut child) = state
        .child
        .lock()
        .map_err(|error| error.to_string())?
        .take()
    {
        terminate_process_tree(&mut child)?;
    }
    *state.port.lock().map_err(|error| error.to_string())? = None;
    *state
        .session_key
        .lock()
        .map_err(|error| error.to_string())? = None;
    *state
        .remote_base
        .lock()
        .map_err(|error| error.to_string())? = None;
    *state.config.lock().map_err(|error| error.to_string())? = None;
    Ok(())
}

#[tauri::command]
#[allow(non_snake_case)]
pub async fn send_agent_message(
    app: AppHandle,
    state: State<'_, AgentServerState>,
    workspace_state: State<'_, WorkspaceState>,
    message: String,
    conversationId: Option<String>,
) -> Result<String, String> {
    let _agent_workspace_guard = workspace_state.begin_agent_task()?;
    let (base, key, config) = {
        let port = state.port.lock().map_err(|error| error.to_string())?;
        let remote_base = state
            .remote_base
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
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
        let base = match remote_base {
            Some(base) => base,
            None => format!(
                "http://127.0.0.1:{}",
                port.ok_or("OpenHands Agent Server is not running")?
            ),
        };
        (base, key, config)
    };
    let (workspace, provider, endpoint, model, api_key) = config;
    let server_is_local = state
        .remote_base
        .lock()
        .map_err(|error| error.to_string())?
        .is_none();
    if server_is_local && Path::new(&workspace) != workspace_state.active_root()?.as_path() {
        return Err(
            "Проект OpenHands изменился. Перезапустите Agent Server для текущей папки".into(),
        );
    }
    let model_id = if provider == "local" {
        "openai/local-model".to_string()
    } else {
        format!("openai/{model}")
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|error| error.to_string())?;
    let base = format!("{base}/api/conversations");
    let headers = [("X-Session-API-Key", key)];
    let mut conversation_id = conversationId;
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
            let policy = client
                .post(format!("{base}/{id}/confirmation_policy"))
                .header(headers[0].0, &headers[0].1)
                .json(&serde_json::json!({"policy":{"kind":"AlwaysConfirm"}}))
                .send()
                .await
                .map_err(|error| format!("Не удалось включить подтверждения OpenHands: {error}"))?;
            ensure_success(policy, "OpenHands не включил подтверждения действий").await?;
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
                "confirmation_policy": {"kind":"AlwaysConfirm"},
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
            status: None,
            pending_approvals: None,
            file_changes: None,
        },
    );
    let mut answer = String::new();
    let mut final_status = String::new();
    let mut was_interrupted = false;
    let mut action_rejected = false;
    let mut assistant_messages = std::collections::HashSet::new();
    let mut pending_actions = std::collections::BTreeMap::<String, PendingApproval>::new();
    let mut file_changes = Vec::<AgentFileChange>::new();
    let mut approval_prompt_emitted = false;
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
                            status: None,
                            pending_approvals: None,
                            file_changes: None,
                        },
                    );
                }
            }
            if kind == "ActionEvent" {
                let name = event
                    .get("tool_name")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("tool");
                if let (Some(action_id), Some(action)) = (
                    event.get("id").and_then(serde_json::Value::as_str),
                    event.get("action").filter(|action| !action.is_null()),
                ) {
                    pending_actions.insert(
                        action_id.to_owned(),
                        PendingApproval {
                            tool_name: name.to_owned(),
                            security_risk: event
                                .get("security_risk")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or("UNKNOWN")
                                .to_owned(),
                            action: action.clone(),
                        },
                    );
                }
                let activity = format!("\n\n[OpenHands] Запускает инструмент: {name}\n");
                answer.push_str(&activity);
                let _ = app.emit(
                    "openhands-agent-event",
                    AgentEvent {
                        delta: activity,
                        done: false,
                        conversation_id: Some(id.clone()),
                        error: None,
                        status: None,
                        pending_approvals: None,
                        file_changes: None,
                    },
                );
            }
            if kind == "ObservationEvent" {
                if event.get("tool_name").and_then(serde_json::Value::as_str) == Some("file_editor")
                {
                    if let Some(observation) = event.get("observation") {
                        let succeeded = observation
                            .get("is_error")
                            .and_then(serde_json::Value::as_bool)
                            != Some(true);
                        let path = observation.get("path").and_then(serde_json::Value::as_str);
                        let before = observation
                            .get("old_content")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned);
                        let after = observation
                            .get("new_content")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned);
                        let existed_before = observation
                            .get("prev_exist")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true);
                        if succeeded {
                            if let (Some(action_id), Some(path), Some(after)) = (
                                event.get("action_id").and_then(serde_json::Value::as_str),
                                path,
                                after,
                            ) {
                                if (existed_before && before.is_some())
                                    || (!existed_before && before.is_none())
                                {
                                    file_changes.push(AgentFileChange {
                                        id: action_id.to_owned(),
                                        path: path.to_owned(),
                                        before,
                                        after,
                                        existed_before,
                                    });
                                }
                            }
                        }
                    }
                }
                if let Some(action_id) = event.get("action_id").and_then(serde_json::Value::as_str)
                {
                    pending_actions.remove(action_id);
                }
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
                            status: None,
                            pending_approvals: None,
                            file_changes: Some(file_changes.clone()),
                        },
                    );
                }
            }
            if kind == "UserRejectObservation" {
                action_rejected = true;
                if let Some(action_id) = event.get("action_id").and_then(serde_json::Value::as_str)
                {
                    pending_actions.remove(action_id);
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
                    was_interrupted |= final_status == "paused";
                } else if key == Some("full_state") {
                    final_status = event
                        .pointer("/value/execution_status")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_owned();
                    was_interrupted |= final_status == "paused";
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
        if final_status == "waiting_for_confirmation" && !approval_prompt_emitted {
            let approvals = pending_actions.values().cloned().collect::<Vec<_>>();
            if approvals.is_empty() {
                return Err(
                    "OpenHands ожидает подтверждение, но не передал действие для проверки".into(),
                );
            }
            let _ = app.emit(
                "openhands-agent-event",
                AgentEvent {
                    delta: String::new(),
                    done: false,
                    conversation_id: Some(id.clone()),
                    error: None,
                    status: None,
                    pending_approvals: Some(approvals),
                    file_changes: None,
                },
            );
            approval_prompt_emitted = true;
        } else if final_status != "waiting_for_confirmation" {
            approval_prompt_emitted = false;
        }
        if ["idle", "finished", "paused", "error", "stuck"].contains(&final_status.as_str()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    if final_status.is_empty() || final_status == "running" {
        return Err("Превышено время ожидания выполнения задачи OpenHands".into());
    }
    let completion_status = if action_rejected {
        "Действие отклонено · выполнение остановлено"
    } else if was_interrupted {
        "Задача остановлена · разговор можно продолжить"
    } else {
        "Готово"
    };
    let completion_message = if action_rejected {
        "\n\nДействие отклонено. Агент остановлен; отправьте новое сообщение, чтобы продолжить задачу."
    } else if was_interrupted {
        "\n\nЗадача остановлена по запросу пользователя. Отправьте новое сообщение, чтобы продолжить разговор."
    } else {
        ""
    };
    answer.push_str(completion_message);
    let _ = app.emit(
        "openhands-agent-event",
        AgentEvent {
            delta: completion_message.to_owned(),
            done: true,
            conversation_id: Some(id),
            error: None,
            status: Some(completion_status.to_owned()),
            pending_approvals: None,
            file_changes: Some(file_changes),
        },
    );
    if answer.trim().is_empty() {
        return Err(format!(
            "OpenHands завершил задачу со статусом {final_status} без текстового ответа"
        ));
    }
    Ok(answer)
}

#[tauri::command]
#[allow(non_snake_case)]
pub async fn respond_to_agent_confirmation(
    state: State<'_, AgentServerState>,
    conversationId: String,
    accept: bool,
) -> Result<(), String> {
    let conversation_id = Uuid::parse_str(&conversationId)
        .map_err(|_| "ID разговора OpenHands имеет неверный формат".to_owned())?;
    let (base, key) = {
        let port = state.port.lock().map_err(|error| error.to_string())?;
        let remote_base = state
            .remote_base
            .lock()
            .map_err(|error| error.to_string())?
            .clone();
        let key = state
            .session_key
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or("OpenHands session key is missing")?;
        let base = match remote_base {
            Some(base) => base,
            None => format!(
                "http://127.0.0.1:{}",
                port.ok_or("OpenHands Agent Server is not running")?
            ),
        };
        (base, key)
    };
    respond_to_confirmation(&base, &key, conversation_id, accept).await
}

async fn respond_to_confirmation(
    base: &str,
    key: &str,
    conversation_id: Uuid,
    accept: bool,
) -> Result<(), String> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?
        .post(format!(
            "{base}/api/conversations/{conversation_id}/events/respond_to_confirmation"
        ))
        .header("X-Session-API-Key", key)
        .json(&serde_json::json!({
            "accept": accept,
            "reason": if accept { "Approved in Flux Hands." } else { "Rejected in Flux Hands." }
        }))
        .send()
        .await
        .map_err(|error| format!("Не удалось отправить решение OpenHands: {error}"))?;
    ensure_success(response, "OpenHands не принял решение по действию").await
}

#[tauri::command]
#[allow(non_snake_case)]
pub async fn interrupt_agent_conversation(
    state: State<'_, AgentServerState>,
    conversationId: String,
) -> Result<(), String> {
    let conversation_id = Uuid::parse_str(&conversationId)
        .map_err(|_| "ID разговора OpenHands имеет неверный формат".to_owned())?;
    let (base, key) = agent_connection(state.inner())?;
    interrupt_conversation(&base, &key, conversation_id).await
}

fn agent_connection(state: &AgentServerState) -> Result<(String, String), String> {
    let port = state.port.lock().map_err(|error| error.to_string())?;
    let remote_base = state
        .remote_base
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    let key = state
        .session_key
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
        .ok_or("OpenHands session key is missing")?;
    let base = match remote_base {
        Some(base) => base,
        None => format!(
            "http://127.0.0.1:{}",
            port.ok_or("OpenHands Agent Server is not running")?
        ),
    };
    Ok((base, key))
}

async fn interrupt_conversation(base: &str, key: &str, id: Uuid) -> Result<(), String> {
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?
        .post(format!("{base}/api/conversations/{id}/interrupt"))
        .header("X-Session-API-Key", key)
        .send()
        .await
        .map_err(|error| format!("Не удалось прервать задачу OpenHands: {error}"))?;
    ensure_success(response, "OpenHands не прервал задачу").await
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

pub fn stop_on_exit(app: &AppHandle) {
    let state = app.state::<AgentServerState>();
    if let Ok(mut child) = state.child.lock() {
        if let Some(mut child) = child.take() {
            let _ = terminate_process_tree(&mut child);
        }
    };
}

#[cfg(test)]
mod tests {
    use super::{
        activate_uv_binaries, agent_connection, agent_server_runtime_args, completion_text,
        direct_agent_list_files, direct_agent_read_file, direct_agent_run_command,
        direct_agent_search, direct_agent_target, direct_agent_tools, direct_agent_write_file,
        find_uv_executable_index, interrupt_conversation, kilo_model_is_free_text,
        parse_sha256_digest, request_direct_agent_completion, resolve_direct_agent_confirmation,
        respond_to_confirmation, reuse_or_stop_existing_agent, run_direct_agent_loop, status,
        AgentConfig, AgentFileChange, AgentServerState, DirectAgentEventEmitter, DirectModelState,
        UV_WINDOWS_ARCHIVE,
    };
    use std::fs;
    use std::io::Cursor;
    use std::process::{Command, Stdio};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use uuid::Uuid;

    #[test]
    fn interrupt_uses_the_openhands_route_and_session_key() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!(
                    "POST /api/conversations/{}/interrupt HTTP/1.1",
                    uuid::Uuid::nil()
                )));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("x-session-api-key: test-session"));
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
            });

            interrupt_conversation(
                &format!("http://{address}"),
                "test-session",
                uuid::Uuid::nil(),
            )
            .await
            .unwrap();
            server.join().unwrap();
        });
    }

    #[test]
    fn approval_uses_the_confirmation_route_session_key_and_accept_flag() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = [0_u8; 4096];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with(&format!(
                    "POST /api/conversations/{}/events/respond_to_confirmation HTTP/1.1",
                    uuid::Uuid::nil()
                )));
                assert!(request
                    .to_ascii_lowercase()
                    .contains("x-session-api-key: test-session"));
                let body = request.split("\r\n\r\n").nth(1).unwrap();
                let body: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(body["accept"], true);
                assert_eq!(body["reason"], "Approved in Flux Hands.");
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                    .unwrap();
            });

            respond_to_confirmation(
                &format!("http://{address}"),
                "test-session",
                uuid::Uuid::nil(),
                true,
            )
            .await
            .unwrap();
            server.join().unwrap();
        });
    }

    #[test]
    fn uv_sha256_parser_accepts_api_and_sidecar_formats() {
        let digest = "088962f9e7b7bd9ea740c04c650b2a21c8928c345bd99ac24350dc924dba656c";
        assert_eq!(
            parse_sha256_digest(&format!("sha256:{digest}")),
            Some(digest.into())
        );
        assert_eq!(
            parse_sha256_digest(&format!("{digest}  {UV_WINDOWS_ARCHIVE}")),
            None
        );
        assert_eq!(parse_sha256_digest(digest), Some(digest.into()));
        assert_eq!(parse_sha256_digest("sha256:abcd"), None);
        assert_eq!(parse_sha256_digest(&"g".repeat(64)), None);
    }

    #[test]
    fn uv_sidecar_checksum_ignores_filename_after_digest() {
        let digest = "088962f9e7b7bd9ea740c04c650b2a21c8928c345bd99ac24350dc924dba656c";
        let sidecar = format!("{digest}  {UV_WINDOWS_ARCHIVE}\n");
        assert_eq!(
            parse_sha256_digest(sidecar.split_whitespace().next().unwrap()),
            Some(digest.into())
        );
    }

    #[test]
    fn uv_archive_lookup_supports_flat_and_nested_executable_paths() {
        use std::io::Write;
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut bytes);
            for path in ["uv.exe", "uvw.exe", "uv-x86_64-pc-windows-msvc/uvx.exe"] {
                writer
                    .start_file(path, zip::write::SimpleFileOptions::default())
                    .unwrap();
                writer.write_all(b"binary").unwrap();
            }
            writer.finish().unwrap();
        }
        bytes.set_position(0);
        let mut archive = zip::ZipArchive::new(bytes).unwrap();
        assert_eq!(find_uv_executable_index(&mut archive, "uv.exe"), Some(0));
        assert_eq!(find_uv_executable_index(&mut archive, "uvx.exe"), Some(2));
        assert_eq!(find_uv_executable_index(&mut archive, "python.exe"), None);
    }

    #[test]
    fn direct_model_request_works_without_an_agent_server() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = vec![0; 16_384];
                let count = stream.read(&mut request).unwrap();
                let request = String::from_utf8_lossy(&request[..count]);
                assert!(request.starts_with("POST /v1/chat/completions HTTP/1.1"));
                let body: serde_json::Value =
                    serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                assert_eq!(body["model"], "test-model");
                assert_eq!(body["messages"][0]["content"], "Earlier question");
                assert_eq!(body["messages"][1]["content"], "Current question");
                let response = r#"{"choices":[{"message":{"content":"Direct response"}}]}"#;
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(), response
                ).unwrap();
            });
            let answer = super::send_openai_compatible_chat(
                Arc::new(AtomicU64::new(0)),
                format!("http://{address}/v1"),
                "test-model",
                vec![
                    serde_json::json!({"role":"user", "content":"Earlier question"}),
                    serde_json::json!({"role":"user", "content":"Current question"}),
                ],
                String::new(),
            )
            .await
            .unwrap();
            assert_eq!(answer, "Direct response");
            server.join().unwrap();
        });
    }

    #[test]
    fn kilo_rate_limit_automatically_uses_a_free_fallback_model() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let mut models = Vec::new();
                for (expected_model, response) in [
                    (
                        "kilo-auto/free",
                        r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"{\"code\":429,\"reason\":\"RATE_LIMIT_EXCEEDED\",\"message\":\"temporarily rate-limited\"}"},"user_id":"org_private_gateway_id"}}"#,
                    ),
                    (
                        "provider/available:free",
                        r#"{"choices":[{"message":{"content":"Fallback response"}}]}"#,
                    ),
                ] {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut request = vec![0_u8; 16_384];
                    let count = stream.read(&mut request).unwrap();
                    let request = String::from_utf8_lossy(&request[..count]);
                    let body: serde_json::Value =
                        serde_json::from_str(request.split("\r\n\r\n").nth(1).unwrap()).unwrap();
                    models.push(body["model"].as_str().unwrap().to_owned());
                    assert_eq!(body["model"], expected_model);
                    let status = if expected_model == "kilo-auto/free" {
                        "429 Too Many Requests"
                    } else {
                        "200 OK"
                    };
                    write!(
                        stream,
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                        response.len()
                    )
                    .unwrap();
                }
                models
            });
            let answer = super::send_kilo_chat_with_fallback(
                Arc::new(AtomicU64::new(0)),
                format!("http://{address}/v1"),
                "kilo-auto/free",
                vec!["kilo-auto/free".into(), "provider/available:free".into()],
                vec![serde_json::json!({"role":"user","content":"hello"})],
            )
            .await
            .unwrap();
            assert!(answer.contains("provider/available:free"));
            assert!(answer.contains("Fallback response"));
            assert!(!answer.contains("org_private_gateway_id"));
            assert_eq!(
                server.join().unwrap(),
                ["kilo-auto/free", "provider/available:free"]
            );
        });
    }

    #[test]
    fn direct_model_response_extracts_text_blocks() {
        let response = serde_json::json!({"choices":[{"message":{"content":[
            {"type":"text","text":"first "},
            {"type":"text","text":"second"}
        ]}}]});
        assert_eq!(completion_text(&response).unwrap(), "first second");
    }

    #[test]
    fn direct_model_response_surfaces_provider_errors_even_with_http_200() {
        let response = serde_json::json!({
            "error": {
                "message": "Upstream error from Nvidia: Service temporarily overloaded",
                "code": 503
            }
        });
        let error = completion_text(&response).unwrap_err();
        assert!(error.contains("503"));
        assert!(error.contains("Service temporarily overloaded"));
    }

    #[test]
    fn direct_model_response_surfaces_nested_rate_limit_without_gateway_identity() {
        let response = serde_json::json!({
            "error": {
                "message": "Provider returned error",
                "code": 429,
                "metadata": {
                    "previous_errors": [{
                        "raw": "model is temporarily rate-limited upstream; retry shortly"
                    }]
                }
            },
            "user_id": "org_private_identifier"
        });
        let error = completion_text(&response).unwrap_err();
        assert!(error.contains("429"));
        assert!(error.contains("temporarily rate-limited upstream"));
        assert!(!error.contains("org_private_identifier"));
    }

    #[test]
    fn direct_model_request_can_be_cancelled() {
        use std::io::Read;
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let generation = Arc::new(AtomicU64::new(0));
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buffer = [0; 4096];
                let _ = stream.read(&mut buffer);
                thread::sleep(Duration::from_millis(500));
            });
            let request = tauri::async_runtime::spawn(super::send_openai_compatible_chat(
                Arc::clone(&generation),
                format!("http://{address}/v1"),
                "test-model",
                vec![serde_json::json!({"role":"user", "content":"Cancel this request"})],
                String::new(),
            ));
            tokio::time::sleep(Duration::from_millis(100)).await;
            generation.fetch_add(1, Ordering::SeqCst);
            let result = tokio::time::timeout(Duration::from_secs(2), request)
                .await
                .expect("cancelled remote request should finish promptly")
                .unwrap();
            assert!(result.unwrap_err().contains("остановлен"));
            server.join().unwrap();
        });
    }
    #[test]
    fn uv_binary_activation_restores_previous_pair_if_second_install_fails() {
        let root = std::env::temp_dir().join(format!("flux-uv-activation-{}", Uuid::new_v4()));
        let directory = root.join("bin");
        let staging = root.join("staging");
        fs::create_dir_all(&directory).unwrap();
        fs::create_dir_all(&staging).unwrap();
        fs::write(directory.join("uv.exe"), b"old uv").unwrap();
        fs::write(directory.join("uvx.exe"), b"old uvx").unwrap();
        fs::write(staging.join("uv.exe"), b"new uv").unwrap();

        let result = activate_uv_binaries(&directory, &staging, ["uv.exe", "uvx.exe"]);

        assert!(result.is_err());
        assert_eq!(fs::read(directory.join("uv.exe")).unwrap(), b"old uv");
        assert_eq!(fs::read(directory.join("uvx.exe")).unwrap(), b"old uvx");
        assert!(!directory.join("uv.exe.previous").exists());
        assert!(!directory.join("uvx.exe.previous").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn agent_connection_prefers_configured_remote_server() {
        let state = AgentServerState::default();
        *state.remote_base.lock().unwrap() = Some("http://agent.example".into());
        *state.session_key.lock().unwrap() = Some("session".into());

        assert_eq!(
            agent_connection(&state).unwrap(),
            ("http://agent.example".into(), "session".into())
        );
    }

    #[test]
    fn agent_server_pins_browser_compatible_fastmcp_and_mcp_versions() {
        let args = agent_server_runtime_args();
        assert!(args
            .windows(2)
            .any(|pair| { pair[0] == "--with" && pair[1] == "fastmcp>=3,<4" }));
        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "--with" && pair[1] == "mcp<2"));
        assert!(args
            .iter()
            .any(|argument| argument == "openhands-agent-server==1.26.0"));
    }

    #[cfg(windows)]
    #[test]
    fn reusing_the_current_agent_releases_the_child_lock_before_status() {
        let state = AgentServerState::default();
        let config: AgentConfig = (
            "workspace".into(),
            "kilo".into(),
            "https://api.kilo.ai/api/gateway".into(),
            "kilo-auto/free".into(),
            "anonymous".into(),
        );
        let mut command = Command::new("ping.exe");
        command
            .args(["-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let child = command.spawn().expect("start temporary ping process");
        *state.child.lock().expect("lock child state") = Some(child);
        *state.config.lock().expect("lock config state") = Some(config.clone());

        assert!(reuse_or_stop_existing_agent(&state, &config).expect("reuse agent"));
        assert!(status(&state).running);

        let child = state.child.lock().expect("lock child state").take();
        if let Some(mut child) = child {
            let _ = crate::terminate_process_tree(&mut child);
        }
    }

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

    #[test]
    fn remote_agent_file_tools_stay_inside_project_and_record_undo_data() {
        let root = std::env::temp_dir().join(format!("flux-agent-tools-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        fs::write(root.join("sample.txt"), "old content").unwrap();

        assert!(direct_agent_target(&root, "../outside.txt", false).is_err());
        assert_eq!(
            direct_agent_read_file(&root, "sample.txt").unwrap(),
            "old content"
        );
        assert!(direct_agent_list_files(&root, "")
            .unwrap()
            .contains("sample.txt"));
        assert!(direct_agent_search(&root, "old content")
            .unwrap()
            .contains("sample.txt:1"));

        let mut changes: Vec<AgentFileChange> = Vec::new();
        direct_agent_write_file(
            &root,
            &serde_json::json!({"path":"sample.txt","content":"new content"}),
            &mut changes,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(root.join("sample.txt")).unwrap(),
            "new content"
        );
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].before.as_deref(), Some("old content"));
        assert_eq!(changes[0].after, "new content");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn remote_agent_completion_sends_tool_schemas_and_parses_tool_calls() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    request.push(byte[0]);
                }
                let headers = String::from_utf8_lossy(&request).to_ascii_lowercase();
                let length = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                let body_start = request.len();
                request.resize(body_start + length, 0);
                stream.read_exact(&mut request[body_start..]).unwrap();
                let body =
                    serde_json::from_slice::<serde_json::Value>(&request[body_start..]).unwrap();

                let response = serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "call-1",
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": serde_json::json!({"path":"sample.txt"}).to_string()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                });
                let response = response.to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                )
                .unwrap();
                (headers, body)
            });

            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(5))
                .build()
                .unwrap();
            let generation = AtomicU64::new(7);
            let messages = vec![serde_json::json!({"role":"user","content":"Read sample.txt"})];
            let response = request_direct_agent_completion(
                &client,
                &format!("http://{address}/v1"),
                "test-model",
                &messages,
                "test-key",
                &generation,
                7,
            )
            .await
            .unwrap();
            let (headers, body) = server.join().unwrap();
            assert!(headers.contains("authorization: bearer test-key"));
            assert_eq!(body["model"], "test-model");
            assert_eq!(body["tool_choice"], "auto");
            assert_eq!(body["tools"], direct_agent_tools());
            assert_eq!(body["messages"][0]["content"], "Read sample.txt");
            assert_eq!(response["choices"][0]["finish_reason"], "tool_calls");
            assert_eq!(
                response["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
                "read_file"
            );
            assert_eq!(
                response["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
                r#"{"path":"sample.txt"}"#
            );
        });
    }

    #[test]
    fn remote_agent_approval_is_one_shot_and_resolves_accept_or_reject() {
        tauri::async_runtime::block_on(async {
            let state = DirectModelState::default();
            let (accepted_sender, accepted_receiver) = tokio::sync::oneshot::channel();
            state
                .approvals
                .lock()
                .await
                .insert("approval-1".into(), accepted_sender);
            resolve_direct_agent_confirmation(&state, "approval-1", true)
                .await
                .unwrap();
            assert!(accepted_receiver.await.unwrap());
            assert!(
                resolve_direct_agent_confirmation(&state, "approval-1", true)
                    .await
                    .is_err()
            );

            let (rejected_sender, rejected_receiver) = tokio::sync::oneshot::channel();
            state
                .approvals
                .lock()
                .await
                .insert("approval-2".into(), rejected_sender);
            resolve_direct_agent_confirmation(&state, "approval-2", false)
                .await
                .unwrap();
            assert!(!rejected_receiver.await.unwrap());
        });
    }

    #[test]
    fn remote_agent_command_runs_in_the_project_and_returns_output() {
        tauri::async_runtime::block_on(async {
            let root = std::env::temp_dir().join(format!("flux-agent-command-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            #[cfg(windows)]
            let command = "Write-Output 'Flux command smoke'";
            #[cfg(not(windows))]
            let command = "printf 'Flux command smoke'";
            let output = direct_agent_run_command(&root, command).await.unwrap();
            assert!(output.starts_with("exit_code=0\n"), "{output}");
            assert!(output.contains("Flux command smoke"), "{output}");
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[test]
    fn remote_agent_loop_waits_for_approval_before_writing_project_file() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::thread;

        tauri::async_runtime::block_on(async {
            let root = std::env::temp_dir().join(format!("flux-agent-loop-{}", Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let root = fs::canonicalize(root).unwrap();
            let output_file = root.join("created.txt");

            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let server = thread::spawn(move || {
                let read_request = |stream: &mut std::net::TcpStream| {
                    let mut request = Vec::new();
                    let mut byte = [0_u8; 1];
                    while !request.ends_with(b"\r\n\r\n") {
                        stream.read_exact(&mut byte).unwrap();
                        request.push(byte[0]);
                    }
                    let headers = String::from_utf8_lossy(&request).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse::<usize>()
                        .unwrap();
                    let body_start = request.len();
                    request.resize(body_start + length, 0);
                    stream.read_exact(&mut request[body_start..]).unwrap();
                    serde_json::from_slice::<serde_json::Value>(&request[body_start..]).unwrap()
                };
                let write_response = |stream: &mut std::net::TcpStream, body: serde_json::Value| {
                    let body = body.to_string();
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                    .unwrap();
                };

                let (mut first, _) = listener.accept().unwrap();
                let _ = read_request(&mut first);
                let first_response = serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"write-1","type":"function","function":{"name":"write_file","arguments":serde_json::json!({"path":"created.txt","content":"created after approval"}).to_string()}}]},"finish_reason":"tool_calls"}]});
                write_response(&mut first, first_response);

                let (mut second, _) = listener.accept().unwrap();
                let second_request = read_request(&mut second);
                assert_eq!(second_request["messages"][2]["role"], "tool");
                assert!(second_request["messages"][2]["content"]
                    .as_str()
                    .unwrap()
                    .contains("created.txt"));
                let final_response = serde_json::json!({"choices":[{"message":{"role":"assistant","content":"The file was created."},"finish_reason":"stop"}]});
                write_response(&mut second, final_response);
            });

            let state = Arc::new(DirectModelState::default());
            let task_state = Arc::clone(&state);
            let task_root = root.clone();
            let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
            let task = tauri::async_runtime::spawn(async move {
                run_direct_agent_loop(
                    &task_state,
                    task_root,
                    format!("http://{address}/v1"),
                    "test-model".into(),
                    vec!["test-model".into()],
                    String::new(),
                    vec![serde_json::json!({"role":"user","content":"Create created.txt"})],
                    DirectAgentEventEmitter(move |event| {
                        event_sender.send(event).map_err(|error| error.to_string())
                    }),
                )
                .await
            });

            let approval_event = event_receiver.recv().await.unwrap();
            let pending = approval_event.pending_approvals.unwrap();
            assert_eq!(pending[0].tool_name, "write_file");
            assert!(
                !output_file.exists(),
                "file must remain untouched before approval"
            );
            let approval_id = pending[0].action["approval_id"].as_str().unwrap();
            resolve_direct_agent_confirmation(&state, approval_id, true)
                .await
                .unwrap();

            let answer = task.await.unwrap().unwrap();
            assert_eq!(answer, "The file was created.");
            assert_eq!(
                fs::read_to_string(&output_file).unwrap(),
                "created after approval"
            );
            let mut saw_done = false;
            while let Some(event) = event_receiver.recv().await {
                if event.done {
                    saw_done = true;
                    assert_eq!(event.file_changes.unwrap().len(), 1);
                }
            }
            assert!(saw_done);
            server.join().unwrap();
            fs::remove_dir_all(root).unwrap();
        });
    }

    #[cfg(windows)]
    #[test]
    fn remote_agent_rejects_windows_junctions_that_escape_project() {
        use std::os::windows::process::CommandExt;

        let parent = std::env::temp_dir().join(format!("flux-agent-junction-{}", Uuid::new_v4()));
        let root = parent.join("project");
        let outside = parent.join("outside");
        let junction = root.join("linked");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.txt"), "secret").unwrap();
        let command = format!(
            "/C mklink /J \"{}\" \"{}\"",
            junction.display(),
            outside.display()
        );
        let output = std::process::Command::new("cmd.exe")
            .raw_arg(command)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );

        let canonical_root = fs::canonicalize(&root).unwrap();
        assert!(direct_agent_read_file(&canonical_root, "linked\\secret.txt").is_err());
        assert!(direct_agent_target(&canonical_root, "linked\\new.txt", false).is_err());

        fs::remove_dir(&junction).unwrap();
        fs::remove_dir_all(parent).unwrap();
    }
}
