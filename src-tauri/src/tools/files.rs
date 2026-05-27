use crate::conversation_store;
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const DEFAULT_LIST_MAX_ENTRIES: usize = 200;
const MAX_LIST_MAX_ENTRIES: usize = 1_000;
const DEFAULT_READ_MAX_BYTES: usize = 32 * 1024;
const MAX_READ_MAX_BYTES: usize = 256 * 1024;
const LIST_OUTPUT_CHAR_BUDGET: usize = 48 * 1024;
const TEXT_DETECTION_SAMPLE_BYTES: usize = 8 * 1024;

/// Shared result for workspace path resolution so file-oriented tools can work
/// with normalized relative paths while still reading/writing absolute paths on
/// disk inside the active conversation workspace.
#[derive(Debug, Clone)]
struct ResolvedWorkspacePath {
    workspace_dir: PathBuf,
    relative_path: PathBuf,
    absolute_path: PathBuf,
}

#[derive(Debug, Deserialize)]
struct ReadFileArgs {
    #[serde(alias = "filename")]
    path: String,
    #[serde(default)]
    max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct WriteFileArgs {
    #[serde(alias = "filename")]
    path: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ListFilesArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    recursive: bool,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    max_entries: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct StatFileArgs {
    #[serde(alias = "filename")]
    path: String,
}

#[derive(Debug, Deserialize)]
struct MkdirArgs {
    path: String,
    #[serde(default = "default_true")]
    recursive: bool,
}

#[derive(Debug, Deserialize)]
struct ReplaceTextArgs {
    #[serde(alias = "filename")]
    path: String,
    old_text: String,
    new_text: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Debug, Deserialize)]
struct ReplaceBlockArgs {
    #[serde(alias = "filename")]
    path: String,
    old_block: String,
    new_block: String,
    #[serde(default)]
    replace_all: bool,
}

#[derive(Debug, Deserialize)]
struct InsertAfterArgs {
    #[serde(alias = "filename")]
    path: String,
    anchor: String,
    content: String,
    #[serde(default)]
    insert_all: bool,
}

#[derive(Debug, Deserialize)]
struct InsertBeforeArgs {
    #[serde(alias = "filename")]
    path: String,
    anchor: String,
    content: String,
    #[serde(default)]
    insert_all: bool,
}

#[derive(Debug, Deserialize)]
struct ApplyPatchArgs {
    #[serde(alias = "filename")]
    path: String,
    edits: Vec<TextPatchEdit>,
}

/// Structured patch operations are intentionally deterministic so the agent can
/// describe exact textual intent and the app can later build undo/history on
/// top of the same edit model.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum TextPatchEdit {
    ReplaceText {
        old_text: String,
        new_text: String,
        #[serde(default)]
        replace_all: bool,
    },
    ReplaceBlock {
        old_block: String,
        new_block: String,
        #[serde(default)]
        replace_all: bool,
    },
    InsertAfter {
        anchor: String,
        content: String,
        #[serde(default)]
        insert_all: bool,
    },
    InsertBefore {
        anchor: String,
        content: String,
        #[serde(default)]
        insert_all: bool,
    },
}

#[derive(Debug, Clone)]
struct TextEditOutcome {
    content: String,
    occurrences_changed: usize,
}

#[derive(Debug, Clone)]
struct TextFileRead {
    content: String,
    total_bytes: usize,
    returned_bytes: usize,
    truncated: bool,
}

#[derive(Debug, Clone, Default)]
struct ListCollectionState {
    output_chars: usize,
    hit_entry_limit: bool,
    hit_output_limit: bool,
}

impl ListCollectionState {
    fn is_truncated(&self) -> bool {
        self.hit_entry_limit || self.hit_output_limit
    }

    fn truncation_reasons(&self) -> Vec<&'static str> {
        let mut reasons = Vec::new();
        if self.hit_entry_limit {
            reasons.push("max_entries");
        }
        if self.hit_output_limit {
            reasons.push("max_output_chars");
        }
        reasons
    }
}

