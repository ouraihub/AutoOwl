//! Local library use cases.
//!
//! Owns the rules the interface layer must not duplicate: which directories are
//! scanned, what a valid page is, and whether a file may be deleted.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::AppPaths;
use crate::domain::local_music::{LocalTrack, Page};
use crate::domain::settings::AppSettings;
use crate::infrastructure::local_repository::{
    LocalMusicError, LocalQuery, LocalRepository, ScanSummary,
};

/// Largest page the interface layer will serve.
pub const MAX_PAGE_SIZE: u32 = 100;

/// Directories scanned for local music.
///
/// The download directory is the library: the app indexes exactly what it
/// downloaded, and a user pointing `download_dir` at an existing collection is
/// the supported way to import one.
pub fn music_roots(paths: &AppPaths, settings: &AppSettings) -> Vec<PathBuf> {
    let mut roots = vec![paths.resolve_download_dir(&settings.download_dir)];
    // The default download directory stays a root even after the user points
    // the setting elsewhere, so previously downloaded files remain listed.
    if !roots.contains(&paths.downloads) {
        roots.push(paths.downloads.clone());
    }
    roots.retain(|root| root.is_dir());
    roots
}

/// True when `candidate` resolves inside one of `roots`.
///
/// Both sides are canonicalized so `..` segments, symlinks and mixed
/// separators cannot smuggle a path out of the library. A path that cannot be
/// canonicalized is refused rather than guessed at.
pub fn is_inside_roots(roots: &[PathBuf], candidate: &Path) -> bool {
    let Ok(target) = candidate.canonicalize() else {
        return false;
    };
    roots.iter().any(|root| {
        root.canonicalize()
            .map(|root| target.starts_with(root))
            .unwrap_or(false)
    })
}

pub struct LocalMusicService {
    repository: LocalRepository,
}

impl LocalMusicService {
    pub fn new(database: Arc<crate::infrastructure::storage::Database>) -> Self {
        Self {
            repository: LocalRepository::new(database),
        }
    }

    /// Lists indexed tracks, scanning first when the index is empty so a fresh
    /// install shows its library without an explicit refresh.
    pub fn list(
        &self,
        roots: &[PathBuf],
        page: u32,
        page_size: u32,
        filter: &str,
    ) -> Result<Page<LocalTrack>, LocalMusicError> {
        validate_page(page, page_size)?;
        if self.repository.is_empty()? && !roots.is_empty() {
            self.repository.scan(roots)?;
        }
        self.repository
            .list(LocalQuery::new(page, page_size).with_filter(filter))
    }

    /// Rebuilds the index from disk.
    pub fn refresh(&self, roots: &[PathBuf]) -> Result<ScanSummary, LocalMusicError> {
        self.repository.scan(roots)
    }

    /// Deletes a track from the index and, when `remove_file`, from disk.
    ///
    /// The file is only unlinked when it lives inside a configured root: a
    /// stale or tampered index row must never turn into an arbitrary delete.
    pub fn delete(
        &self,
        roots: &[PathBuf],
        id: &str,
        remove_file: bool,
    ) -> Result<LocalTrack, LocalMusicError> {
        if id.trim().is_empty() {
            return Err(LocalMusicError::InvalidArgument(
                "本地音乐 ID 无效".to_owned(),
            ));
        }
        let track = self
            .repository
            .get(id)?
            .ok_or_else(|| LocalMusicError::NotFound(id.to_owned()))?;

        if remove_file {
            let path = PathBuf::from(&track.path);
            if !is_inside_roots(roots, &path) {
                return Err(LocalMusicError::InvalidArgument(
                    "只能删除音乐目录内的文件".to_owned(),
                ));
            }
            std::fs::remove_file(&path).map_err(|error| {
                tracing::warn!(event = "local_delete_failed", path = %path.display(), %error);
                LocalMusicError::Io(format!("删除文件失败: {error}"))
            })?;
        }
        self.repository.remove(id)
    }

