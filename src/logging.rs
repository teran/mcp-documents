//! Process-level logging for this **stdio-only** MCP server.
//!
//! Because the transport is stdio, logs must **never** go to stdout (which is
//! reserved for the MCP JSON-RPC protocol) — they go to a **file** (L01). Logging
//! is **enabled only when `LOG_LEVEL` is set** (L02); unset ⇒ disabled.
//!
//! [`rmcp`] uses `tracing` internally, so installing a global `tracing`
//! subscriber here also captures the SDK's own events (L07) — no separate SDK
//! logger hook is required.
//!
//! All helpers are safe to call whether or not logging is enabled: when no
//! subscriber is installed, `tracing::info!` is a no-op. Only [`init`] decides
//! whether a real subscriber is installed.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing_subscriber::EnvFilter;

use crate::config::{LogFormat, log_filename, log_format, log_level};

/// Monotonic per-process counter used to generate `request_id` values (L09).
/// A simple counter is sufficient for a single-process stdio server; no uuid
/// dependency is needed.
static REQUEST_ID: AtomicU64 = AtomicU64::new(0);

/// Initialize logging from the environment.
///
/// Returns `Ok(None)` when logging is **disabled** (`LOG_LEVEL` unset/empty) —
/// the caller proceeds as before. Otherwise opens the `LOG_FILENAME` file
/// (append, chmod 600), installs a global `tracing` subscriber, and emits the
/// B05 startup banner as the **first** log line (L06).
pub fn init() -> Result<Option<()>, io::Error> {
    let Some(level) = log_level() else {
        // Logging disabled (L02): install no subscriber so all later
        // `tracing::info!` calls are no-ops.
        return Ok(None);
    };
    let file = open_log_file(&log_filename())?;
    install_subscriber(file, level.as_str(), log_format())?;
    banner();
    Ok(Some(()))
}

/// Open (creating if needed) the log file in append mode and force chmod 600 on
/// Unix so the log never becomes world-readable (L01).
fn open_log_file(filename: &str) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(filename)?;
    make_private(Path::new(filename))?;
    Ok(file)
}

