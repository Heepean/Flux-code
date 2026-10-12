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

#[derive(Default)]
pub struct WorkspaceState {
    root: Mutex<Option<PathBuf>>,
}

impl WorkspaceState {
    pub(crate) fn active_root(&self) -> Result<PathBuf, String> {
        self.root
            .lock()
            .map_err(|error| error.to_string())?
            .clone()
            .ok_or_else(|| "Сначала откройте папку проекта".into())
    }

    fn set_root(&self, root: PathBuf) -> Result<(), String> {
        *self.root.lock().map_err(|error| error.to_string())? = Some(root);
        Ok(())
    }
}

pub(crate) fn terminate_process_tree(child: &mut Child) -> Result<(), String> {
    if child
        .try_wait()
        .map_err(|error| format!("Не удалось проверить процесс {}: {error}", child.id()))?
        .is_some()
    {
        return Ok(());
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("taskkill.exe");
        command
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .creation_flags(0x08000000);
        let status = command
            .status()
            .map_err(|error| format!("Не удалось остановить дерево процессов: {error}"))?;
        if !status.success()
            && child
                .try_wait()
                .map_err(|error| error.to_string())?
                .is_none()
        {
            return Err(format!(
                "Windows не остановила процесс {} и его потомков",
                child.id()
            ));
        }
    }

    #[cfg(not(windows))]
    if let Err(error) = child.kill() {
        if child
            .try_wait()
            .map_err(|wait_error| wait_error.to_string())?
            .is_none()
        {
            return Err(format!("Не удалось остановить процесс: {error}"));
        }
    }

    child
        .wait()
        .map_err(|error| format!("Не удалось дождаться остановки процесса: {error}"))?;
    Ok(())
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

#[derive(Serialize)]
struct SearchResult {
    path: String,
    line: usize,
    preview: String,
}

#[derive(Serialize)]
struct GitChange {
    code: String,
    path: String,
}

#[derive(Serialize)]
struct GitStatus {
    branch: String,
    changes: Vec<GitChange>,
}

fn ignored_search_directory(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".venv"
            | "venv"
            | "vendor"
            | ".next"
            | ".cache"
    )
}

fn matching_lines(text: &str, query: &str, path: &str, limit: usize) -> Vec<SearchResult> {
    let needle = query.to_lowercase();
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            if line.to_lowercase().contains(&needle) {
                let line = line.trim();
                let preview = if line.chars().count() > 500 {
                    format!("{}…", line.chars().take(499).collect::<String>())
                } else {
                    line.to_owned()
                };
                Some(SearchResult {
                    path: path.to_owned(),
                    line: index + 1,
                    preview,
                })
            } else {
                None
            }
        })
        .take(limit)
        .collect()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
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
fn browse_directory(
    app: AppHandle,
    workspace: State<'_, WorkspaceState>,
) -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, workspace);
        return Err(
            "Выбор папки доступен на desktop; на Android укажите workspace на удалённом Agent Server"
                .into(),
        );
    }
    #[cfg(not(target_os = "android"))]
    {
        let selected = rfd::FileDialog::new()
            .pick_folder()
            .map(|path| activate_workspace(&app, &workspace, path))
            .transpose()?;
        Ok(selected.map(|path| path.to_string_lossy().into_owned()))
    }
}

#[tauri::command]
fn create_project_directory(
    app: AppHandle,
    workspace: State<'_, WorkspaceState>,
    parent: String,
    name: String,
) -> Result<String, String> {
    #[cfg(target_os = "android")]
    {
        let _ = (app, workspace, parent, name);
        return Err("Создание папок проектов доступно в desktop-версии".into());
    }
    #[cfg(not(target_os = "android"))]
    {
        let path = create_project_path(Path::new(&parent), &name)?;
        workspace.set_root(path.clone())?;
        persist_workspace(&app, &path)?;
        Ok(path.to_string_lossy().into_owned())
    }
}