    /// Single track, for queueing it into the player.
    pub fn get(&self, id: &str) -> Result<Option<LocalTrack>, LocalMusicError> {
        self.repository.get(id)
    }

    /// Every indexed track, scanning first when the index is empty.
    ///
    /// The duplicate view and the download precheck both compare against the
    /// whole library, not a page of it, so they cannot go through `list`. Paged
    /// internally because the repository caps one query at `MAX_PAGE_SIZE`.
    pub fn all_tracks(&self, roots: &[PathBuf]) -> Result<Vec<LocalTrack>, LocalMusicError> {
        if self.repository.is_empty()? && !roots.is_empty() {
            self.repository.scan(roots)?;
        }
        let first = self.repository.list(LocalQuery::new(1, MAX_PAGE_SIZE))?;
        let pages = total_pages(first.total, MAX_PAGE_SIZE);
        let mut tracks = first.items;
        for page in 2..=pages {
            let batch = self.repository.list(LocalQuery::new(page, MAX_PAGE_SIZE))?;
            if batch.items.is_empty() {
                break;
            }
            tracks.extend(batch.items);
        }
        Ok(tracks)
    }
}

/// Page count for `total` rows, never below 1.
///
/// An empty list reports `total_pages: 1` rather than 0, so the
/// UI always has a valid page to sit on.
pub fn total_pages(total: u64, page_size: u32) -> u32 {
    let page_size = u64::from(page_size.max(1));
    let pages = total.div_ceil(page_size).max(1);
    u32::try_from(pages).unwrap_or(u32::MAX)
}

/// Requested page clamped into `1..=total_pages`, with that page count.
///
/// An overshooting page is clamped down instead of serving an empty
/// list, so a UI still holding page 5 after the rows were cleared lands on the
/// last page with data rather than on nothing.
pub fn clamp_page(page: u32, total: u64, page_size: u32) -> (u32, u32) {
    let pages = total_pages(total, page_size);
    (page.max(1).min(pages), pages)
}

