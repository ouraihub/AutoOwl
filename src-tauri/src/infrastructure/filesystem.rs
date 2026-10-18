use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum FileSystemError {
    #[error("path escapes the allowed root")]
    OutsideRoot,
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

pub trait FileSystem: Send + Sync {
    fn ensure_root(&self, root: &Path) -> Result<(), FileSystemError>;
    fn safe_path(&self, root: &Path, relative: &Path) -> Result<PathBuf, FileSystemError>;
}

pub struct LocalFileSystem;

impl FileSystem for LocalFileSystem {
    fn ensure_root(&self, root: &Path) -> Result<(), FileSystemError> {
        std::fs::create_dir_all(root).map_err(FileSystemError::from)
    }

    fn safe_path(&self, root: &Path, relative: &Path) -> Result<PathBuf, FileSystemError> {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| component == std::path::Component::ParentDir)
        {
            return Err(FileSystemError::OutsideRoot);
        }
        Ok(root.join(relative))
    }
}

#[cfg(test)]
mod tests {
    use super::{FileSystem, LocalFileSystem};
    use std::path::Path;

    #[test]
    fn rejects_parent_segments() {
        let result = LocalFileSystem.safe_path(Path::new("downloads"), Path::new("../secret"));
        assert!(result.is_err());
    }
}
