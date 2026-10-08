use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use printpdf::{
    Mm, Op, ParsedFont, PdfDocument, PdfFontHandle, PdfPage, PdfSaveOptions, Point, Pt, TextItem,
};
use quick_xml::Reader as XmlReader;
use quick_xml::events::Event;
use serde::Deserialize;

const MAX_DIRECTORY_ENTRIES: usize = 500;
const MAX_WEB_BYTES: usize = 128 * 1024;
const MAX_PDF_TEXT_BYTES: usize = 2 * 1024 * 1024;
const MAX_SEARCH_RESULTS: usize = 40;
const MAX_SEARCH_BYTES: usize = 48 * 1024;
const SEARCH_CONTEXT_LINES: usize = 2;
const MAX_SEARCH_FILES: usize = 8_000;
const MAX_SEARCH_SCANNED_BYTES: usize = 128 * 1024 * 1024;
const MAX_AGENT_READ_BYTES: usize = 256 * 1024;

#[derive(Deserialize)]
pub struct AgentAction {
    pub name: String,
    #[serde(default)]
    pub path: String,
    pub content: Option<String>,
    pub url: Option<String>,
    pub title: Option<String>,
    pub command: Option<String>,
    pub query: Option<String>,
    pub start_line: Option<usize>,
    pub end_line: Option<usize>,
}

fn extract_docx(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|error| format!("Could not open DOCX: {error}"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| format!("Could not read DOCX archive: {error}"))?;
    let mut document = archive
        .by_name("word/document.xml")
        .map_err(|error| format!("DOCX does not contain its main document: {error}"))?;
    let mut xml = Vec::new();
    document
        .read_to_end(&mut xml)
        .map_err(|error| format!("Could not read DOCX text: {error}"))?;
    let mut reader = XmlReader::from_reader(xml.as_slice());
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut text = String::new();
    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Text(event)) => {
                if let Ok(decoded) = event.decode()
                    && let Ok(unescaped) = quick_xml::escape::unescape(&decoded)
                {
                    text.push_str(&unescaped);
                }
            }
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => match event.name().as_ref() {
                b"w:tab" => text.push('\t'),
                b"w:br" => text.push('\n'),
                _ => {}
            },
            Ok(Event::End(event)) if event.name().as_ref() == b"w:p" => text.push('\n'),
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("Could not parse DOCX text: {error}")),
            _ => {}
        }
        buffer.clear();
    }
    if text.trim().is_empty() {
        return Err("No readable text was found in the DOCX document".to_string());
    }
    Ok(text)
}

fn read_agent_file(path: &Path) -> Result<String, String> {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "docx" => extract_docx(path),
        "pdf" => {
            let text = pdf_extract::extract_text(path)
                .map_err(|error| format!("Could not extract PDF text: {error}"))?;
            if text.trim().is_empty() {
                return Err(
                    "This PDF contains no extractable text; it may be scanned and require OCR"
                        .to_string(),
                );
            }
            Ok(text)
        }
        _ => fs::read_to_string(path)
            .map_err(|error| format!("Could not read the file as UTF-8 text: {error}")),
    }
}

