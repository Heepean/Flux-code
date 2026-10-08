use std::fs;
use std::path::{Path, PathBuf};
mod agent_tools;
mod chat_store;
mod git;
mod hf_loader;
mod language_server;
mod llama_manager;
mod model_gateway;
mod terminal;
mod workspace;

#[derive(serde::Serialize)]
pub struct LocalModelStatus {
    app_data_dir: String,
    llama_server_path: Option<String>,
    gguf_files: Vec<String>,
    model_dir_exists: bool,
}

fn find_files(path: &Path, predicate: impl Fn(&Path) -> bool, limit: usize) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(path) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|candidate| candidate.is_file() && predicate(candidate))
        .take(limit)
        .collect()
}

mod model_status {
    use super::{LocalModelStatus, find_files};
    use std::fs;
    use std::path::Path;
    use tauri::Manager;

    #[tauri::command]
    pub fn get_local_model_status(app: tauri::AppHandle) -> Result<LocalModelStatus, String> {
        let flux_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| error.to_string())?;
        let bin_dir = flux_dir.join("bin");
        let model_dir = flux_dir.join("models");

        let llama_server = find_files(
            &bin_dir,
            |path: &Path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("llama-server.exe"))
            },
            10,
        );

        let mut gguf_files: Vec<String> = find_files(
            &model_dir,
            |path: &Path| {
                path.extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
            },
            20,
        )
        .into_iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect();
        let registry_file = flux_dir.join("model-registry.json");
        if let Ok(bytes) = fs::read(registry_file)
            && let Ok(registry) = serde_json::from_slice::<serde_json::Value>(&bytes)
            && let Some(paths) = registry
                .get("external_paths")
                .and_then(serde_json::Value::as_array)
        {
            for path in paths.iter().filter_map(serde_json::Value::as_str) {
                let model = Path::new(path);
                if gguf_files.len() >= 20 {
                    break;
                }
                if model.is_file()
                    && model
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("gguf"))
                {
                    let value = model.to_string_lossy().to_string();
                    if !gguf_files.contains(&value) {
                        gguf_files.push(value);
                    }
                }
            }
        }

        Ok(LocalModelStatus {
            app_data_dir: flux_dir.to_string_lossy().to_string(),
            llama_server_path: llama_server
                .into_iter()
                .next()
                .map(|p| p.to_string_lossy().to_string()),
            gguf_files,
            model_dir_exists: model_dir.exists(),
        })
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let application = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(llama_manager::RuntimeState::default())
        .manage(hf_loader::DownloadState::default())
        .manage(model_gateway::GatewayState::default())
        .manage(chat_store::ChatStoreState::default())
        .manage(terminal::TerminalState::default())
        .manage(language_server::LanguageServerState::default())
        .invoke_handler(tauri::generate_handler![
            agent_tools::execute_agent_action,
            workspace::list_workspace_directory,
            workspace::read_workspace_file,
            workspace::write_workspace_file,
            workspace::create_workspace_entry,
            workspace::rename_workspace_entry,
            workspace::delete_workspace_entry,
            workspace::search_workspace,
            terminal::create_terminal,
            terminal::write_terminal_input,
            terminal::resize_terminal,
            terminal::close_terminal,
            language_server::start_language_server,
            language_server::send_language_server_message,
            language_server::stop_language_server,
            git::git_status,
            git::git_diff,
            git::git_stage,
            git::git_commit,
            model_status::get_local_model_status,
            llama_manager::get_llama_manager_status,
            llama_manager::install_llama,
            llama_manager::uninstall_llama,
            llama_manager::get_machine_profile,
            llama_manager::estimate_model_memory,
            llama_manager::start_llama_server,
            llama_manager::stop_llama_server,
            llama_manager::get_model_capabilities,
            hf_loader::search_hf_models,
            hf_loader::list_hf_gguf_files,
            hf_loader::save_hf_token,
            hf_loader::delete_hf_token,
            hf_loader::get_hf_token_status,
            hf_loader::start_hf_download,
            hf_loader::set_hf_download_paused,
            hf_loader::cancel_hf_download,
            hf_loader::list_local_models,
            hf_loader::delete_local_model,
            hf_loader::import_local_gguf,
            hf_loader::find_gguf_in_folder,
            hf_loader::open_models_folder,
            model_gateway::save_provider_key,
            model_gateway::delete_provider_key,
            model_gateway::provider_key_configured,
            model_gateway::list_provider_models,
            model_gateway::start_chat_completion,
            model_gateway::cancel_chat_completion,
            chat_store::list_chats,
            chat_store::save_chat,
            chat_store::delete_chat
        ])
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .build(tauri::generate_context!());
    let application = match application {
        Ok(application) => application,
        Err(error) => {
            eprintln!("Flux Code could not start: {error}");
            return;
        }
    };
    application.run(|app, event| {
        if let tauri::RunEvent::Exit = event {
            llama_manager::stop_on_exit(app);
        }
    });
}
