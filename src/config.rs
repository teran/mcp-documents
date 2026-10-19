//! Runtime configuration read from environment variables.
//!
//! All settings have safe defaults and are read at call time (so tests can set
//! them per-case). None of these are MCP-framework settings, so they use
//! unprefixed names (`MAX_INPUT_BYTES`, `OCR_MAX_PAGES`, `OCR_CONCURRENCY`,
//! `ALLOW_ROOT_DIR`, `MAX_RETURN_CHARS`).

use std::path::PathBuf;

/// Global lock serializing tests that mutate process-wide env vars.
///
/// `config`, `tools` and `file_reader` tests all read/write the same env vars
/// (`MAX_INPUT_BYTES`, `ALLOW_ROOT_DIR`, ...). Each used to define its own local
/// `Mutex`, so the full parallel test suite raced on those env vars (e.g. a
/// `config` test resetting `MAX_INPUT_BYTES` while a `tools` test was mid-way
/// relying on a value it had just set, causing flaky failures). All such tests
/// must take this single shared lock instead.
#[cfg(test)]
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Default maximum input size in bytes (250 MiB).
pub const DEFAULT_MAX_INPUT_BYTES: usize = 250 * 1024 * 1024;

/// Default maximum number of OCR pages processed per single request.
pub const DEFAULT_OCR_MAX_PAGES: usize = 300;

/// Default maximum characters returned for a `word_range` chunk (safety net).
pub const DEFAULT_MAX_RETURN_CHARS: usize = 50_000_000;

/// Default maximum decompressed bytes for a single archive member (256 MiB).
pub const DEFAULT_MAX_DECOMPRESSED_BYTES: usize = 256 * 1024 * 1024;

/// Read `MAX_INPUT_BYTES` (bytes), defaulting to 250 MiB. Enforced **before**
/// base64 decode and before/while reading a path file.
pub fn max_input_bytes() -> usize {
    env_usize("MAX_INPUT_BYTES", DEFAULT_MAX_INPUT_BYTES)
}

/// Read `OCR_MAX_PAGES`, defaulting to 300 — cap on OCR pages per request.
pub fn ocr_max_pages() -> usize {
    env_usize("OCR_MAX_PAGES", DEFAULT_OCR_MAX_PAGES)
}

/// Read `OCR_CONCURRENCY`, defaulting to the number of CPU cores. Bounds the
/// number of pages OCR'd in parallel per request (via a semaphore).
pub fn ocr_concurrency() -> usize {
    let from_env = env_usize("OCR_CONCURRENCY", 0);
    if from_env > 0 {
        return from_env;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .max(1)
}

/// Read `MAX_RETURN_CHARS`, defaulting to 50M — safety-net cap on the size of a
/// `word_range` chunk returned to the caller (guards against a single enormous
/// whitespace-delimited "word").
pub fn max_return_chars() -> usize {
    env_usize("MAX_RETURN_CHARS", DEFAULT_MAX_RETURN_CHARS)
}

/// Read `MAX_DECOMPRESSED_BYTES`, defaulting to 256 MiB — cap on the
/// decompressed size of a single archive member (e.g. a PPTX slide XML part)
/// read by adapters that parse zip containers directly. Guards against
/// decompression bombs where a tiny compressed member expands to an enormous
/// amount of text.
pub fn max_decompressed_bytes() -> usize {
    env_usize("MAX_DECOMPRESSED_BYTES", DEFAULT_MAX_DECOMPRESSED_BYTES)
}

/// The user's home directory, from `$HOME` (Unix) or `USERPROFILE` (Windows).
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

/// Expand a leading `~` (or `~/`) to the user's home directory.
///
/// `~` alone → home; `~/Documents/foo` → `<home>/Documents/foo`; any other
/// value (including absolute and relative paths) is returned unchanged. Falls
/// back to the literal input when the home directory is unavailable.
pub fn expand_tilde(input: &str) -> PathBuf {
    let trimmed = input.trim();
    if trimmed == "~" {
        return home_dir().unwrap_or_else(|| PathBuf::from("~"));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let mut base = home_dir().unwrap_or_else(|| PathBuf::from("~"));
        base.push(rest);
        return base;
    }
    PathBuf::from(trimmed)
}

/// The default root(s) when `ALLOW_ROOT_DIR` is unset.
fn default_roots() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        // System disk root, e.g. `C:\`.
        let d = match std::env::var("SystemDrive") {
            Ok(d) if !d.is_empty() => format!("{d}\\"),
            _ => "C:\\".into(),
        };
        vec![PathBuf::from(d)]
    }
    #[cfg(not(windows))]
    {
        match home_dir() {
            Some(home) => vec![home.join("Documents")],
            None => vec![PathBuf::from("/")],
        }
    }
}

