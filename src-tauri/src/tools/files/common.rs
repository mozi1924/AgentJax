use crate::agentjax_err;
use crate::conversation_store;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::tools::ToolExecutionContext;
use file_format::{FileFormat, Kind as FileFormatKind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const DEFAULT_READ_MAX_BYTES: usize = 32 * 1024;
pub const MAX_READ_MAX_BYTES: usize = 256 * 1024;
pub const TEXT_DETECTION_SAMPLE_BYTES: usize = 8 * 1024;

/// Shared result for workspace path resolution so file-oriented tools can work
/// with normalized relative paths while still reading/writing absolute paths on
/// disk inside the active conversation workspace.
#[derive(Debug, Clone)]
pub struct ResolvedWorkspacePath {
    pub workspace_dir: PathBuf,
    pub relative_path: PathBuf,
    pub absolute_path: PathBuf,
}

/// Centralized content-sniffing result reused by stat/read/write/edit tools so
/// every file-oriented path applies the same text-vs-binary policy and exposes
/// the same metadata for future multimodal routing.
#[derive(Debug, Clone)]
pub struct FileTypeDetection {
    pub detected_format: String,
    pub detected_short_name: Option<String>,
    pub media_type: String,
    pub detected_extension: String,
    pub format_kind: &'static str,
    pub content_kind: &'static str,
    pub text_readable: bool,
    pub content_kind_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TextFileRead {
    pub content: String,
    pub total_bytes: usize,
    pub returned_bytes: usize,
    pub truncated: bool,
    pub file_type: FileTypeDetection,
}

/// Ensures the active conversation workspace exists before any file tool
/// touches disk.
pub fn get_workspace_dir(context: &ToolExecutionContext) -> AgentJaxResult<PathBuf> {
    let dir = if let Some(conversation_id) = context
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        conversation_store::conversation_workspace_path(conversation_id)?
    } else {
        return Err(agentjax_err!(
            "Missing conversation context for file tool. File tools require a conversation workspace.",
            ToolExecution
        ));
    };

    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|err| {
            AgentJaxError::tool(format!(
                "Failed to create workspace directory {}: {err}",
                dir.display()
            ))
            .with_error_source(&err)
        })?;
    }

    Ok(dir)
}

/// Normalizes a user-provided relative path and rejects absolute paths or any
/// `..` traversal that would escape the workspace root.
pub fn normalize_relative_path(
    raw_path: &str,
    allow_workspace_root: bool,
) -> AgentJaxResult<PathBuf> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return if allow_workspace_root {
            Ok(PathBuf::new())
        } else {
            Err(agentjax_err!("Path cannot be empty", ToolExecution))
        };
    }

    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err(agentjax_err!(
            "Absolute paths are not allowed; use a workspace-relative path",
            ToolExecution
        ));
    }

    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(agentjax_err!(
                        format!("Path '{}' escapes the conversation workspace", raw_path),
                        ToolExecution
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(agentjax_err!(
                    "Absolute paths are not allowed; use a workspace-relative path",
                    ToolExecution
                ));
            }
        }
    }

    if normalized.as_os_str().is_empty() && !allow_workspace_root {
        return Err(agentjax_err!(
            "Path resolves to the workspace root; provide a file or directory path",
            ToolExecution
        ));
    }

    Ok(normalized)
}

/// Resolves a workspace-relative path to an absolute path on disk while
/// preserving the normalized relative form for tool output.
pub fn resolve_workspace_path(
    raw_path: &str,
    context: &ToolExecutionContext,
    allow_workspace_root: bool,
) -> AgentJaxResult<ResolvedWorkspacePath> {
    let workspace_dir = get_workspace_dir(context)?;
    let relative_path = normalize_relative_path(raw_path, allow_workspace_root)?;
    let absolute_path = workspace_dir.join(&relative_path);

    Ok(ResolvedWorkspacePath {
        workspace_dir,
        relative_path,
        absolute_path,
    })
}