/// Set the log file to be private (0600) on Unix; a no-op elsewhere.
#[cfg(unix)]
fn make_private(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn make_private(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Build a `tracing` subscriber writing to `file` with the configured level and
/// format, and install it as the global default (capturing rmcp's SDK events).
fn install_subscriber(file: File, level: &str, format: LogFormat) -> Result<(), io::Error> {
    let builder = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(level))
        .with_writer(file)
        .with_ansi(false);
    let result = match format {
        LogFormat::Json => tracing::subscriber::set_global_default(builder.json().finish()),
        LogFormat::Text => tracing::subscriber::set_global_default(builder.finish()),
    };
    result
        .map_err(|e| io::Error::other(format!("failed to install global tracing subscriber: {e}")))
}

/// The B05 startup banner — the first log line emitted when logging is enabled.
pub fn banner() {
    tracing::info!(
        target: "mcp_documents",
        "Starting mcp-documents/{} ...",
        env!("CARGO_PKG_VERSION")
    );
}

/// Generate a fresh `request_id` for a tool call (L09).
pub fn next_request_id() -> u64 {
    REQUEST_ID.fetch_add(1, Ordering::Relaxed)
}

/// Log the start of a tool call (L08). `args` must already be redacted — the
/// base64 `bytes` content is never logged (L05).
pub fn log_tool_call_start(tool: &str, args: &str, source: &str, request_id: u64) {
    tracing::info!(
        target: "mcp_documents::access",
        tool = %tool,
        args = %args,
        source = %source,
        request_id,
        phase = "start",
        "tool call",
    );
}

/// Log the completion of a tool call with its outcome and duration (L08/L09).
pub fn log_tool_call_end(
    tool: &str,
    args: &str,
    source: &str,
    request_id: u64,
    duration_ms: u64,
    outcome: &str,
) {
    tracing::info!(
        target: "mcp_documents::access",
        tool = %tool,
        args = %args,
        source = %source,
        request_id,
        duration_ms,
        outcome = %outcome,
        phase = "end",
        "tool call",
    );
}

/// Log the registered tool surface at startup (L10). Every tool here is static,
/// so each carries `always_present: true` (L11). All tools are always present —
/// there are no dynamic tools.
pub fn log_tool_surface(tools: &[(String, String)]) {
    for (name, title) in tools {
        tracing::info!(
            target: "mcp_documents::surface",
            tool = %name,
            title = %title,
            always_present = true,
            "registered tool",
        );
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// A `Write + Clone` writer capturing bytes, usable as a `tracing` writer.
    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn with_capture(f: impl FnOnce(Arc<Mutex<Vec<u8>>>)) -> String {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let value = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || Capture(value.clone()))
            .with_ansi(false)
            .finish();
        tracing::subscriber::with_default(subscriber, || f(buf.clone()));
        String::from_utf8(buf.lock().unwrap().clone()).unwrap()
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        crate::config::ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }
    fn set_var(key: &str, val: &str) {
        unsafe { std::env::set_var(key, val) };
    }
    fn remove_var(key: &str) {
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn open_log_file_creates_append_file() {
        let dir = std::env::temp_dir().join(format!("mcp-log-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.log");
        let path_str = path.to_str().unwrap();

        let f = open_log_file(path_str).unwrap();
        // Appends rather than truncating: a second open keeps existing bytes.
        let mut f2 = OpenOptions::new().append(true).open(path_str).unwrap();
        writeln!(f2, "line").unwrap();
        drop(f2);
        drop(f);

        let contents = std::fs::read_to_string(path_str).unwrap();
        assert_eq!(contents, "line\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(path_str).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "log file must be chmod 600 (L01)");
        }

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_disabled_when_log_level_unset() {
        let _g = lock();
        remove_var("LOG_LEVEL");
        assert!(init().unwrap().is_none());
    }

    #[test]
    fn init_enabled_writes_banner_to_log_file() {
        // This is the only test that enables logging (installs the global
        // subscriber); every other test keeps LOG_LEVEL unset.
        let _g = lock();
        let dir = std::env::temp_dir().join(format!("mcp-banner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("banner.log");
        set_var("LOG_LEVEL", "info");
        set_var("LOG_FILENAME", path.to_str().unwrap());
        set_var("LOG_FORMAT", "text");

        let enabled = init().unwrap();
        assert!(enabled.is_some());

        let contents = std::fs::read_to_string(&path).unwrap();
        assert!(
            contents.contains(&format!(
                "Starting mcp-documents/{} ...",
                env!("CARGO_PKG_VERSION")
            )),
            "banner must be the first log line (B05/L06), got: {contents}"
        );

        remove_var("LOG_LEVEL");
        remove_var("LOG_FILENAME");
        remove_var("LOG_FORMAT");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn init_returns_error_when_log_file_unopenable() {
        // The file-open failure must surface as `Err` so the caller (main.rs
        // `let _ = init()`) can continue running the server without logging —
        // a logging failure must never abort the process. This path must NOT
        // install any global subscriber (the open fails first), so it is safe
        // to run in parallel with `init_enabled_writes_banner_to_log_file`.
        let _g = lock();
        remove_var("LOG_LEVEL");
        set_var("LOG_LEVEL", "info");
        let dir = std::env::temp_dir().join(format!("mcp-noexist-{}", std::process::id()));
        // Parent directory does not exist -> open must fail.
        let missing = dir.join("nested").join("x.log");
        set_var("LOG_FILENAME", missing.to_str().unwrap());
        let r = init();
        assert!(
            r.is_err(),
            "init must fail when the log file cannot be opened"
        );
        remove_var("LOG_LEVEL");
        remove_var("LOG_FILENAME");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn banner_and_access_log_helpers_emit_structured_lines() {
        // Take the global env lock so this `with_default`-scoped test does not
        // race the process-global subscriber installed by
        // `init_enabled_writes_banner_to_log_file`.
        let _g = lock();
        let out = with_capture(|_| {
            banner();
            log_tool_call_start(
                "pdf_info",
                r#"{"path":"x","bytes_present":true}"#,
                "STDIO",
                7,
            );
            log_tool_call_end(
                "pdf_info",
                r#"{"path":"x","bytes_present":true}"#,
                "STDIO",
                7,
                5,
                "ok",
            );
        });
        assert!(out.contains(&format!(
            "Starting mcp-documents/{} ...",
            env!("CARGO_PKG_VERSION")
        )));
        assert!(out.contains("tool=pdf_info"), "got: {out}");
        assert!(out.contains("source=STDIO"), "got: {out}");
        assert!(out.contains("request_id=7"), "got: {out}");
        assert!(out.contains("duration_ms=5"), "got: {out}");
        assert!(out.contains("outcome=ok"), "got: {out}");
        assert!(out.contains("phase=\"start\""), "got: {out}");
        assert!(out.contains("phase=\"end\""), "got: {out}");
    }

    #[test]
    fn log_tool_surface_logs_each_tool_with_always_present() {
        let _g = lock();
        let out = with_capture(|_| {
            log_tool_surface(&[
                ("pdf_info".to_string(), "PDF metadata".to_string()),
                (
                    "csv_extract_markdown".to_string(),
                    "Extract CSV".to_string(),
                ),
            ]);
        });
        assert!(out.contains("tool=pdf_info"), "got: {out}");
        assert!(out.contains("title=PDF metadata"), "got: {out}");
        assert!(out.contains("tool=csv_extract_markdown"), "got: {out}");
        assert!(out.contains("always_present=true"), "got: {out}");
    }

    #[test]
    fn request_ids_are_monotonic() {
        let a = next_request_id();
        let b = next_request_id();
        assert!(b > a);
    }

    #[test]
    fn json_format_subscriber_emits_json_lines() {
        // Exercise the `LOG_FORMAT=json` builder path (without touching the
        // process-global subscriber).
        let _g = lock();
        let buf = Arc::new(Mutex::new(Vec::new()));
        let value = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || Capture(value.clone()))
            .with_ansi(false)
            .json()
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            log_tool_call_end("doc_info", r#"{}"#, "STDIO", 1, 2, "ok");
        });
        let out = String::from_utf8(buf.lock().unwrap().clone()).unwrap();
        assert!(out.contains("\"tool\":\"doc_info\""), "got: {out}");
        assert!(out.contains("\"outcome\":\"ok\""), "got: {out}");
        assert!(out.contains("\"request_id\":1"), "got: {out}");
    }
}
