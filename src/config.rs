//! Runtime configuration read from environment variables.
//!
//! All settings have safe defaults and are read at call time (so tests can set
//! them per-case). None of these are MCP-framework settings, so they use
//! unprefixed names (`MAX_INPUT_BYTES`, `OCR_MAX_PAGES`, `OCR_CONCURRENCY`,
//! `ALLOWROOT`, `MAX_RETURN_CHARS`).

use std::path::PathBuf;

/// Global lock serializing tests that mutate process-wide env vars.
///
/// `config`, `tools` and `file_reader` tests all read/write the same env vars
/// (`MAX_INPUT_BYTES`, `ALLOWROOT`, ...). Each used to define its own local
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

/// Read `ALLOWROOT`, defaulting to `/` (the whole filesystem) on Unix.
///
/// Every path passed to a tool is canonicalized and must be within this root.
/// On Windows the analogous notion is the system volume root (`%system disk%:\`),
/// which is the natural `ALLOWROOT` default there.
pub fn allowroot() -> PathBuf {
    match std::env::var("ALLOWROOT") {
        Ok(v) if !v.trim().is_empty() => PathBuf::from(v.trim()),
        _ => {
            #[cfg(windows)]
            {
                // System disk root, e.g. `C:\`.
                match std::env::var("SystemDrive") {
                    Ok(d) if !d.is_empty() => PathBuf::from(format!("{d}\\")),
                    _ => PathBuf::from("C:\\"),
                }
            }
            #[cfg(not(windows))]
            {
                PathBuf::from("/")
            }
        }
    }
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
        remove_var("ALLOWROOT");
        assert_eq!(max_input_bytes(), DEFAULT_MAX_INPUT_BYTES);
        assert_eq!(ocr_max_pages(), DEFAULT_OCR_MAX_PAGES);
        assert_eq!(max_return_chars(), DEFAULT_MAX_RETURN_CHARS);
        assert_eq!(
            ocr_concurrency(),
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        );
        #[cfg(not(windows))]
        assert_eq!(allowroot(), PathBuf::from("/"));
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
}
