use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt;
use keyring::Entry;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

const STREAM_EVENT: &str = "gateway-stream";
const CREDENTIAL_SERVICE: &str = "Flux Code Model Providers";

#[derive(Default)]
pub struct GatewayState {
    next_id: AtomicU64,
    tasks: Arc<Mutex<HashMap<u64, Arc<AtomicBool>>>>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct GatewayMessage {
    pub role: String,
    pub content: String,
}

#[derive(Serialize, Clone)]
struct GatewayEvent {
    request_id: u64,
    delta: String,
    done: bool,
    error: Option<String>,
}

fn provider_key(provider: &str) -> Result<Entry, String> {
    if !matches!(provider, "openai" | "openrouter" | "custom") {
        return Err("Unsupported remote provider".to_string());
    }
    Entry::new(CREDENTIAL_SERVICE, provider).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn save_provider_key(provider: String, api_key: String) -> Result<(), String> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err("API key cannot be empty".to_string());
    }
    provider_key(&provider)?
        .set_password(api_key)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn delete_provider_key(provider: String) -> Result<(), String> {
    match provider_key(&provider)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn provider_key_configured(provider: String) -> Result<bool, String> {
    Ok(provider_key(&provider)?.get_password().is_ok())
}

fn endpoint_for(
    provider: &str,
    endpoint: Option<&str>,
    local_port: Option<u16>,
) -> Result<String, String> {
    match provider {
        "local" => local_port
            .map(|port| format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .ok_or_else(|| {
                "Start a local llama-server before selecting the local provider".to_string()
            }),
        "ollama" => {
            custom_completion_url_with_lan(endpoint.unwrap_or("http://localhost:11434/v1"), false)
        }
        "lmstudio" => {
            custom_completion_url_with_lan(endpoint.unwrap_or("http://localhost:1234/v1"), false)
        }
        "openai" => Ok("https://api.openai.com/v1/chat/completions".to_string()),
        "openrouter" => Ok("https://openrouter.ai/api/v1/chat/completions".to_string()),
        "kilo" => Ok("https://api.kilo.ai/api/gateway/chat/completions".to_string()),
        "custom" => {
            let endpoint = endpoint.unwrap_or_default().trim().trim_end_matches('/');
            custom_completion_url(endpoint)
        }
        _ => Err("Unsupported model provider".to_string()),
    }
}

fn models_url(completion_url: &str) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(completion_url).map_err(|_| "Invalid provider URL".to_string())?;
    let mut models = parsed;
    let path = models.path().trim_end_matches("/chat/completions");
    models.set_path(&format!("{path}/models"));
    Ok(models.to_string())
}

fn configured_or_override_key(provider: &str, override_key: Option<String>) -> Option<String> {
    override_key
        .filter(|key| !key.trim().is_empty())
        .or_else(|| provider_key(provider).ok()?.get_password().ok())
}

async fn list_kilo_free_models() -> Result<Vec<String>, String> {
    let payload: serde_json::Value = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())?
        .get("https://api.kilo.ai/api/gateway/models")
        .send()
        .await
        .map_err(|error| format!("Could not connect to Kilo free model catalog: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Kilo free model catalog rejected the request: {error}"))?
        .json()
        .await
        .map_err(|error| format!("Kilo returned an invalid model list: {error}"))?;
    let models = payload
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Kilo response did not contain a model list".to_string())?;
    let mut names = models
        .iter()
        .filter_map(|model| {
            let id = model.get("id")?.as_str()?;
            (model.get("isFree")?.as_bool()? || id == "kilo-auto/free").then(|| id.to_string())
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    if !names.iter().any(|name| name == "kilo-auto/free") {
        names.insert(0, "kilo-auto/free".to_string());
    }
    Ok(names)
}

#[tauri::command]
pub async fn list_provider_models(
    provider: String,
    endpoint: Option<String>,
    allow_lan_http: Option<bool>,
    api_key_override: Option<String>,
    local_port: Option<u16>,
) -> Result<Vec<String>, String> {
    if provider == "kilo" {
        return list_kilo_free_models().await;
    }
    let completion_url = if provider == "custom" || provider == "ollama" || provider == "lmstudio" {
        custom_completion_url_with_lan(
            endpoint.as_deref().unwrap_or(if provider == "lmstudio" {
                "http://localhost:1234/v1"
            } else if provider == "ollama" {
                "http://localhost:11434/v1"
            } else {
                ""
            }),
            allow_lan_http.unwrap_or(false),
        )?
    } else {
        endpoint_for(&provider, endpoint.as_deref(), local_port)?
    };
    let url = models_url(&completion_url)?;
    let mut request = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(8))
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|error| error.to_string())?
        .get(url);
    let key = if provider == "local" || provider == "ollama" || provider == "lmstudio" {
        None
    } else {
        configured_or_override_key(&provider, api_key_override)
    };
    if let Some(key) = key.filter(|key| !key.trim().is_empty()) {
        let value =
            HeaderValue::from_str(&format!("Bearer {key}")).map_err(|error| error.to_string())?;
        request = request.header(AUTHORIZATION, value);
    }
    let response = request
        .send()
        .await
        .map_err(|error| format!("Could not connect to model provider: {error}"))?
        .error_for_status()
        .map_err(|error| format!("Model provider rejected the request: {error}"))?;
    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|error| format!("Provider returned an invalid model list: {error}"))?;
    let models = payload
        .get("data")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Provider response did not contain a 'data' model list".to_string())?;
    let names = models
        .iter()
        .filter_map(|model| model.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err("Provider returned no selectable models".to_string());
    }
    Ok(names)
}

