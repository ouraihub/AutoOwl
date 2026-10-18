//! Where AutoOwl keeps its data and logs: next to the executable by default,
//! overridable through `autoowl.paths.json` beside it.
//!
//! The override lives in a file, not the database, because the database sits
//! inside the data directory and logging starts before it is opened.
//! A directory that cannot be written is an error, never a silent fallback to
//! somewhere else, so data always lives in exactly one place.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File beside the executable holding user-chosen locations.
pub const PATHS_FILE: &str = "autoowl.paths.json";

/// User-chosen locations; an absent or blank entry means "use the default".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PathOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logs_dir: Option<String>,
}

/// Resolved, writable locations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locations {
    pub data: PathBuf,
    pub logs: PathBuf,
}

/// Directory holding the executable; the base for every default.
pub fn exe_dir() -> Result<PathBuf, String> {
    std::env::current_exe()
        .map_err(|error| format!("无法确定程序所在目录：{error}"))?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "无法确定程序所在目录".to_owned())
}

/// Reads the overrides file; a missing file is "no overrides", a broken one an error.
pub fn read_overrides(exe_dir: &Path) -> Result<PathOverrides, String> {
    let path = exe_dir.join(PATHS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| format!("{} 格式有误：{error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(PathOverrides::default()),
        Err(error) => Err(format!("无法读取 {}：{error}", path.display())),
    }
}

/// Writes the overrides file, dropping blank entries so defaults stay implicit.
pub fn write_overrides(exe_dir: &Path, overrides: &PathOverrides) -> Result<(), String> {
    let cleaned = PathOverrides {
        data_dir: non_blank(overrides.data_dir.as_deref()),
        logs_dir: non_blank(overrides.logs_dir.as_deref()),
    };
    let path = exe_dir.join(PATHS_FILE);
    let text = serde_json::to_string_pretty(&cleaned).map_err(|error| error.to_string())?;
    std::fs::write(&path, text).map_err(|error| format!("无法写入 {}：{error}", path.display()))
}

/// Applies overrides to the defaults: data `<exe>\data`, logs `<data>\logs`.
/// Pure, so the precedence is testable without touching the disk.
pub fn locations(exe_dir: &Path, overrides: &PathOverrides) -> Locations {
    let data = non_blank(overrides.data_dir.as_deref())
        .map_or_else(|| exe_dir.join("data"), PathBuf::from);
    let logs =
        non_blank(overrides.logs_dir.as_deref()).map_or_else(|| data.join("logs"), PathBuf::from);
    Locations { data, logs }
}

/// Resolves and creates both directories, failing if either cannot be written.
pub fn resolve() -> Result<Locations, String> {
    let exe_dir = exe_dir()?;
    let found = locations(&exe_dir, &read_overrides(&exe_dir)?);
    ensure_writable(&found.data, "数据目录")?;
    ensure_writable(&found.logs, "日志目录")?;
    Ok(found)
}

/// Creates `dir` and proves it accepts writes; ACLs alone do not say whether
/// this process can write (deny entries, UAC token filtering).
pub fn ensure_writable(dir: &Path, label: &str) -> Result<(), String> {
    let unwritable = |error: std::io::Error| {
        format!(
            "{label}不可写：{}\n{error}\n\n请把 AutoOwl 放到可写的位置（例如 D:\\AutoOwl），\
             或在 {PATHS_FILE} 中改用其他目录。",
            dir.display()
        )
    };
    std::fs::create_dir_all(dir).map_err(unwritable)?;
    let probe = dir.join(".autoowl-write-probe");
    std::fs::write(&probe, b"").map_err(unwritable)?;
    let _ = std::fs::remove_file(&probe);
    Ok(())
}

fn non_blank(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_executable_and_overrides_win() {
        let exe = Path::new(r"D:\Apps\AutoOwl");
        let none = locations(exe, &PathOverrides::default());
        assert_eq!(none.data, exe.join("data"));
        assert_eq!(none.logs, exe.join("data").join("logs"));

        // Moving only the data dir moves the default logs with it.
        let data_only = PathOverrides {
            data_dir: Some(r"E:\OwlData".into()),
            logs_dir: None,
        };
        let found = locations(exe, &data_only);
        assert_eq!(found.data, PathBuf::from(r"E:\OwlData"));
        assert_eq!(found.logs, PathBuf::from(r"E:\OwlData").join("logs"));

        // Blank entries mean "default", not "the current directory".
        let blank = PathOverrides {
            data_dir: Some("  ".into()),
            logs_dir: Some(String::new()),
        };
        assert_eq!(locations(exe, &blank), none);
    }

    #[test]
    fn overrides_round_trip_and_missing_file_means_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            read_overrides(dir.path()).unwrap(),
            PathOverrides::default()
        );

        let wanted = PathOverrides {
            data_dir: Some(r"E:\OwlData".into()),
            logs_dir: Some(" ".into()),
        };
        write_overrides(dir.path(), &wanted).unwrap();
        // The blank logs entry is dropped on write.
        assert_eq!(
            read_overrides(dir.path()).unwrap(),
            PathOverrides {
                data_dir: Some(r"E:\OwlData".into()),
                logs_dir: None
            }
        );

        std::fs::write(dir.path().join(PATHS_FILE), "{ not json").unwrap();
        assert!(read_overrides(dir.path()).is_err());
    }

    #[test]
    fn a_directory_that_cannot_be_created_is_an_error_not_a_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("occupied");
        std::fs::write(&file, b"x").unwrap();
        // A path under a regular file can never become a directory.
        let error = ensure_writable(&file.join("data"), "数据目录").unwrap_err();
        assert!(error.contains("数据目录不可写"), "{error}");
    }
}