fn read_agent_file_chunk(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String, String> {
    let content = read_agent_file(path)?;
    let lines = content.lines().collect::<Vec<_>>();
    if let (Some(start), Some(end)) = (start_line, end_line)
        && (start == 0 || end < start)
    {
        return Err("Line ranges are 1-based and end_line must be at least start_line".to_string());
    }
    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or_else(|| lines.len().max(1));
    if start > lines.len() && !lines.is_empty() {
        return Err(format!(
            "start_line {start} is beyond the file's {} lines",
            lines.len()
        ));
    }
    let available = lines.len().saturating_sub(start.saturating_sub(1));
    let selected_end = end.min(start.saturating_add(available.saturating_sub(1)));
    let mut output = lines
        .iter()
        .enumerate()
        .skip(start.saturating_sub(1))
        .take(selected_end.saturating_sub(start).saturating_add(1))
        .map(|(index, line)| format!("{}: {}", index + 1, line))
        .collect::<Vec<_>>()
        .join("\n");
    if output.len() > MAX_AGENT_READ_BYTES {
        let mut boundary = MAX_AGENT_READ_BYTES;
        while !output.is_char_boundary(boundary) {
            boundary -= 1;
        }
        output.truncate(boundary);
        output.push_str(
            "\n[Output truncated. Request a smaller line range with start_line/end_line.]",
        );
    } else if end < lines.len() {
        output.push_str(&format!(
            "\n[File continues through line {}. Request another range to read more.]",
            lines.len()
        ));
    }
    if output.is_empty() {
        Ok(format!("File is empty: {}", path.display()))
    } else {
        Ok(output)
    }
}

fn resolve_project_path(project_path: &str, relative_path: &str) -> Result<PathBuf, String> {
    let root = Path::new(project_path)
        .canonicalize()
        .map_err(|error| format!("Could not open the selected project folder: {error}"))?;
    if !root.is_dir() {
        return Err("The selected project path is not a folder".to_string());
    }
    let relative = Path::new(relative_path);
    if relative.as_os_str().is_empty()
        || relative.is_absolute()
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
    let parent = target
        .parent()
        .ok_or_else(|| "The path has no parent folder".to_string())?
        .canonicalize()
        .map_err(|error| format!("Could not access the project folder: {error}"))?;
    if !parent.starts_with(&root) {
        return Err("The path escapes the opened project folder".to_string());
    }
    Ok(target)
}

fn resolve_full_path(path: &str, directory: bool) -> Result<PathBuf, String> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err("Full access actions require an absolute path".to_string());
    }
    let canonical = if requested.exists() {
        requested
            .canonicalize()
            .map_err(|error| error.to_string())?
    } else if !directory {
        let parent = requested
            .parent()
            .ok_or_else(|| "The path has no parent folder".to_string())?
            .canonicalize()
            .map_err(|error| format!("Could not access the target folder: {error}"))?;
        let name = requested
            .file_name()
            .ok_or_else(|| "The file path has no filename".to_string())?;
        parent.join(name)
    } else {
        return Err("The requested folder does not exist".to_string());
    };
    if canonical.exists() {
        let metadata = fs::metadata(&canonical).map_err(|error| error.to_string())?;
        if directory && !metadata.is_dir() {
            return Err("The selected path is not a folder".to_string());
        }
        if !directory && !metadata.is_file() {
            return Err("The selected path is not a regular file".to_string());
        }
    } else if directory {
        return Err("The requested folder does not exist".to_string());
    }
    Ok(canonical)
}