pub struct FileReaderTool;
pub struct FileWriterTool;
pub struct ListFilesTool;
pub struct StatFileTool;
pub struct MkdirTool;
pub struct ReplaceTextTool;
pub struct ReplaceBlockTool;
pub struct ApplyPatchTool;
pub struct InsertAfterTool;
pub struct InsertBeforeTool;

fn default_true() -> bool {
    true
}

/// Ensures the active conversation workspace exists before any file tool
/// touches disk.
fn get_workspace_dir(context: &ToolExecutionContext) -> Result<PathBuf, String> {
    let dir = if let Some(conversation_id) = context
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        conversation_store::conversation_workspace_path(conversation_id)?
    } else {
        return Err(
            "Missing conversation context for file tool. File tools require a conversation workspace."
                .to_string(),
        );
    };

    if !dir.exists() {
        fs::create_dir_all(&dir).map_err(|err| {
            format!(
                "Failed to create workspace directory {}: {err}",
                dir.display()
            )
        })?;
    }

    Ok(dir)
}

/// Normalizes a user-provided relative path and rejects absolute paths or any
/// `..` traversal that would escape the workspace root.
fn normalize_relative_path(raw_path: &str, allow_workspace_root: bool) -> Result<PathBuf, String> {
    let trimmed = raw_path.trim();
    if trimmed.is_empty() {
        return if allow_workspace_root {
            Ok(PathBuf::new())
        } else {
            Err("Path cannot be empty".to_string())
        };
    }

    let candidate = Path::new(trimmed);
    if candidate.is_absolute() {
        return Err("Absolute paths are not allowed; use a workspace-relative path".to_string());
    }

    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(segment) => normalized.push(segment),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(format!(
                        "Path '{}' escapes the conversation workspace",
                        raw_path
                    ));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(
                    "Absolute paths are not allowed; use a workspace-relative path".to_string(),
                )
            }
        }
    }

    if normalized.as_os_str().is_empty() && !allow_workspace_root {
        return Err(
            "Path resolves to the workspace root; provide a file or directory path".to_string(),
        );
    }

    Ok(normalized)
}

/// Resolves a workspace-relative path to an absolute path on disk while
/// preserving the normalized relative form for tool output.
fn resolve_workspace_path(
    raw_path: &str,
    context: &ToolExecutionContext,
    allow_workspace_root: bool,
) -> Result<ResolvedWorkspacePath, String> {
    let workspace_dir = get_workspace_dir(context)?;
    let relative_path = normalize_relative_path(raw_path, allow_workspace_root)?;
    let absolute_path = workspace_dir.join(&relative_path);

    Ok(ResolvedWorkspacePath {
        workspace_dir,
        relative_path,
        absolute_path,
    })
}

fn relative_path_display(path: &Path) -> String {
    if path.as_os_str().is_empty() {
        ".".to_string()
    } else {
        path.to_string_lossy().to_string()
    }
}

fn ensure_parent_dir_exists(path: &Path) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };

    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Failed to create parent directory {}: {err}",
                parent.display()
            )
        })?;
    }

    Ok(())
}

fn parse_tool_args<T>(arguments: &Value, tool_name: &str) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_value(arguments.clone())
        .map_err(|err| format!("Invalid arguments for tool '{tool_name}': {err}"))
}

fn known_binary_extension(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let is_binary = matches!(
        ext.as_str(),
        "7z" | "a"
            | "apk"
            | "avi"
            | "bin"
            | "bmp"
            | "class"
            | "db"
            | "dll"
            | "doc"
            | "docx"
            | "dylib"
            | "eot"
            | "exe"
            | "flac"
            | "gif"
            | "gz"
            | "icns"
            | "ico"
            | "jar"
            | "jpeg"
            | "jpg"
            | "lib"
            | "lockb"
            | "m4a"
            | "mov"
            | "mp3"
            | "mp4"
            | "o"
            | "obj"
            | "ogg"
            | "otf"
            | "pdf"
            | "png"
            | "ppt"
            | "pptx"
            | "psd"
            | "rar"
            | "so"
            | "sqlite"
            | "tar"
            | "tiff"
            | "ttf"
            | "wasm"
            | "wav"
            | "webm"
            | "webp"
            | "woff"
            | "woff2"
            | "xls"
            | "xlsx"
            | "xz"
            | "zip"
    );

    if is_binary {
        Some(ext)
    } else {
        None
    }
}

