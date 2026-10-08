use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

#[derive(Serialize)]
pub struct WorkspaceEntry {
    name: String,
    path: String,
    is_directory: bool,
}

fn project_root(path: &str) -> Result<PathBuf, String> {
    let root = Path::new(path)
        .canonicalize()
        .map_err(|error| format!("Could not open the project folder: {error}"))?;
    if !root.is_dir() {
        return Err("The selected project path is not a folder".to_string());
    }
    Ok(root)
}

fn project_target(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err("Use a relative path inside the opened project folder".to_string());
    }
    let target = root.join(relative);
    let existing = if fs::symlink_metadata(&target).is_ok() {
        target.canonicalize().map_err(|error| error.to_string())?
    } else {
        let parent = target
            .parent()
            .ok_or_else(|| "The file path has no parent folder".to_string())?
            .canonicalize()
            .map_err(|error| format!("Could not access the project folder: {error}"))?;
        if !parent.starts_with(root) {
            return Err("The path escapes the opened project folder".to_string());
        }
        parent.join(
            target
                .file_name()
                .ok_or_else(|| "The file path has no filename".to_string())?,
        )
    };
    if !existing.starts_with(root) {
        return Err("The path escapes the opened project folder".to_string());
    }
    Ok(existing)
}

fn project_entry(root: &Path, path: &str) -> Result<PathBuf, String> {
    let relative = Path::new(path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
                    | Component::CurDir
            )
        })
    {
        return Err("Use a relative path inside the opened project folder".to_string());
    }
    let target = root.join(relative);
    let parent = target
        .parent()
        .ok_or_else(|| "The item has no parent folder".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not access the project folder: {error}"))?;
    if !parent.starts_with(root) {
        return Err("The path escapes the opened project folder".to_string());
    }
    Ok(parent.join(
        target
            .file_name()
            .ok_or_else(|| "The item has no name".to_string())?,
    ))
}

#[tauri::command]
pub fn list_workspace_directory(
    project_path: String,
    relative_path: Option<String>,
) -> Result<Vec<WorkspaceEntry>, String> {
    let root = project_root(&project_path)?;
    let folder = project_target(&root, relative_path.as_deref().unwrap_or(""))?;
    if !folder.is_dir() {
        return Err("The selected path is not a folder".to_string());
    }
    let mut entries = fs::read_dir(&folder)
        .map_err(|error| format!("Could not list the folder: {error}"))?
        .map(|entry| {
            let entry = entry.map_err(|error| error.to_string())?;
            let path = entry.path();
            let canonical = path.canonicalize().map_err(|error| error.to_string())?;
            if !canonical.starts_with(&root) {
                return Ok(None);
            }
            let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
            if !metadata.is_file() && !metadata.is_dir() {
                return Ok(None);
            }
            Ok(Some(WorkspaceEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: path
                    .strip_prefix(&root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
                is_directory: metadata.is_dir(),
            }))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        right
            .is_directory
            .cmp(&left.is_directory)
            .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
    });
    Ok(entries)
}

#[tauri::command]
pub fn read_workspace_file(project_path: String, relative_path: String) -> Result<String, String> {
    let root = project_root(&project_path)?;
    let target = project_target(&root, &relative_path)?;
    if !target.is_file() {
        return Err("The selected path is not a file".to_string());
    }
    fs::read_to_string(target).map_err(|error| {
        format!("Could not open this file as UTF-8 text. It may be a binary file: {error}")
    })
}

#[tauri::command]
pub fn write_workspace_file(
    project_path: String,
    relative_path: String,
    content: String,
) -> Result<(), String> {
    let root = project_root(&project_path)?;
    let target = project_target(&root, &relative_path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("Could not create folder: {error}"))?;
    }
    let safe_target = project_target(&root, &relative_path)?;
    fs::write(safe_target, content).map_err(|error| format!("Could not save file: {error}"))
}

#[tauri::command]
pub fn create_workspace_entry(
    project_path: String,
    relative_path: String,
    is_directory: bool,
) -> Result<(), String> {
    let root = project_root(&project_path)?;
    let target = project_target(&root, &relative_path)?;
    if target.exists() {
        return Err("An item with this name already exists".to_string());
    }
    if is_directory {
        fs::create_dir(&target).map_err(|error| format!("Could not create folder: {error}"))
    } else {
        fs::File::create(&target)
            .map(|_| ())
            .map_err(|error| format!("Could not create file: {error}"))
    }
}