fn resolve_full_write_path(path: &str) -> Result<PathBuf, String> {
    let requested = Path::new(path);
    if !requested.is_absolute() {
        return Err("Full access actions require an absolute path".to_string());
    }
    let parent = requested
        .parent()
        .ok_or_else(|| "The file path has no parent folder".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the target folder: {error}"))?;
    resolve_full_path(path, false)
}

fn list_directory(path: &Path, full_access: bool) -> Result<String, String> {
    let mut entries = fs::read_dir(path)
        .map_err(|error| format!("Could not list the folder: {error}"))?
        .flatten()
        .take(if full_access {
            usize::MAX
        } else {
            MAX_DIRECTORY_ENTRIES
        })
        .map(|entry| {
            let kind = if entry
                .file_type()
                .map(|value| value.is_dir())
                .unwrap_or(false)
            {
                "[folder]"
            } else {
                "[file]"
            };
            format!("{kind} {}", entry.file_name().to_string_lossy())
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();
    Ok(format!(
        "Folder: {}\n{}",
        path.display(),
        entries.join("\n")
    ))
}

#[derive(Clone)]
struct SearchHit {
    score: usize,
    path: String,
    line: usize,
    preview: String,
}

fn ignored_project_path(relative: &Path, file_name: &str) -> bool {
    let ignored_directories = [
        ".git",
        "node_modules",
        "target",
        "dist",
        ".next",
        ".venv",
        "venv",
        "coverage",
        "build",
        "out",
        "vendor",
        ".idea",
        ".vscode-test",
    ];
    if relative.components().any(|component| {
        let Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        ignored_directories
            .iter()
            .any(|ignored| name.eq_ignore_ascii_case(ignored))
    }) {
        return true;
    }
    let lower = file_name.to_ascii_lowercase();
    let sensitive_names = [
        ".env",
        ".npmrc",
        ".pypirc",
        ".netrc",
        "id_rsa",
        "id_ed25519",
        "credentials",
        "secrets.json",
        "secret.json",
        "service-account.json",
    ];
    if sensitive_names.iter().any(|name| lower == *name)
        || lower.starts_with(".env.")
        || lower.ends_with(".pem")
        || lower.ends_with(".key")
        || lower.ends_with(".p12")
        || lower.ends_with(".pfx")
        || lower.ends_with(".keystore")
        || lower.ends_with(".jks")
    {
        return true;
    }
    false
}

fn search_project_text(root: &Path, query: &str) -> Result<String, String> {
    let terms = query
        .split_whitespace()
        .map(str::to_lowercase)
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>();
    if terms.is_empty() {
        return Err("Enter words or phrases to search for".to_string());
    }
    let mut folders = vec![root.to_path_buf()];
    let mut hits = Vec::new();
    let mut files_scanned = 0usize;
    let mut bytes_scanned = 0usize;
    while let Some(folder) = folders.pop() {
        if files_scanned >= MAX_SEARCH_FILES || bytes_scanned >= MAX_SEARCH_SCANNED_BYTES {
            break;
        }
        let entries = fs::read_dir(&folder)
            .map_err(|error| format!("Could not search {}: {error}", folder.display()))?;
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let relative_path = path.strip_prefix(root).unwrap_or(&path);
            if ignored_project_path(relative_path, &name) {
                continue;
            }
            if kind.is_dir() {
                folders.push(path);
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            files_scanned += 1;
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let lower_name = name.to_lowercase();
            let name_score = terms
                .iter()
                .filter(|term| lower_name.contains(term.as_str()))
                .count()
                * 8;
            if name_score > 0 {
                hits.push(SearchHit {
                    score: name_score,
                    path: relative.clone(),
                    line: 0,
                    preview: name.clone(),
                });
            }
            let file = match fs::File::open(&path) {
                Ok(file) => file,
                Err(_) => continue,
            };
            let remaining = MAX_SEARCH_SCANNED_BYTES.saturating_sub(bytes_scanned);
            let mut bytes = Vec::new();
            let mut limited = file.take(remaining.min(2 * 1024 * 1024) as u64);
            if limited.read_to_end(&mut bytes).is_err() || bytes.contains(&0) {
                continue;
            }
            bytes_scanned = bytes_scanned.saturating_add(bytes.len());
            let Ok(text) = String::from_utf8(bytes) else {
                continue;
            };
            let lines = text.lines().collect::<Vec<_>>();
            for (index, line) in lines.iter().enumerate() {
                let lower = line.to_lowercase();
                let score = terms
                    .iter()
                    .filter(|term| lower.contains(term.as_str()))
                    .count();
                if score > 0 {
                    let start = index.saturating_sub(SEARCH_CONTEXT_LINES);
                    let end = (index + SEARCH_CONTEXT_LINES + 1).min(lines.len());
                    let preview = lines[start..end]
                        .iter()
                        .enumerate()
                        .map(|(offset, context)| {
                            format!("{}: {}", start + offset + 1, context.trim())
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    hits.push(SearchHit {
                        score,
                        path: relative.clone(),
                        line: index + 1,
                        preview,
                    });
                }
            }
        }
    }
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.path.cmp(&right.path))
    });
    hits.dedup_by(|left, right| left.path == right.path && left.line == right.line);
    let mut result = format!(
        "Project search: {}\nRelevant results (best matches first):",
        query.trim()
    );
    let mut included_bytes = result.len();
    for hit in hits.into_iter().take(MAX_SEARCH_RESULTS) {
        let entry = if hit.line == 0 {
            format!("\n{} [file name match]: {}", hit.path, hit.preview)
        } else {
            format!("\n{}:{}:\n{}", hit.path, hit.line, hit.preview)
        };
        if included_bytes + entry.len() > MAX_SEARCH_BYTES {
            result.push_str("\n[Search results truncated to fit the model context]");
            break;
        }
        included_bytes += entry.len();
        result.push_str(&entry);
    }
    if result.ends_with("first):") {
        result.push_str("\nNo matching files or lines were found.");
    }
    if files_scanned >= MAX_SEARCH_FILES || bytes_scanned >= MAX_SEARCH_SCANNED_BYTES {
        result.push_str(
            "\n[Search stopped at the project scan limit; refine the query or search a subfolder]",
        );
    }
    Ok(result)
}

fn default_pdf_path(filename: &str, title: Option<&str>) -> Result<PathBuf, String> {
    let downloads = std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| "Could not locate your Windows user folder".to_string())?
        .join("Downloads");
    let name = if filename.trim().is_empty() {
        let safe_title = title
            .unwrap_or("Flux Code document")
            .chars()
            .map(|character| {
                if character.is_control()
                    || matches!(
                        character,
                        '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
                    )
                {
                    '_'
                } else {
                    character
                }
            })
            .collect::<String>()
            .trim_matches([' ', '.'])
            .to_string();
        format!(
            "{}.pdf",
            if safe_title.is_empty() {
                "Flux Code document"
            } else {
                &safe_title
            }
        )
    } else {
        let candidate = Path::new(filename);
        if candidate.components().count() != 1 || candidate.file_name().is_none() {
            return Err("Use a filename only when saving the PDF to Downloads".to_string());
        }
        filename.to_string()
    };
    Ok(downloads.join(name))
}

fn create_pdf(target: &Path, title: &str, content: &str) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("The PDF content is empty".to_string());
    }
    if target
        .extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !extension.eq_ignore_ascii_case("pdf"))
    {
        return Err("PDF output path must end in .pdf".to_string());
    }

    let windows = std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let font_path = ["arial.ttf", "segoeui.ttf", "calibri.ttf"]
        .into_iter()
        .map(|font| windows.join("Fonts").join(font))
        .find(|path| path.is_file())
        .ok_or_else(|| {
            "A Unicode Windows font (Arial, Segoe UI, or Calibri) is required to create the PDF"
                .to_string()
        })?;
    let font_data =
        fs::read(font_path).map_err(|error| format!("Could not load a Unicode font: {error}"))?;
    let mut font_warnings = Vec::new();
    let font = ParsedFont::from_bytes(&font_data, 0, &mut font_warnings)
        .ok_or_else(|| "Could not parse the installed Unicode font".to_string())?;

    let mut document = PdfDocument::new(title);
    let font_id = document.add_font(&font);
    let mut lines: Vec<(String, bool)> = Vec::new();
    if !title.trim().is_empty() {
        lines.push((title.trim().to_string(), true));
        lines.push((String::new(), false));
    }
    for source_line in content.lines() {
        let line = source_line.trim_end();
        if line.is_empty() {
            lines.push((String::new(), false));
            continue;
        }
        let heading = line.trim_start().starts_with('#');
        let cleaned = line
            .trim_start_matches('#')
            .trim_start()
            .replace("**", "")
            .replace('`', "");
        let max_chars = if heading { 74 } else { 92 };
        let mut current = String::new();
        for word in cleaned.split_whitespace() {
            if !current.is_empty() && current.chars().count() + 1 + word.chars().count() > max_chars
            {
                lines.push((current, heading));
                current = String::new();
            }
            if !current.is_empty() {
                current.push(' ');
            }
            for character in word.chars() {
                if current.chars().count() >= max_chars {
                    lines.push((current, heading));
                    current = String::new();
                }
                current.push(character);
            }
        }
        if !current.is_empty() {
            lines.push((current, heading));
        }
    }
    if lines.is_empty() {
        lines.push((String::new(), false));
    }

    let mut pages = Vec::new();
    for page_lines in lines.chunks(47) {
        let mut ops = vec![
            Op::StartTextSection,
            Op::SetTextCursor {
                pos: Point {
                    x: Mm(18.0).into(),
                    y: Mm(278.0).into(),
                },
            },
            Op::SetLineHeight { lh: Pt(15.0) },
        ];
        for (line, heading) in page_lines {
            ops.push(Op::SetFont {
                font: PdfFontHandle::External(font_id.clone()),
                size: Pt(if *heading { 14.0 } else { 10.5 }),
            });
            ops.push(Op::ShowText {
                items: vec![TextItem::Text(line.clone())],
            });
            ops.push(Op::AddLineBreak);
        }
        ops.push(Op::EndTextSection);
        pages.push(PdfPage::new(Mm(210.0), Mm(297.0), ops));
    }
    let mut warnings = Vec::new();
    let bytes = document.with_pages(pages).save(
        &PdfSaveOptions {
            subset_fonts: true,
            ..Default::default()
        },
        &mut warnings,
    );
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create the PDF folder: {error}"))?;
    }
    fs::write(target, &bytes).map_err(|error| format!("Could not save PDF: {error}"))?;
    let saved = fs::read(target).map_err(|error| format!("Could not verify PDF: {error}"))?;
    if saved.len() != bytes.len() || !saved.starts_with(b"%PDF-") {
        return Err("The saved PDF did not pass file verification".to_string());
    }
    Ok(format!("PDF created: {}", target.display()))
}