pub fn relative_path_display(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

pub fn ensure_parent_dir_exists(path: &Path) -> AgentJaxResult<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|err| {
            AgentJaxError::tool(format!(
                "Failed to create parent directory {}: {err}",
                parent.display()
            ))
            .with_error_source(&err)
        })?;
    }

    Ok(())
}

pub fn parse_tool_args<T>(arguments: &Value, tool_name: &str) -> AgentJaxResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(arguments.clone())
        .map_err(|err| AgentJaxError::tool(format!("Invalid arguments for tool '{tool_name}': {err}")))
}

pub fn detect_binary_reason_from_bytes(bytes: &[u8]) -> Option<&'static str> {
    if bytes.contains(&0) {
        return Some("sample contains NUL bytes");
    }

    if std::str::from_utf8(bytes).is_err() {
        return Some("sample is not valid UTF-8");
    }

    let suspicious_controls = bytes
        .iter()
        .filter(|byte| matches!(**byte, 0x01..=0x08 | 0x0B | 0x0C | 0x0E..=0x1F | 0x7F))
        .count();
    if !bytes.is_empty() && suspicious_controls * 10 > bytes.len() {
        return Some("sample contains too many non-text control bytes");
    }

    None
}

pub fn read_file_sample(path: &Path, max_bytes: usize) -> AgentJaxResult<Vec<u8>> {
    let metadata = fs::metadata(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to stat file {}: {err}", path.display())).with_error_source(&err))?;
    if !metadata.is_file() {
        return Ok(Vec::new());
    }

    let sample_len = usize::try_from(metadata.len())
        .unwrap_or(max_bytes)
        .min(max_bytes);
    if sample_len == 0 || max_bytes == 0 {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to open file {}: {err}", path.display())).with_error_source(&err))?;
    let mut sample = Vec::with_capacity(sample_len);
    file.take(sample_len as u64)
        .read_to_end(&mut sample)
        .map_err(|err| AgentJaxError::tool(format!("Failed to inspect file {}: {err}", path.display())).with_error_source(&err))?;

    Ok(sample)
}

pub fn file_format_kind_label(kind: FileFormatKind) -> &'static str {
    match kind {
        FileFormatKind::Archive => "archive",
        FileFormatKind::Audio => "audio",
        FileFormatKind::Compressed => "compressed",
        FileFormatKind::Database => "database",
        FileFormatKind::Diagram => "diagram",
        FileFormatKind::Disk => "disk",
        FileFormatKind::Document => "document",
        FileFormatKind::Ebook => "ebook",
        FileFormatKind::Executable => "executable",
        FileFormatKind::Font => "font",
        FileFormatKind::Formula => "formula",
        FileFormatKind::Geospatial => "geospatial",
        FileFormatKind::Image => "image",
        FileFormatKind::Metadata => "metadata",
        FileFormatKind::Model => "model",
        FileFormatKind::Other => "other",
        FileFormatKind::Package => "package",
        FileFormatKind::Playlist => "playlist",
        FileFormatKind::Presentation => "presentation",
        FileFormatKind::Rom => "rom",
        FileFormatKind::Spreadsheet => "spreadsheet",
        FileFormatKind::Subtitle => "subtitle",
        FileFormatKind::Video => "video",
        _ => "other",
    }
}

pub fn media_type_is_textual(media_type: &str) -> bool {
    media_type.starts_with("text/")
        || matches!(
            media_type,
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/x-sh"
                | "application/x-shellscript"
        )
        || media_type.ends_with("+json")
        || media_type.ends_with("+xml")
}

pub fn format_is_text_candidate(format: FileFormat) -> bool {
    matches!(format, FileFormat::Empty | FileFormat::PlainText)
        || media_type_is_textual(format.media_type())
}

pub fn detect_file_type(path: &Path) -> AgentJaxResult<FileTypeDetection> {
    let metadata = fs::metadata(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to stat file {}: {err}", path.display())).with_error_source(&err))?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Ok(FileTypeDetection {
            detected_format: "Other".to_string(),
            detected_short_name: None,
            media_type: "inode/unknown".to_string(),
            detected_extension: String::new(),
            format_kind: "other",
            content_kind: "other",
            text_readable: false,
            content_kind_reason: Some(
                "File on disk is neither a directory nor a standard file".to_string(),
            ),
        });
    }

    if metadata.is_dir() {
        return Ok(FileTypeDetection {
            detected_format: "Directory".to_string(),
            detected_short_name: None,
            media_type: "inode/directory".to_string(),
            detected_extension: String::new(),
            format_kind: "directory",
            content_kind: "directory",
            text_readable: false,
            content_kind_reason: None,
        });
    }

    let file = fs::File::open(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to open file {}: {err}", path.display())).with_error_source(&err))?;
    let detected = FileFormat::from_reader(file)
        .map_err(|err| AgentJaxError::tool(format!("Failed to detect file type for {}: {err}", path.display())).with_error_source(&err))?;
    let sample = read_file_sample(path, TEXT_DETECTION_SAMPLE_BYTES)?;
    let sample_reason = detect_binary_reason_from_bytes(&sample);
    let text_candidate = format_is_text_candidate(detected);
    let text_readable = text_candidate && sample_reason.is_none();
    let content_kind_reason = if text_readable {
        None
    } else if detected == FileFormat::ArbitraryBinaryData {
        Some(
            sample_reason
                .map(|reason| {
                    format!("content probe classified the file as arbitrary binary data ({reason})")
                })
                .unwrap_or_else(|| {
                    "content probe classified the file as arbitrary binary data".to_string()
                }),
        )
    } else if text_candidate {
        Some(
            sample_reason
                .map(|reason| {
                    format!(
                        "content probe recognized {} ({}) but it is not UTF-8 text readable ({reason})",
                        detected.name(),
                        detected.media_type()
                    )
                })
                .unwrap_or_else(|| {
                    format!(
                        "content probe recognized {} ({}) but it is not UTF-8 text readable",
                        detected.name(),
                        detected.media_type()
                    )
                }),
        )
    } else {
        Some(format!(
            "content probe recognized {} ({})",
            detected.name(),
            detected.media_type()
        ))
    };

    Ok(FileTypeDetection {
        detected_format: detected.name().to_string(),
        detected_short_name: detected.short_name().map(str::to_string),
        media_type: detected.media_type().to_string(),
        detected_extension: detected.extension().to_string(),
        format_kind: file_format_kind_label(detected.kind()),
        content_kind: if text_readable { "text" } else { "binary" },
        text_readable,
        content_kind_reason,
    })
}