#[tauri::command]
pub fn rename_workspace_entry(
    project_path: String,
    relative_path: String,
    new_name: String,
) -> Result<String, String> {
    let root = project_root(&project_path)?;
    let target = project_entry(&root, &relative_path)?;
    if !target.exists() {
        return Err("The selected item no longer exists".to_string());
    }
    let new_name = new_name.trim();
    if new_name.is_empty()
        || new_name.contains('/')
        || new_name.contains('\\')
        || new_name == "."
        || new_name == ".."
    {
        return Err("Enter a valid file or folder name".to_string());
    }
    let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("Renaming symbolic links is not supported".to_string());
    }
    let parent = target
        .parent()
        .ok_or_else(|| "The selected item has no parent folder".to_string())?;
    let renamed = parent.join(new_name);
    if renamed.exists() {
        return Err("An item with this name already exists".to_string());
    }
    fs::rename(&target, &renamed).map_err(|error| format!("Could not rename item: {error}"))?;
    renamed
        .strip_prefix(&root)
        .map(|path| path.to_string_lossy().replace('\\', "/"))
        .map_err(|_| "The renamed item escapes the project folder".to_string())
}

#[tauri::command]
pub fn delete_workspace_entry(project_path: String, relative_path: String) -> Result<(), String> {
    let root = project_root(&project_path)?;
    let target = project_entry(&root, &relative_path)?;
    if target == root {
        return Err("The opened project folder cannot be deleted here".to_string());
    }
    let metadata = fs::symlink_metadata(&target).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(target).map_err(|error| format!("Could not delete item: {error}"))
    } else if metadata.is_dir() {
        fs::remove_dir_all(target).map_err(|error| format!("Could not delete folder: {error}"))
    } else {
        Err("The selected item cannot be deleted".to_string())
    }
}

#[derive(Serialize)]
pub struct SearchMatch {
    path: String,
    line: Option<usize>,
    preview: String,
    file_name_match: bool,
}

#[derive(Serialize)]
pub struct SearchPage {
    matches: Vec<SearchMatch>,
    total: usize,
    offset: usize,
    page_size: usize,
}

const SEARCH_PAGE_SIZE: usize = 100;
const SEARCH_SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    ".next",
    ".venv",
    "venv",
    "coverage",
];

fn search_files(
    root: &Path,
    query: &str,
    offset: usize,
    include_generated: bool,
) -> Result<SearchPage, String> {
    let needle = query.to_lowercase();
    let mut folders = vec![root.to_path_buf()];
    let mut matches = Vec::new();
    let mut total = 0usize;
    let end = offset.saturating_add(SEARCH_PAGE_SIZE);

    while let Some(folder) = folders.pop() {
        let entries = fs::read_dir(&folder)
            .map_err(|error| format!("Could not search {}: {error}", folder.display()))?;
        for entry in entries {
            let entry = entry.map_err(|error| error.to_string())?;
            let kind = entry.file_type().map_err(|error| error.to_string())?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if kind.is_dir() {
                if include_generated || !SEARCH_SKIP_DIRS.contains(&name.to_lowercase().as_str()) {
                    folders.push(path);
                }
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if name.to_lowercase().contains(&needle) {
                if total >= offset && total < end {
                    matches.push(SearchMatch {
                        path: relative.clone(),
                        line: None,
                        preview: name.clone(),
                        file_name_match: true,
                    });
                }
                total = total.saturating_add(1);
            }
            let Ok(file) = fs::File::open(&path) else {
                continue;
            };
            for (index, line) in BufReader::new(file).lines().enumerate() {
                let Ok(line) = line else { break };
                if !line.to_lowercase().contains(&needle) {
                    continue;
                }
                if total >= offset && total < end {
                    let preview = line.chars().take(240).collect::<String>();
                    matches.push(SearchMatch {
                        path: relative.clone(),
                        line: Some(index + 1),
                        preview,
                        file_name_match: false,
                    });
                }
                total = total.saturating_add(1);
            }
        }
    }
    Ok(SearchPage {
        matches,
        total,
        offset,
        page_size: SEARCH_PAGE_SIZE,
    })
}

#[tauri::command]
pub async fn search_workspace(
    project_path: String,
    query: String,
    offset: usize,
    include_generated: bool,
) -> Result<SearchPage, String> {
    if query.trim().is_empty() {
        return Err("Enter text to search for".to_string());
    }
    let root = project_root(&project_path)?;
    tauri::async_runtime::spawn_blocking(move || {
        search_files(&root, query.trim(), offset, include_generated)
    })
    .await
    .map_err(|error| format!("Search task failed: {error}"))?
}