async fn fetch_https(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|error| format!("Invalid URL: {error}"))?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("Only public HTTPS URLs are supported".to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(|error| error.to_string())?;
    let mut response = client
        .get(parsed)
        .send()
        .await
        .map_err(|error| format!("Could not fetch URL: {error}"))?;
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        let remaining = MAX_WEB_BYTES.saturating_sub(body.len());
        body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
        if body.len() >= MAX_WEB_BYTES {
            break;
        }
    }
    let text = String::from_utf8_lossy(&body);
    Ok(format!("HTTP {status}\n{}", text))
}

fn run_local_command(command: &str, working_directory: Option<&Path>) -> Result<String, String> {
    if command.trim().is_empty() {
        return Err("The command is empty".to_string());
    }

    #[cfg(target_os = "windows")]
    let mut process = {
        let mut process = Command::new("powershell.exe");
        process.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            command,
        ]);
        process
    };
    #[cfg(not(target_os = "windows"))]
    let mut process = {
        let mut process = Command::new("sh");
        process.args(["-lc", command]);
        process
    };

    if let Some(directory) = working_directory {
        if !directory.is_dir() {
            return Err(format!(
                "Working directory does not exist: {}",
                directory.display()
            ));
        }
        process.current_dir(directory);
    }

    let output = process
        .output()
        .map_err(|error| format!("Could not start the command: {error}"))?;
    let mut result = String::new();
    if !output.stdout.is_empty() {
        result.push_str(&String::from_utf8_lossy(&output.stdout));
    }
    if !output.stderr.is_empty() {
        if !result.is_empty() && !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str("[stderr]\n");
        result.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    if result.is_empty() {
        result.push_str("(no output)");
    }
    Ok(format!(
        "Exit code: {}\n{result}",
        output.status.code().unwrap_or(-1)
    ))
}