fn detect_binary_reason_from_bytes(bytes: &[u8]) -> Option<&'static str> {
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

fn detect_non_text_file(path: &Path) -> Result<Option<String>, String> {
    if let Some(ext) = known_binary_extension(path) {
        return Ok(Some(format!(
            "files ending in '.{ext}' are treated as binary"
        )));
    }

    let metadata = fs::metadata(path)
        .map_err(|err| format!("Failed to stat file {}: {err}", path.display()))?;
    if !metadata.is_file() {
        return Ok(None);
    }

    let sample_len = usize::try_from(metadata.len())
        .unwrap_or(TEXT_DETECTION_SAMPLE_BYTES)
        .min(TEXT_DETECTION_SAMPLE_BYTES);
    if sample_len == 0 {
        return Ok(None);
    }

    let file = fs::File::open(path)
        .map_err(|err| format!("Failed to open file {}: {err}", path.display()))?;
    let mut sample = Vec::with_capacity(sample_len);
    file.take(sample_len as u64)
        .read_to_end(&mut sample)
        .map_err(|err| format!("Failed to inspect file {}: {err}", path.display()))?;

    Ok(detect_binary_reason_from_bytes(&sample).map(str::to_string))
}

fn ensure_text_file(path: &Path, operation: &str) -> Result<(), String> {
    if let Some(reason) = detect_non_text_file(path)? {
        return Err(format!(
            "Refusing to {operation} '{}' because it appears to be a non-text/binary file ({reason})",
            path.display()
        ));
    }

    Ok(())
}

fn ensure_text_path_for_write(path: &Path) -> Result<(), String> {
    if path.exists() {
        ensure_text_file(path, "write")?;
    } else if let Some(ext) = known_binary_extension(path) {
        return Err(format!(
            "Refusing to write '{}' because files ending in '.{ext}' are treated as binary",
            path.display()
        ));
    }

    Ok(())
}

fn truncate_to_utf8_boundary(bytes: &[u8], max_bytes: usize) -> Result<&[u8], String> {
    let mut end = bytes.len().min(max_bytes);
    while end > 0 && std::str::from_utf8(&bytes[..end]).is_err() {
        end -= 1;
    }

    if end == 0 && !bytes.is_empty() {
        return Err("File preview could not be truncated to a valid UTF-8 boundary".to_string());
    }

    Ok(&bytes[..end])
}

fn read_text_file(path: &Path, max_bytes: usize, operation: &str) -> Result<TextFileRead, String> {
    ensure_text_file(path, operation)?;

    let metadata = fs::metadata(path)
        .map_err(|err| format!("Failed to stat file {}: {err}", path.display()))?;
    let total_bytes = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let read_limit = max_bytes.saturating_add(4);
    let bytes_to_read = total_bytes.min(read_limit);

    let file = fs::File::open(path)
        .map_err(|err| format!("Failed to open file {}: {err}", path.display()))?;
    let mut buffer = Vec::with_capacity(bytes_to_read);
    file.take(bytes_to_read as u64)
        .read_to_end(&mut buffer)
        .map_err(|err| format!("Failed to read file {}: {err}", path.display()))?;

    let preview = truncate_to_utf8_boundary(&buffer, max_bytes)?;
    let content = std::str::from_utf8(preview)
        .map_err(|err| format!("Failed to decode file {} as UTF-8: {err}", path.display()))?
        .to_string();

    Ok(TextFileRead {
        returned_bytes: preview.len(),
        total_bytes,
        truncated: total_bytes > preview.len(),
        content,
    })
}