fn create_project_path(parent: &Path, name: &str) -> Result<PathBuf, String> {
    let parent = fs::canonicalize(parent)
        .map_err(|error| format!("Не удалось открыть родительскую папку: {error}"))?;
    if !parent.is_dir() {
        return Err("Родительский путь не является папкой".into());
    }
    let name = name.trim();
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.chars().any(|ch| {
            matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || ch.is_control()
        })
    {
        return Err("Имя проекта не должно содержать символы / \\: * ? \" < > |".into());
    }
    let path = parent.join(name);
    fs::create_dir(&path).map_err(|error| format!("Не удалось создать папку проекта: {error}"))?;
    fs::canonicalize(path).map_err(|error| error.to_string())
}

#[tauri::command]
fn browse_project_parent() -> Result<Option<String>, String> {
    #[cfg(target_os = "android")]
    return Err("Создание папок проектов доступно в desktop-версии".into());
    #[cfg(not(target_os = "android"))]
    Ok(rfd::FileDialog::new()
        .set_title("Выберите место для нового проекта")
        .pick_folder()
        .map(|path| path.to_string_lossy().into_owned()))
}

#[tauri::command]
fn activate_project(
    app: AppHandle,
    workspace: State<'_, WorkspaceState>,
    path: String,
) -> Result<String, String> {
    let root = activate_workspace(&app, &workspace, PathBuf::from(path))?;
    Ok(root.to_string_lossy().into_owned())
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
fn get_workspace_root(
    app: AppHandle,
    workspace: State<'_, WorkspaceState>,
    legacy_path: Option<String>,
) -> Result<Option<String>, String> {
    if let Ok(root) = workspace.active_root() {
        return Ok(Some(root.to_string_lossy().into_owned()));
    }
    let config = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?
        .join("active-workspace.txt");
    let saved = match fs::read_to_string(&config) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let Some(legacy_path) = legacy_path else {
                return Ok(None);
            };
            // One-time migration from the workspace path that earlier Flux Hands
            // versions stored in localStorage. Once migrated, native state owns it.
            let root = match fs::canonicalize(legacy_path.trim()) {
                Ok(root) if root.is_dir() => root,
                _ => return Ok(None),
            };
            workspace.set_root(root.clone())?;
            if let Some(directory) = config.parent() {
                fs::create_dir_all(directory).map_err(|error| error.to_string())?;
            }
            fs::write(config, root.to_string_lossy().as_bytes())
                .map_err(|error| format!("Не удалось перенести папку проекта: {error}"))?;
            return Ok(Some(root.to_string_lossy().into_owned()));
        }
        Err(error) => return Err(format!("Не удалось прочитать настройки проекта: {error}")),
    };
    let root = match fs::canonicalize(saved.trim()) {
        Ok(root) if root.is_dir() => root,
        _ => return Ok(None),
    };
    workspace.set_root(root.clone())?;
    Ok(Some(root.to_string_lossy().into_owned()))
}

#[tauri::command]
fn default_workspace(
    app: AppHandle,
    workspace: State<'_, WorkspaceState>,
) -> Result<String, String> {
    if let Ok(root) = workspace.active_root() {
        return Ok(root.to_string_lossy().into_owned());
    }
    let path = app
        .path()
        .document_dir()
        .map_err(|error| error.to_string())?
        .join("Flux Hands Workspace");
    fs::create_dir_all(&path).map_err(|error| error.to_string())?;
    let root = fs::canonicalize(path).map_err(|error| error.to_string())?;
    workspace.set_root(root.clone())?;
    persist_workspace(&app, &root)?;
    Ok(root.to_string_lossy().into_owned())
}

fn activate_workspace(
    app: &AppHandle,
    workspace: &WorkspaceState,
    path: PathBuf,
) -> Result<PathBuf, String> {
    let root = fs::canonicalize(path)
        .map_err(|error| format!("Не удалось открыть папку проекта: {error}"))?;
    if !root.is_dir() {
        return Err("Выбранный путь не является папкой".into());
    }
    workspace.set_root(root.clone())?;
    persist_workspace(app, &root)?;
    Ok(root)
}