#[tauri::command]
pub async fn execute_agent_action(
    action: AgentAction,
    project_path: Option<String>,
    access_mode: String,
) -> Result<String, String> {
    if action.name == "run_command" {
        if access_mode != "full" {
            return Err("Local command execution requires Full access".to_string());
        }
        let command = action
            .command
            .as_deref()
            .ok_or_else(|| "The command is missing".to_string())?;
        let working_directory = if action.path.trim().is_empty() {
            None
        } else {
            Some(resolve_full_path(&action.path, true)?)
        };
        return run_local_command(command, working_directory.as_deref());
    }
    if action.name == "create_pdf" {
        let content = action
            .content
            .as_deref()
            .ok_or_else(|| "The PDF content is missing".to_string())?;
        if access_mode != "full" && content.len() > MAX_PDF_TEXT_BYTES {
            return Err("PDF content is larger than 2 MB".to_string());
        }
        let title = action.title.as_deref().unwrap_or("Flux Code document");
        let target = if access_mode == "full" {
            let requested = Path::new(&action.path);
            if action.path.trim().is_empty() || !requested.is_absolute() {
                default_pdf_path(&action.path, action.title.as_deref())?
            } else {
                resolve_full_write_path(&action.path)?
            }
        } else {
            let project = project_path.ok_or_else(|| {
                "Open a project folder to save the PDF in this access mode".to_string()
            })?;
            resolve_project_path(&project, &action.path)?
        };
        return create_pdf(&target, title, content);
    }
    if action.name == "fetch_url" {
        return fetch_https(
            action
                .url
                .as_deref()
                .ok_or_else(|| "The requested URL is missing".to_string())?,
        )
        .await;
    }
    let full_access = access_mode == "full";
    if action.name == "search_project" {
        let root = if full_access {
            if action.path.trim().is_empty() {
                let project = project_path.as_deref().ok_or_else(|| {
                    "Provide an absolute folder path to search when no project is open".to_string()
                })?;
                resolve_full_path(project, true)?
            } else {
                resolve_full_path(&action.path, true)?
            }
        } else if action.path.trim().is_empty() {
            let project = project_path
                .as_deref()
                .ok_or_else(|| "Open a project folder before searching its files".to_string())?;
            Path::new(project)
                .canonicalize()
                .map_err(|error| format!("Could not open project folder: {error}"))?
        } else {
            let project = project_path
                .as_deref()
                .ok_or_else(|| "Open a project folder before searching its files".to_string())?;
            resolve_project_path(project, &action.path)?
        };
        if !root.is_dir() {
            return Err("The search location is not a folder".to_string());
        }
        return search_project_text(&root, action.query.as_deref().unwrap_or_default());
    }
    if action.name == "list_directory" {
        let target = if full_access {
            resolve_full_path(&action.path, true)?
        } else {
            let project = project_path.ok_or_else(|| {
                "Open a project folder to browse it in this access mode".to_string()
            })?;
            resolve_project_path(&project, &action.path)?
        };
        if !target.is_dir() {
            return Err("The selected path is not a folder".to_string());
        }
        return list_directory(&target, full_access);
    }
    let target = if full_access {
        if action.name == "write_file" {
            resolve_full_write_path(&action.path)?
        } else {
            resolve_full_path(&action.path, false)?
        }
    } else {
        let project = project_path
            .ok_or_else(|| "Open a project folder to access files in this mode".to_string())?;
        resolve_project_path(&project, &action.path)?
    };
    match action.name.as_str() {
        "read_file" => {
            if !target.exists() {
                return Err("The requested file does not exist".to_string());
            }
            read_agent_file_chunk(&target, action.start_line, action.end_line)
        }
        "write_file" => {
            let content = action
                .content
                .ok_or_else(|| "The requested file content is missing".to_string())?;
            fs::write(&target, &content)
                .map_err(|error| format!("Could not write the file: {error}"))?;
            let saved = fs::read(&target)
                .map_err(|error| format!("File was written but could not be verified: {error}"))?;
            if saved != content.as_bytes() {
                return Err(
                    "File write verification failed: saved content does not match".to_string(),
                );
            }
            Ok(format!(
                "File written and verified ({} bytes): {}",
                saved.len(),
                target.display()
            ))
        }
        _ => Err("Unsupported agent action".to_string()),
    }
}
