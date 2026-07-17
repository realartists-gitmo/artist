use super::skill_tool::SkillError;
use std::{io::Read, path::Path};

pub const OUTPUT_CAP: usize = 128 * 1024;

pub fn read_bounded(path: &Path, base: &Path) -> Result<String, SkillError> {
    let mut file = std::fs::File::open(path).map_err(message)?;
    let metadata = file.metadata().map_err(message)?;
    if !metadata.is_file() || metadata.len() > OUTPUT_CAP as u64 {
        return Err(SkillError::Message(
            "resource is not a bounded regular file".into(),
        ));
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let opened = std::fs::canonicalize(format!("/proc/self/fd/{}", file.as_raw_fd()))
            .map_err(message)?;
        if !opened.starts_with(base) {
            return Err(SkillError::Message(
                "resource path escapes the skill".into(),
            ));
        }
    }
    let mut content = String::new();
    file.read_to_string(&mut content).map_err(message)?;
    Ok(cap(content))
}

pub fn resources(base: &Path) -> Vec<String> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(base) {
        for entry in entries.flatten().take(100) {
            if entry.path().is_file() && entry.file_name() != "SKILL.md" {
                files.push(format!(
                    "  <file>{}</file>",
                    entry.file_name().to_string_lossy()
                ));
            } else if entry.path().is_dir() {
                files.push(format!(
                    "  <directory>{}/</directory>",
                    entry.file_name().to_string_lossy()
                ));
            }
        }
    }
    files.sort();
    files
}

pub fn cap(mut value: String) -> String {
    if value.len() <= OUTPUT_CAP {
        return value;
    }
    let mut end = OUTPUT_CAP - 16;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push_str("\n[truncated]");
    value
}

pub fn message(error: impl std::fmt::Display) -> SkillError {
    SkillError::Message(error.to_string())
}