fn write_text_file(path: &Path, content: &str) -> Result<(), String> {
    ensure_text_path_for_write(path)?;
    ensure_parent_dir_exists(path)?;
    fs::write(path, content)
        .map_err(|err| format!("Failed to write file {}: {err}", path.display()))
}

fn count_lines(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

fn system_time_to_unix_ms(time: Result<SystemTime, std::io::Error>) -> Option<i64> {
    time.ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_millis() as i64)
}

fn is_hidden_name(name: &str) -> bool {
    name.starts_with('.')
}

fn metadata_kind(metadata: &fs::Metadata) -> &'static str {
    if metadata.is_dir() {
        "directory"
    } else if metadata.is_file() {
        "file"
    } else {
        "other"
    }
}

fn stat_value(path: &Path, metadata: &fs::Metadata) -> Value {
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

fn collect_directory_entries(
    workspace_dir: &Path,
    current_dir: &Path,
    recursive: bool,
    include_hidden: bool,
    max_entries: usize,
    entries: &mut Vec<Value>,
    state: &mut ListCollectionState,
) -> Result<(), String> {
    let mut children = Vec::new();
    for entry in fs::read_dir(current_dir)
        .map_err(|err| format!("Failed to list directory {}: {err}", current_dir.display()))?
    {
        let entry = entry.map_err(|err| {
            format!(
                "Failed to inspect directory entry {}: {err}",
                current_dir.display()
            )
        })?;
        children.push(entry);
    }

    children.sort_by_key(|entry| entry.path());

    for entry in children {
        if entries.len() >= max_entries {
            state.hit_entry_limit = true;
            break;
        }

        let path = entry.path();
        let relative = path
            .strip_prefix(workspace_dir)
            .map_err(|err| format!("Failed to derive workspace-relative path: {err}"))?
            .to_path_buf();

        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if !include_hidden && is_hidden_name(&name) {
            continue;
        }

        let metadata = entry
            .metadata()
            .map_err(|err| format!("Failed to read metadata for {}: {err}", path.display()))?;
        let value = stat_value(&relative, &metadata);
        let estimated_chars = value.to_string().chars().count();
        if !entries.is_empty() && state.output_chars + estimated_chars > LIST_OUTPUT_CHAR_BUDGET {
            state.hit_output_limit = true;
            break;
        }

        state.output_chars += estimated_chars;
        entries.push(value);

        if recursive && metadata.is_dir() {
            collect_directory_entries(
                workspace_dir,
                &path,
                true,
                include_hidden,
                max_entries,
                entries,
                state,
            )?;
            if state.is_truncated() {
                break;
            }
        }
    }

    Ok(())
}

fn count_occurrences(content: &str, needle: &str, label: &str) -> Result<usize, String> {
    if needle.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }

    Ok(content.match_indices(needle).count())
}

fn replace_exact_text(
    content: &str,
    needle: &str,
    replacement: &str,
    replace_all: bool,
    label: &str,
) -> Result<TextEditOutcome, String> {
    let occurrences = count_occurrences(content, needle, label)?;
    if occurrences == 0 {
        return Err(format!("Could not find the requested {label} in the file"));
    }

    if !replace_all && occurrences > 1 {
        return Err(format!(
            "Found {occurrences} matches for the requested {label}; rerun with replace_all=true to update every occurrence"
        ));
    }

    let updated = if replace_all {
        content.replace(needle, replacement)
    } else {
        content.replacen(needle, replacement, 1)
    };

    Ok(TextEditOutcome {
        content: updated,
        occurrences_changed: if replace_all { occurrences } else { 1 },
    })
}