/// Rejects pages the UI cannot render.
pub fn validate_page(page: u32, page_size: u32) -> Result<(), LocalMusicError> {
    if page == 0 || page_size == 0 || page_size > MAX_PAGE_SIZE {
        return Err(LocalMusicError::InvalidArgument(format!(
            "分页参数无效，页码需大于 0 且每页 1-{MAX_PAGE_SIZE} 条"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_page, is_inside_roots, music_roots, validate_page, LocalMusicService, MAX_PAGE_SIZE,
    };
    use crate::config::AppPaths;
    use crate::domain::settings::AppSettings;
    use crate::infrastructure::storage::Database;
    use std::sync::Arc;

    fn service() -> LocalMusicService {
        let database = Arc::new(Database::open_in_memory().expect("open"));
        LocalMusicService::new(database)
    }

    #[test]
    fn page_bounds_are_enforced() {
        assert!(validate_page(1, 20).is_ok());
        assert!(validate_page(0, 20).is_err());
        assert!(validate_page(1, 0).is_err());
        assert!(validate_page(1, MAX_PAGE_SIZE + 1).is_err());
    }

    #[test]
    fn an_overshooting_page_clamps_to_the_last_page_with_rows() {
        // 45 rows at 20 per page is 3 pages; page 9 lands on 3, not on empty.
        assert_eq!(clamp_page(9, 45, 20), (3, 3));
        assert_eq!(clamp_page(2, 45, 20), (2, 3));
        // An empty table still has one page to sit on.
        assert_eq!(clamp_page(7, 0, 20), (1, 1));
        // A zero page from a sloppy caller is the first page, never page 0.
        assert_eq!(clamp_page(0, 45, 20), (1, 3));
        // An exact multiple must not report a trailing empty page.
        assert_eq!(clamp_page(4, 40, 20), (2, 2));
    }

    #[test]
    fn music_roots_include_the_configured_and_default_directories() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::from_root(temp.path().to_path_buf());
        paths.ensure().expect("ensure");
        let configured = temp.path().join("library");
        std::fs::create_dir_all(&configured).expect("create library");

        let settings = AppSettings {
            download_dir: configured.to_string_lossy().into_owned(),
            ..AppSettings::default()
        };
        let roots = music_roots(&paths, &settings);
        assert!(roots.contains(&configured));
        assert!(roots.contains(&paths.downloads));
    }

    #[test]
    fn music_roots_drop_directories_that_do_not_exist() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::from_root(temp.path().join("missing"));
        let roots = music_roots(&paths, &AppSettings::default());
        assert!(roots.is_empty());
    }

    #[test]
    fn paths_outside_every_root_are_refused() {
        let library = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let inside_file = library.path().join("a.mp3");
        std::fs::write(&inside_file, b"x").expect("write");
        let outside_file = outside.path().join("b.mp3");
        std::fs::write(&outside_file, b"x").expect("write");

        let roots = vec![library.path().to_path_buf()];
        assert!(is_inside_roots(&roots, &inside_file));
        assert!(!is_inside_roots(&roots, &outside_file));
        // A traversal that climbs out of the root is refused even though it is
        // spelled relative to the root.
        let traversal = library.path().join("..").join(
            outside_file
                .file_name()
                .map(std::path::Path::new)
                .unwrap_or(std::path::Path::new("b.mp3")),
        );
        assert!(!is_inside_roots(&roots, &traversal));
    }

    #[test]
    fn delete_refuses_a_file_outside_the_configured_root() {
        let library = tempfile::tempdir().expect("tempdir");
        let outside = tempfile::tempdir().expect("tempdir");
        let stray = outside.path().join("stray.mp3");
        std::fs::write(&stray, b"ID3\x03\0\0\0\0").expect("write");

        let service = service();
        // Index the stray file by scanning the directory it lives in, then
        // narrow the configured roots to the library only.
        service
            .refresh(&[outside.path().to_path_buf()])
            .expect("scan");
        let indexed = service
            .list(&[], 1, 10, "")
            .expect("list")
            .items
            .pop()
            .expect("one track");

        let result = service.delete(&[library.path().to_path_buf()], &indexed.id, true);
        assert!(result.is_err());
        // Neither the file nor the row may be gone.
        assert!(stray.exists());
        assert!(service.get(&indexed.id).expect("get").is_some());
    }

    #[test]
    fn delete_removes_the_file_and_the_row_inside_a_root() {
        let library = tempfile::tempdir().expect("tempdir");
        let file = library.path().join("a.mp3");
        std::fs::write(&file, b"ID3\x03\0\0\0\0").expect("write");
        let roots = vec![library.path().to_path_buf()];

        let service = service();
        service.refresh(&roots).expect("scan");
        let indexed = service
            .list(&roots, 1, 10, "")
            .expect("list")
            .items
            .pop()
            .expect("one track");

        service.delete(&roots, &indexed.id, true).expect("delete");
        assert!(!file.exists());
        assert!(service.get(&indexed.id).expect("get").is_none());
    }

    #[test]
    fn delete_can_drop_the_row_while_keeping_the_file() {
        let library = tempfile::tempdir().expect("tempdir");
        let file = library.path().join("a.mp3");
        std::fs::write(&file, b"ID3\x03\0\0\0\0").expect("write");
        let roots = vec![library.path().to_path_buf()];

        let service = service();
        service.refresh(&roots).expect("scan");
        let indexed = service
            .list(&roots, 1, 10, "")
            .expect("list")
            .items
            .pop()
            .expect("one track");

        service.delete(&roots, &indexed.id, false).expect("delete");
        assert!(file.exists());
        assert!(service.get(&indexed.id).expect("get").is_none());
    }

    #[test]
    fn deleting_an_unknown_id_fails() {
        let service = service();
        assert!(service.delete(&[], "nope", false).is_err());
        assert!(service.delete(&[], "  ", false).is_err());
    }
}