pub fn attach_file_type_metadata(
    object: &mut serde_json::Map<String, Value>,
    detection: &FileTypeDetection,
) {
    object.insert("contentKind".to_string(), json!(detection.content_kind));
    object.insert("textReadable".to_string(), json!(detection.text_readable));
    object.insert(
        "contentKindReason".to_string(),
        json!(detection.content_kind_reason),
    );
    object.insert(
        "detectedFormat".to_string(),
        json!(detection.detected_format),
    );
    object.insert(
        "detectedShortName".to_string(),
        json!(detection.detected_short_name),
    );
    object.insert("mediaType".to_string(), json!(detection.media_type));
    object.insert(
        "detectedExtension".to_string(),
        json!(detection.detected_extension),
    );
    object.insert("formatKind".to_string(), json!(detection.format_kind));
    object.insert("typeDetectionSource".to_string(), json!("content_sniffing"));
}

pub fn ensure_text_file(path: &Path, operation: &str) -> AgentJaxResult<()> {
    let detection = detect_file_type(path)?;
    if !detection.text_readable {
        return Err(agentjax_err!(
            format!(
                "Refusing to {operation} '{}' because it appears to be a non-text/binary file ({reason})",
                path.display(),
                reason = detection
                    .content_kind_reason
                    .as_deref()
                    .unwrap_or("content probe marked it as non-text")
            ),
            ToolExecution
        ));
    }

    Ok(())
}