fn insert_relative_to_anchor(
    content: &str,
    anchor: &str,
    insertion: &str,
    insert_after: bool,
    insert_all: bool,
    label: &str,
) -> Result<TextEditOutcome, String> {
    let occurrences = count_occurrences(content, anchor, "anchor text")?;
    if occurrences == 0 {
        return Err(format!(
            "Could not find the requested anchor text for {label}"
        ));
    }

    if !insert_all && occurrences > 1 {
        return Err(format!(
            "Found {occurrences} anchor matches for {label}; rerun with insert_all=true to apply the insertion at every match"
        ));
    }

    let updated = if insert_all {
        if insert_after {
            content.replace(anchor, &format!("{anchor}{insertion}"))
        } else {
            content.replace(anchor, &format!("{insertion}{anchor}"))
        }
    } else {
        let index = content
            .find(anchor)
            .ok_or_else(|| format!("Could not find the requested anchor text for {label}"))?;
        let split_index = if insert_after {
            index + anchor.len()
        } else {
            index
        };
        let mut next = String::with_capacity(content.len() + insertion.len());
        next.push_str(&content[..split_index]);
        next.push_str(insertion);
        next.push_str(&content[split_index..]);
        next
    };

    Ok(TextEditOutcome {
        content: updated,
        occurrences_changed: if insert_all { occurrences } else { 1 },
    })
}

fn apply_single_text_patch(content: &str, edit: &TextPatchEdit) -> Result<TextEditOutcome, String> {
    match edit {
        TextPatchEdit::ReplaceText {
            old_text,
            new_text,
            replace_all,
        } => replace_exact_text(content, old_text, new_text, *replace_all, "old_text"),
        TextPatchEdit::ReplaceBlock {
            old_block,
            new_block,
            replace_all,
        } => replace_exact_text(content, old_block, new_block, *replace_all, "old_block"),
        TextPatchEdit::InsertAfter {
            anchor,
            content: insertion,
            insert_all,
        } => insert_relative_to_anchor(
            content,
            anchor,
            insertion,
            true,
            *insert_all,
            "insert_after",
        ),
        TextPatchEdit::InsertBefore {
            anchor,
            content: insertion,
            insert_all,
        } => insert_relative_to_anchor(
            content,
            anchor,
            insertion,
            false,
            *insert_all,
            "insert_before",
        ),
    }
}

fn apply_text_patch_plan(
    content: &str,
    edits: &[TextPatchEdit],
) -> Result<(String, Vec<Value>), String> {
    if edits.is_empty() {
        return Err("Patch must contain at least one edit".to_string());
    }

    let mut next_content = content.to_string();
    let mut details = Vec::with_capacity(edits.len());

    for (index, edit) in edits.iter().enumerate() {
        let outcome = apply_single_text_patch(&next_content, edit)
            .map_err(|err| format!("Patch edit {} failed: {err}", index + 1))?;
        next_content = outcome.content;
        details.push(json!({
            "index": index + 1,
            "op": patch_operation_name(edit),
            "occurrencesChanged": outcome.occurrences_changed
        }));
    }

    Ok((next_content, details))
}

fn patch_operation_name(edit: &TextPatchEdit) -> &'static str {
    match edit {
        TextPatchEdit::ReplaceText { .. } => "replace_text",
        TextPatchEdit::ReplaceBlock { .. } => "replace_block",
        TextPatchEdit::InsertAfter { .. } => "insert_after",
        TextPatchEdit::InsertBefore { .. } => "insert_before",
    }
}

impl Tool for FileReaderTool {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn display_name(&self) -> &'static str {
        "Read File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FileSearch")
    }

    fn description(&self) -> &'static str {
        "Reads a UTF-8 text file preview from the current conversation workspace. Large files are truncated, and binary files are rejected."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path to read, e.g. 'src/components/Sidebar.tsx'."
                },
                "filename": {
                    "type": "string",
                    "description": "Backward-compatible alias for 'path'."
                },
                "max_bytes": {
                    "type": "integer",
                    "description": "Optional maximum number of UTF-8 bytes to return. Defaults to 32768 and is capped at 262144."
                }
            },
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<ReadFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        if !resolved.absolute_path.exists() {
            return Err(format!(
                "File '{}' not found in current conversation workspace",
                relative_path_display(&resolved.relative_path)
            ));
        }

        let max_bytes = args
            .max_bytes
            .unwrap_or(DEFAULT_READ_MAX_BYTES)
            .clamp(1, MAX_READ_MAX_BYTES);
        let text = read_text_file(&resolved.absolute_path, max_bytes, "read")?;
        let line_count = count_lines(&text.content);
        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "content": text.content,
            "bytesRead": text.returned_bytes,
            "totalBytes": text.total_bytes,
            "lineCount": line_count,
            "truncated": text.truncated,
            "maxBytes": max_bytes,
        }))
    }
}

