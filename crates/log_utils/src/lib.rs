/// Initialize the global logger using env_logger.
///
/// Call this once at the start of the program to enable log output.
/// The log level can be controlled via the `RUST_LOG` environment variable.
pub fn init() {
    let _ = env_logger::builder().try_init();
}

/// Initialize the global logger with a specific default filter level.
pub fn init_with_level(level: log::LevelFilter) {
    let _ = env_logger::builder()
        .filter_level(level)
        .try_init();
}
