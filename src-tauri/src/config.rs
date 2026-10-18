use std::path::{Path, PathBuf};

/// Filesystem layout for the application data directory.
///
/// A `data/` layout (settings.db + downloads/) rooted in
/// the platform application data directory instead of the process working
/// directory, per `docs/工程规范与质量门禁.md` section 8.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub database: PathBuf,
    pub logs: PathBuf,
    pub downloads: PathBuf,
    pub cache: PathBuf,
    pub video_output: PathBuf,
}

impl AppPaths {
    pub fn from_root(root: PathBuf) -> Self {
        Self {
            database: root.join("autoowl.db"),
            logs: root.join("logs"),
            downloads: root.join("downloads"),
            cache: root.join("cache"),
            video_output: root.join("video_output"),
            root,
        }
    }

    /// Creates every directory the application writes to.
    pub fn ensure(&self) -> std::io::Result<()> {
        for directory in [
            &self.root,
            &self.logs,
            &self.downloads,
            &self.cache,
            &self.video_output,
        ] {
            std::fs::create_dir_all(directory)?;
        }
        Ok(())
    }

    /// Resolves a user-configured download directory against the app root.
    ///
    /// Relative values stay inside the application data directory; absolute
    /// values are honoured so users can target an external music library.
    pub fn resolve_download_dir(&self, configured: &str) -> PathBuf {
        let trimmed = configured.trim();
        if trimmed.is_empty() {
            return self.downloads.clone();
        }
        let candidate = Path::new(trimmed);
        if candidate.is_absolute() {
            candidate.to_path_buf()
        } else {
            self.root.join(candidate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AppPaths;
    use std::path::{Path, PathBuf};

    #[test]
    fn relative_download_dir_stays_under_root() {
        let paths = AppPaths::from_root(PathBuf::from("root"));
        assert_eq!(
            paths.resolve_download_dir("music"),
            Path::new("root").join("music")
        );
    }

    #[test]
    fn empty_download_dir_falls_back_to_default() {
        let paths = AppPaths::from_root(PathBuf::from("root"));
        assert_eq!(paths.resolve_download_dir("  "), paths.downloads);
    }

    #[test]
    fn absolute_download_dir_is_honoured() {
        let paths = AppPaths::from_root(PathBuf::from("root"));
        let absolute = if cfg!(windows) {
            "D:\\Music"
        } else {
            "/srv/music"
        };
        assert_eq!(paths.resolve_download_dir(absolute), Path::new(absolute));
    }
}
