use std::path::Path;

use aether_observability::{LogConfig, LogHandle, LogLevel};

/// Rotating `autoowl.log` (5 MB x 3) in `logs_dir`; `RUST_LOG` overrides the
/// level, and the settings page can change it at runtime through the Aether plugin.
pub fn init(logs_dir: &Path) -> Result<LogHandle, String> {
    aether_observability::init(
        logs_dir,
        LogConfig {
            file_stem: "autoowl",
            default_level: LogLevel::INFO,
            env_var: Some("RUST_LOG"),
            ..LogConfig::default()
        },
        None,
    )
    .map_err(|error| error.to_string())
}
