//! Pure helpers for resolving and validating inclusive-from/exclusive-to ranges.
//!
//! Kept in the domain layer so the application use cases can validate user
//! ranges (page/slide/word/row) without duplicating logic, and so the logic is
//! unit-testable without any adapter.

use std::ops::Range;

use crate::domain::error::ExtractionError;

/// Resolve an optional `(from, to)` pair (both inclusive-from/exclusive-to,
/// 0-based) against `total` items, clamping to `[0, total)`.
///
/// - `None` for `from` means the start of the document (`0`).
/// - `None` for `to` means the end of the document (`total`).
/// - `from > to` is rejected as a malformed request.
/// - Out-of-range bounds are clamped (never an error on their own) so a large
///   `to` simply means "to the end".
///
/// The returned range is guaranteed `start <= end` and `end <= total`.
pub fn resolve_range(
    from: Option<usize>,
    to: Option<usize>,
    total: usize,
    what: &str,
) -> Result<Range<usize>, ExtractionError> {
    let start = from.unwrap_or(0).min(total);
    let end = to.unwrap_or(total).min(total);
    if start > end {
        return Err(ExtractionError::Malformed(format!(
            "{what}_from ({start}) must not exceed {what}_to ({end})"
        )));
    }
    Ok(start..end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_whole_document() {
        assert_eq!(resolve_range(None, None, 10, "page").unwrap(), 0..10);
    }

    #[test]
    fn explicit_range_is_used() {
        assert_eq!(resolve_range(Some(2), Some(5), 10, "row").unwrap(), 2..5);
    }

    #[test]
    fn end_clamped_to_total() {
        assert_eq!(resolve_range(Some(0), Some(999), 10, "row").unwrap(), 0..10);
        assert_eq!(resolve_range(Some(3), None, 10, "row").unwrap(), 3..10);
    }

    #[test]
    fn start_clamped_to_total() {
        assert_eq!(resolve_range(Some(20), None, 10, "row").unwrap(), 10..10);
    }

    #[test]
    fn from_greater_than_to_is_malformed() {
        let err = resolve_range(Some(5), Some(2), 10, "row").unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
        assert!(err.to_string().contains("row_from"));
    }
}