pub fn ensure_text_path_for_write(path: &Path) -> AgentJaxResult<()> {
    if path.exists() {
        ensure_text_file(path, "write")?;
    }
    // 硬编码后缀拦截 logic was removed as requested.
    Ok(())
}

pub fn truncate_to_utf8_boundary(bytes: &[u8], max_bytes: usize) -> AgentJaxResult<&[u8]> {
    let mut end = bytes.len().min(max_bytes);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }

    if end == 0 && !bytes.is_empty() {
        return Err(agentjax_err!(
            "File preview could not be truncated to a valid UTF-8 boundary",
            ToolExecution
        ));
    }

    Ok(&bytes[..end])
}

pub fn read_text_file(
    path: &Path,
    max_bytes: usize,
    operation: &str,
) -> AgentJaxResult<TextFileRead> {
    let file_type = detect_file_type(path)?;
    if !file_type.text_readable {
        return Err(agentjax_err!(
            format!(
                "Refusing to {operation} '{}' because it appears to be a non-text/binary file ({reason})",
                path.display(),
                reason = file_type
                    .content_kind_reason
                    .as_deref()
                    .unwrap_or("content probe marked it as non-text")
            ),
            ToolExecution
        ));
    }

    let metadata = fs::metadata(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to stat file {}: {err}", path.display())).with_error_source(&err))?;
    let total_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let read_limit = max_bytes.saturating_add(4);
    let bytes_to_read = total_bytes.min(read_limit);

    let file = fs::File::open(path)
        .map_err(|err| AgentJaxError::tool(format!("Failed to open file {}: {err}", path.display())).with_error_source(&err))?;
    let mut buffer = Vec::with_capacity(bytes_to_read);
    file.take(bytes_to_read as u64)
        .read_to_end(&mut buffer)
        .map_err(|err| AgentJaxError::tool(format!("Failed to read file {}: {err}", path.display())).with_error_source(&err))?;

    let preview = truncate_to_utf8_boundary(&buffer, max_bytes)?;
    let content = std::str::from_utf8(preview)
        .map_err(|err| AgentJaxError::tool(format!("Failed to decode file {} as UTF-8: {err}", path.display())).with_error_source(&err))?
        .to_string();

    Ok(TextFileRead {
        returned_bytes: preview.len(),
        total_bytes,
        truncated: total_bytes > preview.len(),
        content,
        file_type,
    })
}

pub fn write_text_file(path: &Path, content: &str) -> AgentJaxResult<()> {
    ensure_text_path_for_write(path)?;
    ensure_parent_dir_exists(path)?;
    fs::write(path, content)
        .map_err(|err| AgentJaxError::tool(format!("Failed to write file {}: {err}", path.display())).with_error_source(&err))
}

pub fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

pub fn system_time_to_unix_ms(time: Result<SystemTime, std::io::Error>) -> Option<i64> {
    time.ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

pub fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

pub fn metadata_kind(metadata: &fs::Metadata) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
}

pub fn stat_value(path: &Path, metadata: &fs::Metadata) -> Value {
    let name = path
        .file_name()
        .and_then(|segment| segment.to_str())
        .map(str::to_string)
        .unwrap_or_else(|| relative_path_display(path));
    let hidden = is_hidden_name(&name);

    json!({
        "path": relative_path_display(path),
        "name": name,
        "kind": metadata_kind(metadata),
        "isFile": metadata.is_file(),
        "isDirectory": metadata.is_dir(),
        "sizeBytes": metadata.len(),
        "readonly": metadata.permissions().readonly(),
        "hidden": hidden,
        "createdTimeMs": system_time_to_unix_ms(metadata.created()),
        "modifiedTimeMs": system_time_to_unix_ms(metadata.modified()),
        "accessedTimeMs": system_time_to_unix_ms(metadata.accessed()),
    })
}