impl Tool for FileWriterTool {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn display_name(&self) -> &'static str {
        "Write File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Writes UTF-8 text to a workspace-relative file. Creates missing parent directories, overwrites existing text files, and rejects binary targets."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file path to write, e.g. 'notes/output.txt'."
                },
                "filename": {
                    "type": "string",
                    "description": "Backward-compatible alias for 'path'."
                },
                "content": {
                    "type": "string",
                    "description": "Complete UTF-8 file contents to write."
                }
            },
            "required": ["content"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<WriteFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        write_text_file(&resolved.absolute_path, &args.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "bytesWritten": args.content.as_bytes().len(),
            "lineCount": count_lines(&args.content),
            "status": "success"
        }))
    }
}

impl Tool for ListFilesTool {
    fn name(&self) -> &'static str {
        "list_files"
    }

    fn display_name(&self) -> &'static str {
        "List Files"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FolderOpen")
    }

    fn description(&self) -> &'static str {
        "Lists files and directories inside the conversation workspace. Supports nested paths, optional recursion, and truncates oversized directory results."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional workspace-relative directory path to list. Defaults to the workspace root."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to recursively include nested directory contents."
                },
                "include_hidden": {
                    "type": "boolean",
                    "description": "Whether to include hidden entries whose names start with '.'."
                },
                "max_entries": {
                    "type": "integer",
                    "description": "Maximum number of entries to return. Defaults to 200 and is capped at 1000."
                }
            }
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<ListFilesArgs>(arguments, self.name())?;
        let target = args.path.unwrap_or_else(|| ".".to_string());
        let resolved = resolve_workspace_path(&target, context, true)?;
        if !resolved.absolute_path.exists() {
            return Err(format!(
                "Directory '{}' not found in current conversation workspace",
                relative_path_display(&resolved.relative_path)
            ));
        }

        let metadata = fs::metadata(&resolved.absolute_path)
            .map_err(|err| format!("Failed to stat {}: {err}", resolved.absolute_path.display()))?;
        if !metadata.is_dir() {
            return Err(format!(
                "Path '{}' is not a directory",
                relative_path_display(&resolved.relative_path)
            ));
        }

        let max_entries = args
            .max_entries
            .unwrap_or(DEFAULT_LIST_MAX_ENTRIES)
            .clamp(1, MAX_LIST_MAX_ENTRIES);
        let mut entries = Vec::new();
        let mut state = ListCollectionState::default();
        collect_directory_entries(
            &resolved.workspace_dir,
            &resolved.absolute_path,
            args.recursive,
            args.include_hidden,
            max_entries,
            &mut entries,
            &mut state,
        )?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "recursive": args.recursive,
            "includeHidden": args.include_hidden,
            "maxEntries": max_entries,
            "truncated": state.is_truncated(),
            "truncationReasons": state.truncation_reasons(),
            "entryCount": entries.len(),
            "approxOutputChars": state.output_chars,
            "entries": entries,
        }))
    }
}

impl Tool for StatFileTool {
    fn name(&self) -> &'static str {
        "stat_file"
    }

    fn display_name(&self) -> &'static str {
        "Stat File"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Info")
    }

    fn description(&self) -> &'static str {
        "Returns metadata for a workspace-relative file or directory, including type, size, permissions, timestamps, and whether a file looks text-readable."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative file or directory path to inspect."
                },
                "filename": {
                    "type": "string",
                    "description": "Backward-compatible alias for 'path'."
                }
            },
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<StatFileArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, true)?;
        let metadata = fs::metadata(&resolved.absolute_path).map_err(|err| {
            format!(
                "Failed to stat '{}' in current conversation workspace: {err}",
                relative_path_display(&resolved.relative_path)
            )
        })?;
        let mut value = stat_value(&resolved.relative_path, &metadata);
        if metadata.is_file() {
            let (content_kind, text_readable, reason) =
                match detect_non_text_file(&resolved.absolute_path)? {
                    Some(reason) => ("binary", false, Some(reason)),
                    None => ("text", true, None),
                };

            if let Some(object) = value.as_object_mut() {
                object.insert("contentKind".to_string(), json!(content_kind));
                object.insert("textReadable".to_string(), json!(text_readable));
                object.insert("contentKindReason".to_string(), json!(reason));
            }
        }

        Ok(value)
    }
}