fn emit(app: &AppHandle, request_id: u64, delta: String, done: bool, error: Option<String>) {
    let _ = app.emit(
        STREAM_EVENT,
        GatewayEvent {
            request_id,
            delta,
            done,
            error,
        },
    );
}

fn custom_completion_url(endpoint: &str) -> Result<String, String> {
    custom_completion_url_with_lan(endpoint, false)
}

fn custom_completion_url_with_lan(endpoint: &str, allow_lan_http: bool) -> Result<String, String> {
    let endpoint = endpoint.trim().trim_end_matches('/');
    let parsed =
        reqwest::Url::parse(endpoint).map_err(|_| "Enter a valid provider URL".to_string())?;
    let host = parsed.host_str().unwrap_or_default();
    let loopback = host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "::1";
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && (loopback || allow_lan_http)) {
        return Err(
            "Provider URL must use HTTPS, or explicitly allow HTTP for a trusted local network"
                .to_string(),
        );
    }
    let path = parsed.path().trim_end_matches('/');
    if path.ends_with("/chat/completions") {
        Ok(endpoint.to_string())
    } else if path.ends_with("/v1") {
        Ok(format!("{endpoint}/chat/completions"))
    } else {
        Ok(format!("{endpoint}/v1/chat/completions"))
    }
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri maps command payload fields to this function's parameters.
pub fn start_chat_completion(
    app: AppHandle,
    state: State<'_, GatewayState>,
    provider: String,
    model: String,
    endpoint: Option<String>,
    allow_lan_http: Option<bool>,
    api_key_override: Option<String>,
    local_port: Option<u16>,
    messages: Vec<GatewayMessage>,
) -> Result<u64, String> {
    if messages.is_empty() {
        return Err("At least one chat message is required".to_string());
    }
    let url = if provider == "kilo" {
        endpoint_for(&provider, endpoint.as_deref(), local_port)?
    } else if provider == "custom" || provider == "ollama" || provider == "lmstudio" {
        custom_completion_url_with_lan(
            endpoint.as_deref().unwrap_or(if provider == "lmstudio" {
                "http://localhost:1234/v1"
            } else if provider == "ollama" {
                "http://localhost:11434/v1"
            } else {
                ""
            }),
            allow_lan_http.unwrap_or(false),
        )?
    } else {
        endpoint_for(&provider, endpoint.as_deref(), local_port)?
    };
    let api_key = if provider == "local"
        || provider == "ollama"
        || provider == "lmstudio"
        || provider == "kilo"
    {
        None
    } else if provider == "custom" {
        configured_or_override_key(&provider, api_key_override)
    } else {
        Some(
            provider_key(&provider)?
                .get_password()
                .map_err(|_| format!("No API key saved for {provider}"))?,
        )
    };
    let request_id = state.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let cancelled = Arc::new(AtomicBool::new(false));
    state
        .tasks
        .lock()
        .map_err(|error| error.to_string())?
        .insert(request_id, cancelled.clone());
    let tasks = Arc::clone(&state.tasks);
    tauri::async_runtime::spawn(async move {
        stream_completion(
            app.clone(),
            request_id,
            url,
            model,
            messages,
            api_key,
            cancelled,
        )
        .await;
        if let Ok(mut tasks) = tasks.lock() {
            tasks.remove(&request_id);
        }
    });
    Ok(request_id)
}

#[tauri::command]
pub fn cancel_chat_completion(
    state: State<'_, GatewayState>,
    request_id: u64,
) -> Result<(), String> {
    let tasks = state.tasks.lock().map_err(|error| error.to_string())?;
    let cancelled = tasks
        .get(&request_id)
        .ok_or_else(|| "Generation request not found".to_string())?;
    cancelled.store(true, Ordering::Relaxed);
    Ok(())
}

