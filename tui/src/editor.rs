use std::env;
use std::fs;
use std::io;
use std::process::Command;

/// Determine which editor to use.  $EDITOR → nvim → vi.
fn resolve_editor() -> String {
    if let Ok(editor) = env::var("EDITOR") {
        if !editor.is_empty() {
            return editor;
        }
    }
    if Command::new("nvim").arg("--version").output().is_ok() {
        return "nvim".to_string();
    }
    "vi".to_string()
}

/// Write `content` to a temp file, open it in the user's editor, and return the
/// edited content.  The caller is responsible for suspending / restoring the TUI.
pub fn edit_in_external(content: &str, file_extension: &str) -> io::Result<String> {
    let tmp_path = env::temp_dir().join(format!("yan_edit_{}.{}", std::process::id(), file_extension));

    fs::write(&tmp_path, content)?;

    let editor = resolve_editor();
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let mut cmd = Command::new(program);
    for arg in parts {
        cmd.arg(arg);
    }
    cmd.arg(&tmp_path);

    let status = cmd.status().map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        io::Error::new(io::ErrorKind::Other, format!("Failed to spawn editor '{}': {}", editor, e))
    })?;

    let result = fs::read_to_string(&tmp_path);
    let _ = fs::remove_file(&tmp_path);

    if !status.success() {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("Editor exited with status: {}", status),
        ));
    }

    result
}