impl Tool for MkdirTool {
    fn name(&self) -> &'static str {
        "mkdir"
    }

    fn display_name(&self) -> &'static str {
        "Make Directory"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FolderPlus")
    }

    fn description(&self) -> &'static str {
        "Creates a directory inside the conversation workspace. Supports nested paths and recursive creation."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Workspace-relative directory path to create."
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to create all missing parent directories. Defaults to true."
                }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<MkdirArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let existed_before = resolved.absolute_path.exists();

        if args.recursive {
            fs::create_dir_all(&resolved.absolute_path).map_err(|err| {
                format!(
                    "Failed to create directory {}: {err}",
                    resolved.absolute_path.display()
                )
            })?;
        } else {
            fs::create_dir(&resolved.absolute_path).map_err(|err| {
                format!(
                    "Failed to create directory {}: {err}",
                    resolved.absolute_path.display()
                )
            })?;
        }

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "created": !existed_before,
            "alreadyExisted": existed_before,
            "recursive": args.recursive,
        }))
    }
}

impl Tool for ReplaceTextTool {
    fn name(&self) -> &'static str {
        "replace_text"
    }

    fn display_name(&self) -> &'static str {
        "Replace Text"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Replaces exact text in a workspace text file. By default it requires a unique match, making edits deterministic and undo-friendly."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Backward-compatible alias for 'path'." },
                "old_text": { "type": "string", "description": "Exact text to find." },
                "new_text": { "type": "string", "description": "Replacement text." },
                "replace_all": { "type": "boolean", "description": "Whether to replace every exact match instead of requiring uniqueness." }
            },
            "required": ["old_text", "new_text"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<ReplaceTextArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let outcome = replace_exact_text(
            &original.content,
            &args.old_text,
            &args.new_text,
            args.replace_all,
            "old_text",
        )?;
        write_text_file(&resolved.absolute_path, &outcome.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "occurrencesChanged": outcome.occurrences_changed,
            "bytesWritten": outcome.content.as_bytes().len(),
            "lineCount": count_lines(&outcome.content),
            "status": "success"
        }))
    }
}

impl Tool for ReplaceBlockTool {
    fn name(&self) -> &'static str {
        "replace_block"
    }

    fn display_name(&self) -> &'static str {
        "Replace Block"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Replaces an exact multi-line block in a workspace text file. By default it requires a unique match for predictable patching."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Backward-compatible alias for 'path'." },
                "old_block": { "type": "string", "description": "Exact text block to replace." },
                "new_block": { "type": "string", "description": "Replacement text block." },
                "replace_all": { "type": "boolean", "description": "Whether to replace every exact block match instead of requiring uniqueness." }
            },
            "required": ["old_block", "new_block"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<ReplaceBlockArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let outcome = replace_exact_text(
            &original.content,
            &args.old_block,
            &args.new_block,
            args.replace_all,
            "old_block",
        )?;
        write_text_file(&resolved.absolute_path, &outcome.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "occurrencesChanged": outcome.occurrences_changed,
            "bytesWritten": outcome.content.as_bytes().len(),
            "lineCount": count_lines(&outcome.content),
            "status": "success"
        }))
    }
}