async fn stream_completion(
    app: AppHandle,
    request_id: u64,
    url: String,
    model: String,
    messages: Vec<GatewayMessage>,
    api_key: Option<String>,
    cancelled: Arc<AtomicBool>,
) {
    let client = match reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            emit(
                &app,
                request_id,
                String::new(),
                true,
                Some(error.to_string()),
            );
            return;
        }
    };
    let mut request = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .json(&serde_json::json!({
          "model": if model.trim().is_empty() { "local-model" } else { model.trim() },
          "messages": messages,
          "stream": true
        }));
    if let Some(api_key) = api_key.filter(|key| !key.trim().is_empty()) {
        match HeaderValue::from_str(&format!("Bearer {api_key}")) {
            Ok(value) => request = request.header(AUTHORIZATION, value),
            Err(error) => {
                emit(
                    &app,
                    request_id,
                    String::new(),
                    true,
                    Some(error.to_string()),
                );
                return;
            }
        }
    }
    let response = match request.send().await {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status();
            let detail = response.text().await.unwrap_or_default();
            emit(
                &app,
                request_id,
                String::new(),
                true,
                Some(format!("Provider returned {status}: {detail}")),
            );
            return;
        }
        Err(error) => {
            emit(
                &app,
                request_id,
                String::new(),
                true,
                Some(error.to_string()),
            );
            return;
        }
    };

    let mut stream = response.bytes_stream();
    let mut line_buffer = Vec::new();
    while let Some(chunk) = stream.next().await {
        if cancelled.load(Ordering::Relaxed) {
            emit(&app, request_id, String::new(), true, None);
            return;
        }
        let chunk = match chunk {
            Ok(chunk) => chunk,
            Err(error) => {
                emit(
                    &app,
                    request_id,
                    String::new(),
                    true,
                    Some(error.to_string()),
                );
                return;
            }
        };
        line_buffer.extend_from_slice(&chunk);
        while let Some(newline) = line_buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = line_buffer.drain(..=newline).collect();
            if consume_sse_line(&app, request_id, &line) {
                return;
            }
        }
    }
    if !line_buffer.is_empty() && consume_sse_line(&app, request_id, &line_buffer) {
        return;
    }
    emit(&app, request_id, String::new(), true, None);
}

fn consume_sse_line(app: &AppHandle, request_id: u64, bytes: &[u8]) -> bool {
    let line = String::from_utf8_lossy(bytes).trim().to_string();
    let Some(data) = line.strip_prefix("data:").map(str::trim) else {
        return false;
    };
    if data == "[DONE]" {
        emit(app, request_id, String::new(), true, None);
        return true;
    }
    let Ok(payload) = serde_json::from_str::<serde_json::Value>(data) else {
        return false;
    };
    if let Some(error) = payload.get("error") {
        emit(
            app,
            request_id,
            String::new(),
            true,
            Some(error.to_string()),
        );
        return true;
    }
    if let Some(delta) = payload
        .pointer("/choices/0/delta/content")
        .and_then(serde_json::Value::as_str)
    {
        emit(app, request_id, delta.to_string(), false, None);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{custom_completion_url_with_lan, endpoint_for, models_url};

    #[test]
    fn routes_local_and_known_remote_providers() {
        assert_eq!(
            endpoint_for("local", None, Some(8080)).unwrap(),
            "http://127.0.0.1:8080/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for("openai", None, None).unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for("openrouter", None, None).unwrap(),
            "https://openrouter.ai/api/v1/chat/completions"
        );
    }

    #[test]
    fn custom_endpoint_requires_tls_or_loopback() {
        assert!(endpoint_for("custom", Some("http://192.168.1.4/v1"), None).is_err());
        assert_eq!(
            endpoint_for("custom", Some("https://api.example/v1"), None).unwrap(),
            "https://api.example/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for("custom", Some("http://localhost:11434/v1"), None).unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            endpoint_for("custom", Some("https://api.example"), None).unwrap(),
            "https://api.example/v1/chat/completions"
        );
        assert!(custom_completion_url_with_lan("http://192.168.1.4:8080/v1", false).is_err());
        assert_eq!(
            custom_completion_url_with_lan("http://192.168.1.4:8080/v1", true).unwrap(),
            "http://192.168.1.4:8080/v1/chat/completions"
        );
    }

    #[test]
    fn supports_local_ollama_and_model_enumeration_urls() {
        assert_eq!(
            endpoint_for("ollama", None, None).unwrap(),
            "http://localhost:11434/v1/chat/completions"
        );
        assert_eq!(
            models_url("http://localhost:11434/v1/chat/completions").unwrap(),
            "http://localhost:11434/v1/models"
        );
    }
}