/// Read `ALLOW_ROOT_DIR` and return **one or more** root directories.
///
/// The variable may hold a single path or a comma/semicolon-separated list of
/// paths (each `~`-expanded). When unset/empty it defaults to `~/Documents` on
/// Unix (falling back to `/` when the home directory is unavailable, e.g. in
/// minimal containers) and to the system volume root on Windows. Every `path`
/// passed to a tool is canonicalized and must be within **any** of these roots.
pub fn allowroots() -> Vec<PathBuf> {
    match std::env::var("ALLOW_ROOT_DIR") {
        Ok(v) if !v.trim().is_empty() => v
            .split([';', ','])
            .map(|s| expand_tilde(s.trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .collect::<Vec<_>>(),
        _ => default_roots(),
    }
}

/// The primary `ALLOW_ROOT_DIR` (first entry), for callers that want a single
/// root. See [`allowroots`] for the full (possibly multiple) set.
pub fn allowroot() -> PathBuf {
    allowroots()
        .into_iter()
        .next()
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Parse a `usize` from the given env var, falling back to `default` on
/// missing/unparseable/zero-unless-explicit values.
fn env_usize(key: &str, default: usize) -> usize {
    match std::env::var(key) {
        Ok(v) => v.trim().parse::<usize>().unwrap_or(default),
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn set_var(key: &str, val: &str) {
        unsafe { std::env::set_var(key, val) };
    }
    fn remove_var(key: &str) {
        unsafe { std::env::remove_var(key) };
    }

    #[test]
    fn defaults_when_unset() {
        let _g = lock();
        remove_var("MAX_INPUT_BYTES");
        remove_var("OCR_MAX_PAGES");
        remove_var("OCR_CONCURRENCY");
        remove_var("MAX_RETURN_CHARS");
        remove_var("ALLOW_ROOT_DIR");
        assert_eq!(max_input_bytes(), DEFAULT_MAX_INPUT_BYTES);
        assert_eq!(ocr_max_pages(), DEFAULT_OCR_MAX_PAGES);
        assert_eq!(max_return_chars(), DEFAULT_MAX_RETURN_CHARS);
        assert_eq!(max_decompressed_bytes(), DEFAULT_MAX_DECOMPRESSED_BYTES);
        assert_eq!(
            ocr_concurrency(),
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        );
        #[cfg(not(windows))]
        {
            let expected = home_dir()
                .map(|h| h.join("Documents"))
                .unwrap_or_else(|| PathBuf::from("/"));
            assert_eq!(allowroot(), expected);
        }
    }

    #[test]
    fn parses_decompressed_bytes() {
        let _g = lock();
        set_var("MAX_DECOMPRESSED_BYTES", "1024");
        assert_eq!(max_decompressed_bytes(), 1024);
        remove_var("MAX_DECOMPRESSED_BYTES");
    }

    #[test]
    fn expand_tilde_handles_home_and_other() {
        let _g = lock();
        // Bare `~` expands to the home dir.
        if let Some(home) = home_dir() {
            assert_eq!(expand_tilde("~"), home);
            assert_eq!(expand_tilde("~/Documents"), home.join("Documents"));
        }
        // Non-tilde inputs are returned unchanged.
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        assert_eq!(expand_tilde("rel/path"), PathBuf::from("rel/path"));
        // `~user` is intentionally NOT expanded (only `~` and `~/`).
        assert_eq!(expand_tilde("~other/x"), PathBuf::from("~other/x"));
    }

    #[test]
    fn allowroots_splits_and_expands_multiple() {
        let _g = lock();
        // Unset → a single default root.
        remove_var("ALLOW_ROOT_DIR");
        assert_eq!(allowroots().len(), 1);

        // Comma/semicolon-separated list, each entry `~`-expanded.
        set_var("ALLOW_ROOT_DIR", "/a/b, ~/Downloads;/c/d");
        let roots = allowroots();
        assert_eq!(roots.len(), 3);
        assert_eq!(roots[0], PathBuf::from("/a/b"));
        if let Some(home) = home_dir() {
            assert_eq!(roots[1], home.join("Downloads"));
        }
        assert_eq!(roots[2], PathBuf::from("/c/d"));
        remove_var("ALLOW_ROOT_DIR");
    }

    #[test]
    fn parses_env_values() {
        let _g = lock();
        set_var("MAX_INPUT_BYTES", "1024");
        set_var("OCR_MAX_PAGES", "7");
        set_var("OCR_CONCURRENCY", "3");
        set_var("MAX_RETURN_CHARS", "100");
        assert_eq!(max_input_bytes(), 1024);
        assert_eq!(ocr_max_pages(), 7);
        assert_eq!(ocr_concurrency(), 3);
        assert_eq!(max_return_chars(), 100);

        remove_var("MAX_INPUT_BYTES");
        remove_var("OCR_MAX_PAGES");
        remove_var("OCR_CONCURRENCY");
        remove_var("MAX_RETURN_CHARS");
    }

    #[test]
    fn invalid_env_falls_back_to_default() {
        let _g = lock();
        set_var("OCR_MAX_PAGES", "not-a-number");
        assert_eq!(ocr_max_pages(), DEFAULT_OCR_MAX_PAGES);
        remove_var("OCR_MAX_PAGES");
    }

    #[cfg(not(windows))]
    #[test]
    fn default_root_falls_back_to_root_without_home() {
        let _g = lock();
        let home = std::env::var_os("HOME");
        remove_var("ALLOW_ROOT_DIR");
        unsafe { std::env::remove_var("HOME") };
        assert_eq!(allowroot(), PathBuf::from("/"));
        if let Some(h) = home {
            unsafe { std::env::set_var("HOME", h) };
        }
    }
}