fn persist_workspace(app: &AppHandle, root: &Path) -> Result<(), String> {
    let directory = app
        .path()
        .app_config_dir()
        .map_err(|error| error.to_string())?;
    fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    fs::write(
        directory.join("active-workspace.txt"),
        root.to_string_lossy().as_bytes(),
    )
    .map_err(|error| format!("Не удалось сохранить папку проекта: {error}"))
}

fn checked_workspace_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    let candidate = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        root.join(candidate)
    };
    let canonical_candidate = fs::canonicalize(&candidate)
        .map_err(|error| format!("Путь проекта недоступен: {error}"))?;
    if !canonical_candidate.starts_with(root) {
        return Err("Доступ за пределы открытого проекта запрещён".into());
    }
    Ok(canonical_candidate)
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
fn scan_models(workspace: State<'_, WorkspaceState>) -> Result<Vec<Entry>, String> {
    let path = workspace.active_root()?;
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
fn list_directory(
    workspace: State<'_, WorkspaceState>,
    path: String,
) -> Result<Vec<Entry>, String> {
    let root = workspace.active_root()?;
    let path = checked_workspace_path(&root, &path)?;
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
fn search_workspace(
    workspace: State<'_, WorkspaceState>,
    query: String,
) -> Result<Vec<SearchResult>, String> {
    let query = validate_search_query(&query)?;
    search_workspace_files(&workspace.active_root()?, query)
}

fn validate_search_query(query: &str) -> Result<&str, String> {
    let query = query.trim();
    if query.is_empty() || query.chars().count() > 256 {
        return Err("Введите поисковый текст длиной до 256 символов".into());
    }
    Ok(query)
}

fn search_workspace_files(path: &Path, query: &str) -> Result<Vec<SearchResult>, String> {
    let root =
        fs::canonicalize(path).map_err(|error| format!("Не удалось открыть проект: {error}"))?;
    if !root.is_dir() {
        return Err("Папка проекта недоступна".into());
    }
    let mut directories = vec![(root.clone(), 0usize)];
    let mut results = Vec::new();
    let mut visited_files = 0usize;
    while let Some((directory, depth)) = directories.pop() {
        if depth > 24 || results.len() >= 300 || visited_files >= 15_000 {
            break;
        }
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for item in entries.flatten() {
            if results.len() >= 300 || visited_files >= 15_000 {
                break;
            }
            let item_path = item.path();
            let file_type = match item.file_type() {
                Ok(file_type) if !file_type.is_symlink() => file_type,
                _ => continue,
            };
            if file_type.is_dir() {
                if !ignored_search_directory(&item.file_name().to_string_lossy()) {
                    directories.push((item_path, depth + 1));
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            visited_files += 1;
            let metadata = match item.metadata() {
                Ok(metadata) if metadata.len() <= 2 * 1024 * 1024 => metadata,
                _ => continue,
            };
            if metadata.len() == 0 {
                continue;
            }
            let bytes = match fs::read(&item_path) {
                Ok(bytes) => bytes,
                Err(_) => continue,
            };
            let text = match std::str::from_utf8(&bytes) {
                Ok(text) => text,
                Err(_) => continue,
            };
            let absolute_path = item_path.to_string_lossy().into_owned();
            let mut matches = matching_lines(text, query, &absolute_path, 300 - results.len());
            results.append(&mut matches);
        }
    }
    Ok(results)
}

fn git_output(path: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(path).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    command
        .output()
        .map_err(|error| format!("Не удалось запустить Git: {error}"))
}

#[tauri::command]
fn git_status(workspace: State<'_, WorkspaceState>) -> Result<GitStatus, String> {
    git_status_for(&workspace.active_root()?)
}

fn git_status_for(path: &Path) -> Result<GitStatus, String> {
    let path = path.to_string_lossy().into_owned();
    if !Path::new(&path).is_dir() {
        return Err("Папка проекта недоступна".into());
    }
    let root = git_output(&path, &["rev-parse", "--show-toplevel"])?;
    if !root.status.success() {
        return Err("Открытая папка не является Git-репозиторием".into());
    }
    let branch_output = git_output(&path, &["symbolic-ref", "--short", "-q", "HEAD"])?;
    let branch = if branch_output.status.success() {
        String::from_utf8_lossy(&branch_output.stdout)
            .trim()
            .to_owned()
    } else {
        let commit = git_output(&path, &["rev-parse", "--short", "HEAD"])?;
        if !commit.status.success() {
            return Err("Не удалось определить текущую ветку или commit".into());
        }
        format!(
            "HEAD detached at {}",
            String::from_utf8_lossy(&commit.stdout).trim()
        )
    };
    let status = git_output(
        &path,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    if !status.status.success() {
        return Err(format!(
            "Git не смог прочитать состояние проекта: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    let mut changes = Vec::new();
    let mut entries = status.stdout.split(|byte| *byte == 0).peekable();
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&entry[..2]).to_string();
        let relative = String::from_utf8_lossy(&entry[3..]).into_owned();
        let full_path = Path::new(&path)
            .join(&relative)
            .to_string_lossy()
            .into_owned();
        changes.push(GitChange {
            code,
            path: full_path,
        });
        if entry[..2].contains(&b'R') || entry[..2].contains(&b'C') {
            let _ = entries.next();
        }
        if changes.len() >= 2000 {
            break;
        }
    }
    Ok(GitStatus { branch, changes })
}

#[derive(Serialize)]
struct GitFileDiff {
    path: String,
    diff: String,
    staged: bool,
}

#[tauri::command]
fn git_file_diff(
    workspace: State<'_, WorkspaceState>,
    path: String,
) -> Result<GitFileDiff, String> {
    git_file_diff_for(&workspace.active_root()?, &path)
}

fn git_file_diff_for(root: &Path, path: &str) -> Result<GitFileDiff, String> {
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let target = checked_workspace_path(&root, path)?;
    if !target.is_file() {
        return Err("Изменённый путь не является доступным файлом".into());
    }
    let relative = target
        .strip_prefix(&root)
        .map_err(|_| "Файл находится вне проекта")?;
    let relative = relative.to_str().ok_or("Путь файла не является UTF-8")?;
    let status = git_output(
        root.to_str().ok_or("Путь проекта не является UTF-8")?,
        &["status", "--porcelain=v1", "--", relative],
    )?;
    if !status.status.success() {
        return Err("Не удалось получить Git-статус файла".into());
    }
    let code = String::from_utf8_lossy(&status.stdout)
        .chars()
        .take(2)
        .collect::<String>();
    if code.is_empty() {
        return Err("Файл больше не содержит Git-изменений".into());
    }
    let staged = code
        .chars()
        .next()
        .is_some_and(|value| value != ' ' && value != '?');
    let diff = if code.starts_with("??") {
        let bytes = fs::read(&target).map_err(|error| error.to_string())?;
        if bytes.len() > 1_000_000 || bytes.contains(&0) {
            return Err(
                "Предпросмотр untracked доступен только для текстовых файлов до 1 МБ".into(),
            );
        }
        let content = String::from_utf8(bytes).map_err(|_| "Файл не является UTF-8 текстом")?;
        let mut diff = format!("--- /dev/null\n+++ b/{relative}\n");
        for line in content.lines() {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
        if content.is_empty() {
            diff.push_str("@@ пустой файл @@\n");
        }
        diff
    } else {
        let pathspec = format!(":(top,literal){relative}");
        let root = root.to_str().ok_or("Путь проекта не является UTF-8")?;
        let mut diff = String::new();
        if code.chars().any(|value| value != ' ' && value != '?') {
            let staged_output = git_output(
                root,
                &[
                    "diff",
                    "--cached",
                    "--no-ext-diff",
                    "--no-color",
                    "--",
                    &pathspec,
                ],
            )?;
            if !staged_output.status.success() {
                return Err(format!(
                    "Git staged diff завершился с ошибкой: {}",
                    String::from_utf8_lossy(&staged_output.stderr).trim()
                ));
            }
            if !staged_output.stdout.is_empty() {
                diff.push_str("# Индексированные изменения\n");
                diff.push_str(&String::from_utf8_lossy(&staged_output.stdout));
            }
        }
        if code
            .chars()
            .any(|value| value == ' ' || value == 'M' || value == 'D' || value == 'A')
        {
            let output = git_output(
                root,
                &["diff", "--no-ext-diff", "--no-color", "--", &pathspec],
            )?;
            if !output.status.success() {
                return Err(format!(
                    "Git diff завершился с ошибкой: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            if !output.stdout.is_empty() {
                if !diff.is_empty() {
                    diff.push('\n');
                }
                diff.push_str("# Незастейдженные изменения\n");
                diff.push_str(&String::from_utf8_lossy(&output.stdout));
            }
        }
        if diff.is_empty() {
            return Err("Для файла нет текстового diff (возможно, он бинарный)".into());
        }
        diff
    };
    Ok(GitFileDiff {
        path: target.to_string_lossy().into_owned(),
        diff,
        staged,
    })
}

#[tauri::command]
fn read_text_file(workspace: State<'_, WorkspaceState>, path: String) -> Result<String, String> {
    let root = workspace.active_root()?;
    let path = checked_workspace_path(&root, &path)?;
    if !path.is_file() {
        return Err("Путь не указывает на файл проекта".into());
    }
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn write_text_file(
    workspace: State<'_, WorkspaceState>,
    path: String,
    content: String,
) -> Result<(), String> {
    let root = workspace.active_root()?;
    let path = checked_workspace_path(&root, &path)?;
    if !path.is_file() {
        return Err("Можно сохранять только существующие файлы проекта".into());
    }
    std::fs::write(path, content).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentFileRollback {
    path: String,
    before: Option<String>,
    after: String,
    existed_before: bool,
}

#[tauri::command]
fn rollback_agent_file_change(
    workspace: State<'_, WorkspaceState>,
    change: AgentFileRollback,
) -> Result<(), String> {
    rollback_agent_file_change_for(&workspace.active_root()?, &change)
}

fn rollback_agent_file_change_for(root: &Path, change: &AgentFileRollback) -> Result<(), String> {
    if !change.existed_before && change.before.is_some() {
        return Err("Журнал изменения файла некорректен".into());
    }
    if change.existed_before && change.before.is_none() {
        return Err("Для восстановления файла отсутствует исходное содержимое".into());
    }
    let root = fs::canonicalize(root).map_err(|error| error.to_string())?;
    let target = checked_workspace_path(&root, &change.path)?;
    if !target.is_file() {
        return Err("Файл больше не существует; безопасный откат невозможен".into());
    }
    let current = fs::read_to_string(&target)
        .map_err(|error| format!("Невозможно проверить файл перед откатом: {error}"))?;
    if current != change.after {
        return Err("Файл изменился после действия агента. Сначала проверьте текущий diff.".into());
    }
    if change.existed_before {
        fs::write(
            target,
            change.before.as_deref().expect("validated before content"),
        )
        .map_err(|error| format!("Не удалось восстановить файл: {error}"))
    } else {
        fs::remove_file(target)
            .map_err(|error| format!("Не удалось удалить созданный агентом файл: {error}"))
    }
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
        terminate_process_tree(&mut child)?;
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
        terminate_process_tree(&mut child)?;
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
    workspace: State<'_, WorkspaceState>,
    repo: String,
    file: String,
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
    let destination = workspace.active_root()?;
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
        .manage(WorkspaceState::default())
        .manage(openhands::AgentServerState::default())
        .invoke_handler(tauri::generate_handler![
            list_directory,
            browse_directory,
            browse_project_parent,
            create_project_directory,
            activate_project,
            browse_gguf,
            browse_executable,
            get_workspace_root,
            default_workspace,
            path_is_file,
            install_llama_runtime,
            scan_models,
            search_workspace,
            git_status,
            git_file_diff,
            read_text_file,
            write_text_file,
            rollback_agent_file_change,
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
            openhands::interrupt_agent_conversation,
            openhands::respond_to_agent_confirmation,
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
                let _ = terminate_process_tree(&mut child);
            }
        }
    });
}

#[cfg(test)]
mod workspace_search_tests {
    use super::{
        checked_workspace_path, create_project_path, git_file_diff_for, git_status_for,
        ignored_search_directory, matching_lines, rollback_agent_file_change_for,
        search_workspace_files, terminate_process_tree, validate_search_query, AgentFileRollback,
        StartModel,
    };
    use std::fs;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    #[test]
    fn local_model_config_deserializes_frontend_camel_case_fields() {
        let config: StartModel = serde_json::from_value(serde_json::json!({
            "modelPath": "C:/models/test.gguf",
            "executable": "C:/runtime/llama-server.exe",
            "context": 1024,
            "gpuLayers": 0
        }))
        .unwrap();

        assert_eq!(config.model_path, "C:/models/test.gguf");
        assert_eq!(config.executable, "C:/runtime/llama-server.exe");
        assert_eq!(config.context, Some(1024));
        assert_eq!(config.gpu_layers, Some(0));
    }

    #[test]
    fn search_matches_case_insensitively_and_keeps_line_numbers() {
        let results = matching_lines(
            "first line\nNeedle here\nneedle again",
            "NEEDLE",
            "src/lib.rs",
            10,
        );
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].line, 2);
        assert_eq!(results[0].path, "src/lib.rs");
        assert_eq!(results[0].preview, "Needle here");
        assert_eq!(results[1].line, 3);
    }

    #[test]
    fn search_caps_results_and_ignores_dependency_and_build_directories() {
        let results = matching_lines("match\nmatch\nmatch", "match", "file.txt", 2);
        assert_eq!(results.len(), 2);
        assert!(ignored_search_directory("node_modules"));
        assert!(ignored_search_directory(".GIT"));
        assert!(!ignored_search_directory("src"));
    }

    #[test]
    fn workspace_search_walks_source_files_and_skips_dependency_folders() {
        let root = std::env::temp_dir().join(format!("flux-hands-search-{}", uuid::Uuid::new_v4()));
        let source = root.join("src");
        let dependencies = root.join("node_modules").join("package");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&dependencies).unwrap();
        fs::write(source.join("main.rs"), "first\nTarget Phrase\nlast").unwrap();
        fs::write(dependencies.join("ignored.js"), "target phrase").unwrap();

        let results = search_workspace_files(&root, "target phrase").unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].path.ends_with("main.rs"));
        assert_eq!(results[0].line, 2);
        assert_eq!(results[0].preview, "Target Phrase");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_search_rejects_empty_and_excessive_queries() {
        assert!(validate_search_query("  ").is_err());
        assert!(validate_search_query(&"x".repeat(257)).is_err());
    }

    #[test]
    fn project_creation_makes_one_child_folder_and_rejects_path_traversal() {
        let parent = std::env::temp_dir().join(format!(
            "flux-hands-project-create-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&parent).unwrap();

        let project = create_project_path(&parent, "  Demo Project  ").unwrap();
        assert_eq!(project.file_name().unwrap(), "Demo Project");
        assert!(project.is_dir());
        assert!(create_project_path(&parent, "../outside").is_err());
        assert!(create_project_path(&parent, "bad:name").is_err());
        assert!(create_project_path(&parent, " ").is_err());
        assert!(!parent.parent().unwrap().join("outside").exists());

        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn terminating_agent_wrapper_stops_its_child_processes() {
        use std::os::windows::process::CommandExt;

        let pid_file = std::env::temp_dir().join(format!(
            "flux-hands-process-tree-{}.txt",
            uuid::Uuid::new_v4()
        ));
        let script = format!(
            "$child = Start-Process -PassThru -WindowStyle Hidden -FilePath $env:ComSpec -ArgumentList '/c','ping -t 127.0.0.1 > NUL'; Set-Content -NoNewline -Path '{}' -Value $child.Id; while ($true) {{ Start-Sleep -Seconds 1 }}",
            pid_file.display()
        );
        let mut wrapper = Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .creation_flags(0x08000000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();

        let child_pid = (0..100).find_map(|_| {
            if let Ok(value) = fs::read_to_string(&pid_file) {
                if let Ok(pid) = value.trim().parse::<u32>() {
                    return Some(pid);
                }
            }
            std::thread::sleep(Duration::from_millis(50));
            None
        });
        let Some(child_pid) = child_pid else {
            let _ = terminate_process_tree(&mut wrapper);
            let _ = fs::remove_file(pid_file);
            panic!("PowerShell did not create the child process");
        };

        terminate_process_tree(&mut wrapper).unwrap();
        let child_check = Command::new("tasklist.exe")
            .args(["/FI", &format!("PID eq {child_pid}"), "/NH"])
            .creation_flags(0x08000000)
            .output()
            .unwrap();
        let child_listing = String::from_utf8_lossy(&child_check.stdout);
        assert!(
            !child_listing.contains(&child_pid.to_string()),
            "child process {child_pid} survived process-tree termination: {child_listing}"
        );
        let _ = fs::remove_file(pid_file);
    }

    #[test]
    fn file_commands_are_bound_to_the_canonical_workspace_root() {
        let parent =
            std::env::temp_dir().join(format!("flux-hands-boundary-{}", uuid::Uuid::new_v4()));
        let root = parent.join("project");
        let outside = parent.join("project-sibling");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let project_file = root.join("source.txt");
        let outside_file = outside.join("secret.txt");
        fs::write(&project_file, "before").unwrap();
        fs::write(&outside_file, "secret").unwrap();

        let root = fs::canonicalize(root).unwrap();
        let project_path = project_file.to_string_lossy().into_owned();
        let outside_path = outside_file.to_string_lossy().into_owned();
        assert_eq!(
            checked_workspace_path(&root, &project_path).unwrap(),
            root.join("source.txt")
        );
        assert!(checked_workspace_path(&root, &outside_path).is_err());
        assert_eq!(fs::read_to_string(&project_file).unwrap(), "before");
        assert_eq!(fs::read_to_string(&outside_file).unwrap(), "secret");

        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn file_commands_reject_symlinks_that_escape_the_workspace() {
        use std::os::unix::fs::symlink;

        let parent =
            std::env::temp_dir().join(format!("flux-hands-symlink-{}", uuid::Uuid::new_v4()));
        let root = parent.join("project");
        let outside = parent.join("outside.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, "secret").unwrap();
        let link = root.join("linked.txt");
        symlink(&outside, &link).unwrap();

        let root = fs::canonicalize(root).unwrap();
        assert!(checked_workspace_path(&root, &link.to_string_lossy()).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "secret");

        fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn git_status_reports_branch_modified_and_untracked_files() {
        let root = std::env::temp_dir().join(format!("flux-hands-git-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(run(&["init", "-b", "test-branch"]).status.success());
        assert!(
            run(&["config", "user.email", "flux-hands-test@example.invalid"])
                .status
                .success()
        );
        assert!(run(&["config", "user.name", "Flux Hands Test"])
            .status
            .success());
        fs::write(root.join("tracked.txt"), "before").unwrap();
        assert!(run(&["add", "tracked.txt"]).status.success());
        assert!(run(&["commit", "-m", "initial"]).status.success());
        fs::write(root.join("tracked.txt"), "after").unwrap();
        fs::write(root.join("new file.txt"), "new").unwrap();

        let status = git_status_for(&root).unwrap();
        assert_eq!(status.branch, "test-branch");
        assert_eq!(status.changes.len(), 2);
        assert!(status
            .changes
            .iter()
            .any(|change| change.code == " M" && change.path.ends_with("tracked.txt")));
        assert!(status
            .changes
            .iter()
            .any(|change| change.code == "??" && change.path.ends_with("new file.txt")));

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn git_diff_previews_tracked_and_untracked_text_files_inside_project() {
        let root =
            std::env::temp_dir().join(format!("flux-hands-git-diff-{}", uuid::Uuid::new_v4()));
        let outside = root.with_extension("outside");
        fs::create_dir_all(&root).unwrap();
        fs::write(&outside, "secret").unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(run(&["init", "-b", "test-branch"]).status.success());
        assert!(
            run(&["config", "user.email", "flux-hands-test@example.invalid"])
                .status
                .success()
        );
        assert!(run(&["config", "user.name", "Flux Hands Test"])
            .status
            .success());
        let tracked = root.join("tracked.txt");
        fs::write(&tracked, "before\n").unwrap();
        assert!(run(&["add", "tracked.txt"]).status.success());
        assert!(run(&["commit", "-m", "initial"]).status.success());
        fs::write(&tracked, "staged\n").unwrap();
        assert!(run(&["add", "tracked.txt"]).status.success());
        fs::write(&tracked, "after\n").unwrap();
        let added = root.join("new file.txt");
        fs::write(&added, "new line\n").unwrap();

        let tracked_diff = git_file_diff_for(&root, &tracked.to_string_lossy()).unwrap();
        assert!(tracked_diff.diff.contains("# Индексированные изменения"));
        assert!(tracked_diff.diff.contains("-before"));
        assert!(tracked_diff.diff.contains("+staged"));
        assert!(tracked_diff.diff.contains("# Незастейдженные изменения"));
        assert!(tracked_diff.diff.contains("-before"));
        assert!(tracked_diff.diff.contains("+after"));
        let added_diff = git_file_diff_for(&root, &added.to_string_lossy()).unwrap();
        assert!(added_diff.diff.contains("+new line"));
        assert!(git_file_diff_for(&root, &outside.to_string_lossy()).is_err());

        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }

    #[test]
    fn agent_file_rollback_restores_only_unchanged_workspace_files() {
        let parent =
            std::env::temp_dir().join(format!("flux-hands-rollback-{}", uuid::Uuid::new_v4()));
        let root = parent.join("project");
        fs::create_dir_all(&root).unwrap();
        let edited = root.join("edited.txt");
        fs::write(&edited, "agent version").unwrap();
        let change = AgentFileRollback {
            path: edited.to_string_lossy().into_owned(),
            before: Some("original".into()),
            after: "agent version".into(),
            existed_before: true,
        };
        rollback_agent_file_change_for(&root, &change).unwrap();
        assert_eq!(fs::read_to_string(&edited).unwrap(), "original");

        let created = root.join("created.txt");
        fs::write(&created, "created by agent").unwrap();
        let create_change = AgentFileRollback {
            path: created.to_string_lossy().into_owned(),
            before: None,
            after: "created by agent".into(),
            existed_before: false,
        };
        rollback_agent_file_change_for(&root, &create_change).unwrap();
        assert!(!created.exists());

        fs::write(&edited, "user edits after agent").unwrap();
        assert!(rollback_agent_file_change_for(&root, &change).is_err());
        assert_eq!(
            fs::read_to_string(&edited).unwrap(),
            "user edits after agent"
        );
        let outside = parent.join("outside.txt");
        fs::write(&outside, "do not touch").unwrap();
        let outside_change = AgentFileRollback {
            path: outside.to_string_lossy().into_owned(),
            before: Some("secret".into()),
            after: "do not touch".into(),
            existed_before: true,
        };
        assert!(rollback_agent_file_change_for(&root, &outside_change).is_err());
        assert_eq!(fs::read_to_string(&outside).unwrap(), "do not touch");

        fs::remove_dir_all(parent).unwrap();
    }
}
