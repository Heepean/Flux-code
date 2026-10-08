use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

struct LanguageServerSession {
    writer: Arc<Mutex<ChildStdin>>,
    child: Arc<Mutex<Child>>,
}

#[derive(Default)]
pub struct LanguageServerState {
    sessions: Arc<Mutex<HashMap<String, LanguageServerSession>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerMessage {
    session_id: String,
    message: String,
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, String> {
    let mut content_length = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(None);
        }
        if line == b"\r\n" || line == b"\n" {
            break;
        }
        if let Some(separator) = line.iter().position(|byte| *byte == b':')
            && line[..separator].eq_ignore_ascii_case(b"content-length")
        {
            content_length = std::str::from_utf8(&line[separator + 1..])
                .ok()
                .and_then(|value| value.trim().parse::<usize>().ok());
        }
    }
    let length = content_length
        .ok_or_else(|| "Language server sent a frame without Content-Length".to_string())?;
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .map_err(|error| format!("Could not read language server message: {error}"))?;
    Ok(Some(body))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerExit {
    session_id: String,
    detail: String,
}

#[tauri::command]
pub fn start_language_server(
    app: AppHandle,
    state: State<'_, LanguageServerState>,
    session_id: String,
    command: String,
    args: Vec<String>,
    project_path: String,
) -> Result<(), String> {
    if session_id.trim().is_empty() || command.trim().is_empty() {
        return Err("A session id and language server command are required".to_string());
    }
    let root = std::path::Path::new(&project_path)
        .canonicalize()
        .map_err(|error| format!("Could not open language server workspace: {error}"))?;
    if !root.is_dir() {
        return Err("Language server workspace must be a folder".to_string());
    }
    if state
        .sessions
        .lock()
        .map_err(|_| "Language server state is unavailable".to_string())?
        .contains_key(&session_id)
    {
        return Err("A language server is already running for this session".to_string());
    }
    let mut child = Command::new(&command)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("Could not start {command}: {error}"))?;
    let writer = child
        .stdin
        .take()
        .ok_or_else(|| "Could not open language server input".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read language server output".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Could not read language server log".to_string())?;
    let child = Arc::new(Mutex::new(child));
    let writer = Arc::new(Mutex::new(writer));
    let sessions = Arc::clone(&state.sessions);
    let app_output = app.clone();
    let output_id = session_id.clone();
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut detail = String::new();
        loop {
            match read_message(&mut reader) {
                Ok(Some(bytes)) => {
                    let message = String::from_utf8_lossy(&bytes).into_owned();
                    let _ = app_output.emit(
                        "language-server-message",
                        ServerMessage {
                            session_id: output_id.clone(),
                            message,
                        },
                    );
                }
                Ok(None) => break,
                Err(error) => {
                    detail = error;
                    break;
                }
            }
        }
        if let Ok(mut sessions) = sessions.lock() {
            sessions.remove(&output_id);
        }
        let _ = app_output.emit(
            "language-server-exit",
            ServerExit {
                session_id: output_id,
                detail,
            },
        );
    });
    let log_app = app.clone();
    let log_id = session_id.clone();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = log_app.emit(
                "language-server-log",
                ServerMessage {
                    session_id: log_id.clone(),
                    message: line,
                },
            );
        }
    });
    state
        .sessions
        .lock()
        .map_err(|_| "Language server state is unavailable".to_string())?
        .insert(session_id, LanguageServerSession { writer, child });
    Ok(())
}

#[tauri::command]
pub fn send_language_server_message(
    state: State<'_, LanguageServerState>,
    session_id: String,
    message: String,
) -> Result<(), String> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "Language server state is unavailable".to_string())?;
    let session = sessions
        .get(&session_id)
        .ok_or_else(|| "Language server session is not running".to_string())?;
    let mut writer = session
        .writer
        .lock()
        .map_err(|_| "Language server input is unavailable".to_string())?;
    write!(writer, "Content-Length: {}\r\n\r\n", message.len())
        .map_err(|error| error.to_string())?;
    writer
        .write_all(message.as_bytes())
        .map_err(|error| format!("Could not send language server message: {error}"))?;
    writer.flush().map_err(|error| error.to_string())
}

#[tauri::command]
pub fn stop_language_server(
    state: State<'_, LanguageServerState>,
    session_id: String,
) -> Result<(), String> {
    let session = state
        .sessions
        .lock()
        .map_err(|_| "Language server state is unavailable".to_string())?
        .remove(&session_id);
    if let Some(session) = session {
        session
            .child
            .lock()
            .map_err(|_| "Language server process is unavailable".to_string())?
            .kill()
            .map_err(|error| format!("Could not stop language server: {error}"))?;
    }
    Ok(())
}
