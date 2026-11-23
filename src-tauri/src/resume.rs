//! The tools that were on when the app restarted to install an update.
//! The restart stops them, and the next launch starts them again.

use std::{fs, path::Path};

use crate::ToolId;

const FILE: &str = "resume-after-update.json";

pub fn remember(directory: &Path, tools: &[ToolId]) {
    if let Ok(contents) = serde_json::to_vec(tools) {
        let _ = fs::write(directory.join(FILE), contents);
    }
}

/// Returns the remembered tools once. `None` means the last exit was not an update.
pub fn take(directory: &Path) -> Option<Vec<ToolId>> {
    let path = directory.join(FILE);
    let contents = fs::read(&path).ok()?;
    let _ = fs::remove_file(&path);
    serde_json::from_slice(&contents).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(name: &str) -> std::path::PathBuf {
        let directory =
            std::env::temp_dir().join(format!("lazy-worker-resume-{name}-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn returns_the_remembered_tools_once() {
        let directory = directory("once");
        remember(&directory, &[ToolId::Caffeinate, ToolId::Proxy]);
        assert_eq!(
            take(&directory),
            Some(vec![ToolId::Caffeinate, ToolId::Proxy])
        );
        assert_eq!(take(&directory), None);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn remembers_an_update_with_no_tool_on() {
        let directory = directory("none");
        remember(&directory, &[]);
        assert_eq!(take(&directory), Some(Vec::new()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn drops_a_file_it_cannot_read() {
        let directory = directory("broken");
        fs::write(directory.join(FILE), "not a list").unwrap();
        assert_eq!(take(&directory), None);
        assert!(!directory.join(FILE).exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
