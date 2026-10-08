use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

struct TerminalSession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Arc<Mutex<Box<dyn portable_pty::Child + Send>>>,
}

#[derive(Default)]
pub struct TerminalState {
    sessions: Arc<Mutex<HashMap<String, TerminalSession>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalOutput {
    terminal_id: String,
    data: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TerminalExit {
    terminal_id: String,
    code: Option<u32>,
}

#[tauri::command]
pub fn create_terminal(
    app: AppHandle,
    state: State<'_, TerminalState>,
    project_path: String,
    cols: u16,
    rows: u16,
) -> Result<String, String> {
    let cwd = Path::new(&project_path)
        .canonicalize()
        .map_err(|error| format!("Could not open terminal in project folder: {error}"))?;
    if !cwd.is_dir() {
        return Err("The terminal working directory is not a folder".to_string());
    }

    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: rows.clamp(1, 300),
            cols: cols.clamp(1, 500),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Could not create terminal: {error}"))?;

    let mut command = if cfg!(windows) {
        CommandBuilder::new("powershell.exe")
    } else {
        CommandBuilder::new("/bin/sh")
    };
    if cfg!(windows) {
        command.args(["-NoLogo", "-NoExit"]);
    }
    command.cwd(cwd);
    let child: Box<dyn portable_pty::Child + Send> = pair
        .slave
        .spawn_command(command)
        .map_err(|error| format!("Could not start terminal shell: {error}"))?;
    drop(pair.slave);

    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("Could not read terminal output: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("Could not write to terminal: {error}"))?;
    let terminal_id = uuid::Uuid::new_v4().to_string();
    let sessions = Arc::clone(&state.sessions);
    let output_app = app.clone();
    let output_id = terminal_id.clone();
    let child = Arc::new(Mutex::new(child));
    let waiting_child = Arc::clone(&child);

    std::thread::spawn(move || {
        let mut buffer = [0_u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(length) => {
                    let data = String::from_utf8_lossy(&buffer[..length]).into_owned();
                    let _ = output_app.emit(
                        "terminal-output",
                        TerminalOutput {
                            terminal_id: output_id.clone(),
                            data,
                        },
                    );
                }
            }
        }
        let code = waiting_child
            .lock()
            .ok()
            .and_then(|mut child| child.wait().ok())
            .map(|status| status.exit_code());
        if let Ok(mut sessions) = sessions.lock() {
            sessions.remove(&output_id);
        }
        let _ = output_app.emit(
            "terminal-exit",
            TerminalExit {
                terminal_id: output_id,
                code,
            },
        );
    });

    state
        .sessions
        .lock()
        .map_err(|_| "Terminal state is unavailable".to_string())?
        .insert(
            terminal_id.clone(),
            TerminalSession {
                master: pair.master,
                writer,
                child,
            },
        );
    Ok(terminal_id)
}

#[tauri::command]
pub fn write_terminal_input(
    state: State<'_, TerminalState>,
    terminal_id: String,
    data: String,
) -> Result<(), String> {
    let mut sessions = state
        .sessions
        .lock()
        .map_err(|_| "Terminal state is unavailable".to_string())?;
    let session = sessions
        .get_mut(&terminal_id)
        .ok_or_else(|| "Terminal session is no longer available".to_string())?;
    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|error| format!("Could not write to terminal: {error}"))
}

#[tauri::command]
pub fn resize_terminal(
    state: State<'_, TerminalState>,
    terminal_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let sessions = state
        .sessions
        .lock()
        .map_err(|_| "Terminal state is unavailable".to_string())?;
    let session = sessions
        .get(&terminal_id)
        .ok_or_else(|| "Terminal session is no longer available".to_string())?;
    session
        .master
        .resize(PtySize {
            rows: rows.clamp(1, 300),
            cols: cols.clamp(1, 500),
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| format!("Could not resize terminal: {error}"))
}

#[tauri::command]
pub fn close_terminal(state: State<'_, TerminalState>, terminal_id: String) -> Result<(), String> {
    let mut session = state
        .sessions
        .lock()
        .map_err(|_| "Terminal state is unavailable".to_string())?
        .remove(&terminal_id);
    if let Some(session) = session.as_mut() {
        session
            .child
            .lock()
            .map_err(|_| "Terminal process is unavailable".to_string())?
            .kill()
            .map_err(|error| format!("Could not stop terminal: {error}"))?;
    }
    Ok(())
}