impl Tool for InsertAfterTool {
    fn name(&self) -> &'static str {
        "insert_after"
    }

    fn display_name(&self) -> &'static str {
        "Insert After"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Inserts text immediately after an exact anchor string in a workspace text file. By default the anchor must be unique."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Backward-compatible alias for 'path'." },
                "anchor": { "type": "string", "description": "Exact anchor text to insert after." },
                "content": { "type": "string", "description": "Text to insert." },
                "insert_all": { "type": "boolean", "description": "Whether to insert after every anchor match instead of requiring uniqueness." }
            },
            "required": ["anchor", "content"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<InsertAfterArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let outcome = insert_relative_to_anchor(
            &original.content,
            &args.anchor,
            &args.content,
            true,
            args.insert_all,
            self.name(),
        )?;
        write_text_file(&resolved.absolute_path, &outcome.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "occurrencesChanged": outcome.occurrences_changed,
            "bytesWritten": outcome.content.as_bytes().len(),
            "lineCount": count_lines(&outcome.content),
            "status": "success"
        }))
    }
}

impl Tool for InsertBeforeTool {
    fn name(&self) -> &'static str {
        "insert_before"
    }

    fn display_name(&self) -> &'static str {
        "Insert Before"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Inserts text immediately before an exact anchor string in a workspace text file. By default the anchor must be unique."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Backward-compatible alias for 'path'." },
                "anchor": { "type": "string", "description": "Exact anchor text to insert before." },
                "content": { "type": "string", "description": "Text to insert." },
                "insert_all": { "type": "boolean", "description": "Whether to insert before every anchor match instead of requiring uniqueness." }
            },
            "required": ["anchor", "content"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<InsertBeforeArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let outcome = insert_relative_to_anchor(
            &original.content,
            &args.anchor,
            &args.content,
            false,
            args.insert_all,
            self.name(),
        )?;
        write_text_file(&resolved.absolute_path, &outcome.content)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "occurrencesChanged": outcome.occurrences_changed,
            "bytesWritten": outcome.content.as_bytes().len(),
            "lineCount": count_lines(&outcome.content),
            "status": "success"
        }))
    }
}

impl Tool for ApplyPatchTool {
    fn name(&self) -> &'static str {
        "apply_patch"
    }

    fn display_name(&self) -> &'static str {
        "Apply Patch"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FilePenLine")
    }

    fn description(&self) -> &'static str {
        "Applies a deterministic sequence of structured text edits to a workspace text file. Use this when multiple replace/insert operations should land atomically."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Workspace-relative file path to edit." },
                "filename": { "type": "string", "description": "Backward-compatible alias for 'path'." },
                "edits": {
                    "type": "array",
                    "description": "Ordered patch edits. Supported op values: replace_text, replace_block, insert_after, insert_before.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "op": {
                                "type": "string",
                                "enum": ["replace_text", "replace_block", "insert_after", "insert_before"]
                            },
                            "old_text": { "type": "string" },
                            "new_text": { "type": "string" },
                            "old_block": { "type": "string" },
                            "new_block": { "type": "string" },
                            "anchor": { "type": "string" },
                            "content": { "type": "string" },
                            "replace_all": { "type": "boolean" },
                            "insert_all": { "type": "boolean" }
                        },
                        "required": ["op"]
                    }
                }
            },
            "required": ["edits"],
            "anyOf": [
                { "required": ["path"] },
                { "required": ["filename"] }
            ]
        })
    }

    fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> Result<Value, String> {
        let args = parse_tool_args::<ApplyPatchArgs>(arguments, self.name())?;
        let resolved = resolve_workspace_path(&args.path, context, false)?;
        let original = read_text_file(&resolved.absolute_path, MAX_READ_MAX_BYTES, "edit")?;
        let (patched, details) = apply_text_patch_plan(&original.content, &args.edits)?;
        write_text_file(&resolved.absolute_path, &patched)?;

        Ok(json!({
            "path": relative_path_display(&resolved.relative_path),
            "operationsApplied": details.len(),
            "bytesWritten": patched.as_bytes().len(),
            "lineCount": count_lines(&patched),
            "details": details,
            "status": "success"
        }))
    }
}
