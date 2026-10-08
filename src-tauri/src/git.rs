use std::path::Path;
use std::process::{Command, Output};

use serde::Serialize;

fn run_git(root: &Path, args: &[&str]) -> Result<Output, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not open project folder: {error}"))?;
    if !root.is_dir() {
        return Err("The selected project path is not a folder".to_string());
    }
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|error| format!("Could not start Git: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GitChange {
    path: String,
    index_status: String,
    worktree_status: String,
    untracked: bool,
}

#[tauri::command]
pub fn git_status(project_path: String) -> Result<Vec<GitChange>, String> {
    let output = run_git(
        Path::new(&project_path),
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    )?;
    let mut fields = output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty());
    let mut changes = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let index = field[0] as char;
        let worktree = field[1] as char;
        let path = String::from_utf8_lossy(&field[3..]).into_owned();
        let path = path.replace('\\', "/");
        changes.push(GitChange {
            untracked: index == '?' && worktree == '?',
            path,
            index_status: index.to_string(),
            worktree_status: worktree.to_string(),
        });
        if matches!(index, 'R' | 'C') || matches!(worktree, 'R' | 'C') {
            let _ = fields.next();
        }
    }
    Ok(changes)
}

#[tauri::command]
pub fn git_diff(project_path: String, relative_path: String) -> Result<String, String> {
    if Path::new(&relative_path).is_absolute()
        || Path::new(&relative_path).components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Use a file path inside the project".to_string());
    }
    let output = run_git(
        Path::new(&project_path),
        &["diff", "--no-ext-diff", "--", &relative_path],
    )?;
    if !output.stdout.is_empty() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let cached = run_git(
        Path::new(&project_path),
        &["diff", "--cached", "--no-ext-diff", "--", &relative_path],
    )?;
    if !cached.stdout.is_empty() {
        return Ok(String::from_utf8_lossy(&cached.stdout).into_owned());
    }
    let root = Path::new(&project_path)
        .canonicalize()
        .map_err(|error| format!("Could not open project folder: {error}"))?;
    let path = root.join(&relative_path);
    let tracked = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args(["ls-files", "--error-unmatch", "--", &relative_path])
        .output()
        .map(|result| result.status.success())
        .unwrap_or(false);
    if tracked || !path.is_file() {
        return Ok(String::new());
    }
    let bytes = std::fs::read(&path).map_err(|error| format!("Could not read file: {error}"))?;
    if bytes.contains(&0) {
        return Ok(format!("Binary file {relative_path} is not shown"));
    }
    let content = String::from_utf8_lossy(&bytes);
    let lines = content.lines().collect::<Vec<_>>();
    let mut diff = format!(
        "diff --git a/{relative_path} b/{relative_path}\nnew file mode 100644\n--- /dev/null\n+++ b/{relative_path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for line in lines {
        diff.push('+');
        diff.push_str(line);
        diff.push('\n');
    }
    Ok(diff)
}

#[tauri::command]
pub fn git_stage(project_path: String, relative_path: String, staged: bool) -> Result<(), String> {
    if Path::new(&relative_path).is_absolute()
        || Path::new(&relative_path).components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::Prefix(_)
            )
        })
    {
        return Err("Use a file path inside the project".to_string());
    }
    let args = if staged {
        vec!["restore", "--staged", "--", relative_path.as_str()]
    } else {
        vec!["add", "--", relative_path.as_str()]
    };
    run_git(Path::new(&project_path), &args)?;
    Ok(())
}

#[tauri::command]
pub fn git_commit(project_path: String, message: String) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("Enter a commit message".to_string());
    }
    let output = run_git(Path::new(&project_path), &["commit", "-m", message])?;
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
