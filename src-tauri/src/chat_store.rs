use std::fs::{self, File};
use std::io::Write;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

#[derive(Default)]
pub struct ChatStoreState {
    lock: Mutex<()>,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    pub id: String,
    pub role: String,
    pub content: String,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct StoredChat {
    pub id: String,
    pub title: String,
    pub updated_at: u64,
    pub messages: Vec<StoredMessage>,
}

fn chat_file(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    app.path()
        .app_data_dir()
        .map(|path| path.join("chats.json"))
        .map_err(|error| error.to_string())
}

fn read_chats(path: &std::path::Path) -> Result<Vec<StoredChat>, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn write_chats(path: &std::path::Path, chats: &[StoredChat]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(chats).map_err(|error| error.to_string())?;
    let temp_path = path.with_extension("json.tmp");
    let mut file = File::create(&temp_path).map_err(|error| error.to_string())?;
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::rename(&temp_path, path).map_err(|error| error.to_string())
}

fn validate_chat(chat: &StoredChat) -> Result<(), String> {
    if chat.id.is_empty() || chat.id.len() > 128 {
        return Err("Chat id is invalid".to_string());
    }
    if chat.messages.iter().any(|message| {
        message.id.is_empty() || !matches!(message.role.as_str(), "system" | "user" | "assistant")
    }) {
        return Err("Chat contains a message with an invalid id or role".to_string());
    }
    Ok(())
}

#[tauri::command]
pub fn list_chats(
    app: AppHandle,
    state: State<'_, ChatStoreState>,
) -> Result<Vec<StoredChat>, String> {
    let _guard = state.lock.lock().map_err(|error| error.to_string())?;
    let mut chats = read_chats(&chat_file(&app)?)?;
    chats.sort_by_key(|chat| std::cmp::Reverse(chat.updated_at));
    Ok(chats)
}

#[tauri::command]
pub fn save_chat(
    app: AppHandle,
    state: State<'_, ChatStoreState>,
    mut chat: StoredChat,
) -> Result<StoredChat, String> {
    validate_chat(&chat)?;
    chat.updated_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_millis() as u64;
    let _guard = state.lock.lock().map_err(|error| error.to_string())?;
    let path = chat_file(&app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut chats = read_chats(&path)?;
    let saved_chat = chat.clone();
    match chats.iter_mut().find(|stored| stored.id == chat.id) {
        Some(stored) => *stored = chat,
        None => chats.push(chat),
    }
    write_chats(&path, &chats)?;
    Ok(saved_chat)
}

#[tauri::command]
pub fn delete_chat(
    app: AppHandle,
    state: State<'_, ChatStoreState>,
    chat_id: String,
) -> Result<(), String> {
    let _guard = state.lock.lock().map_err(|error| error.to_string())?;
    let path = chat_file(&app)?;
    let mut chats = read_chats(&path)?;
    chats.retain(|chat| chat.id != chat_id);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    write_chats(&path, &chats)
}

#[cfg(test)]
mod tests {
    use super::{StoredChat, StoredMessage, validate_chat};

    #[test]
    fn chat_accepts_user_and_assistant_messages() {
        let chat = StoredChat {
            id: "chat-1".to_string(),
            title: "Test".to_string(),
            updated_at: 1,
            messages: vec![StoredMessage {
                id: "message-1".to_string(),
                role: "user".to_string(),
                content: "Hello".to_string(),
            }],
        };
        assert!(validate_chat(&chat).is_ok());
    }

    #[test]
    fn chat_rejects_unknown_message_roles() {
        let chat = StoredChat {
            id: "chat-1".to_string(),
            title: "Test".to_string(),
            updated_at: 1,
            messages: vec![StoredMessage {
                id: "message-1".to_string(),
                role: "tool".to_string(),
                content: "ignored".to_string(),
            }],
        };
        assert!(validate_chat(&chat).is_err());
    }
}
