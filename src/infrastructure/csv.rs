//! `CsvStreamReader` — implements [`CsvReader`] using the `csv` crate with
//! streaming (memory < file size).
//!
//! - `csv_info`: a single streaming pass counts rows and captures the header
//!   (O(1) memory).
//! - `extract_csv`: skips to the start row and reads only the requested range
//!   (O(range) memory), rendering a Markdown table.

use std::io::{Cursor, Read};
use std::ops::Range;
use std::sync::Arc;

use crate::domain::error::ExtractionError;
use crate::domain::meta::CsvInfo;
use crate::domain::ports::CsvReader;
use crate::domain::result::Source;

use super::error_map::map_csv_io_error;
use super::table::render_table;

/// Streaming CSV reader.
#[derive(Default)]
pub struct CsvStreamReader {}

impl CsvStreamReader {
    /// Create a new reader.
    pub fn new() -> Self {
        Self {}
    }

    /// Build a `csv::Reader` over the source, streaming from disk when it is a
    /// path (so memory stays below file size) and from a cursor for in-memory
    /// bytes.
    ///
    /// The read path is wrapped in `catch_unwind` (mirroring the other
    /// adapters) so a panic while opening/constructing the reader becomes a
    /// domain error rather than aborting the MCP request.
    fn open(&self, source: &Source) -> Result<csv::Reader<Box<dyn Read>>, ExtractionError> {
        let result = std::panic::catch_unwind(|| {
            let reader: Box<dyn Read> = match source {
                Source::Path(p) => {
                    let file = std::fs::File::open(p).map_err(map_csv_io_error)?;
                    Box::new(file)
                }
                Source::Bytes(b) => Box::new(Cursor::new(b.clone())),
            };
            Ok::<csv::Reader<Box<dyn Read>>, ExtractionError>(csv::Reader::from_reader(reader))
        });
        match result {
            Ok(Ok(rdr)) => Ok(rdr),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExtractionError::Internal(
                "CSV reader panicked while opening input".into(),
            )),
        }
    }

    /// Parse CSV info (header + row count), isolating a panic from the `csv`
    /// crate into a domain error.
    fn csv_info_from_reader(
        &self,
        mut rdr: csv::Reader<Box<dyn Read>>,
    ) -> Result<CsvInfo, ExtractionError> {
        // `csv::Reader<Box<dyn Read>>` is not `UnwindSafe` (the `dyn Read`
        // trait object does not guarantee it), so the closure must be asserted
        // unwind-safe to use `catch_unwind`.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let headers = rdr
                .headers()
                .map_err(|e| ExtractionError::Malformed(format!("csv header: {e}")))?;
            let column_count = headers.len();
            let header: Vec<String> = headers.iter().map(str::to_string).collect();

            let mut row_count = 0usize;
            for record in rdr.records() {
                record.map_err(|e| ExtractionError::Malformed(format!("csv record: {e}")))?;
                row_count += 1;
            }
            Ok::<_, ExtractionError>(CsvInfo {
                row_count,
                column_count,
                header,
            })
        }));
        match result {
            Ok(Ok(info)) => Ok(info),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExtractionError::Internal(
                "CSV reader panicked while reading info".into(),
            )),
        }
    }

    /// Extract a row range (or the whole file when `row_range` is `None`) as a
    /// Markdown table, isolating a panic from the `csv` crate into a domain
    /// error.
    fn extract_from_reader(
        &self,
        mut rdr: csv::Reader<Box<dyn Read>>,
        row_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let headers = rdr
                .headers()
                .map_err(|e| ExtractionError::Malformed(format!("csv header: {e}")))?;
            let header: Vec<String> = headers.iter().map(str::to_string).collect();

            // Data rows are 0-indexed after the header. from/to are
            // inclusive-from / exclusive-to over those data rows.
            let from = row_range.as_ref().map(|r| r.start).unwrap_or(0);
            let to = row_range.as_ref().map(|r| r.end);
            if let Some(t) = to
                && from > t
            {
                return Err(ExtractionError::Malformed(format!(
                    "row_from ({from}) must not exceed row_to ({t})"
                )));
            }

            let mut rows: Vec<Vec<String>> = Vec::new();
            for (i, record) in rdr.records().enumerate() {
                if i < from {
                    continue;
                }
                if let Some(t) = to
                    && i >= t
                {
                    break;
                }
                let record =
                    record.map_err(|e| ExtractionError::Malformed(format!("csv record: {e}")))?;
                rows.push(record.iter().map(str::to_string).collect());
            }

            Ok::<_, ExtractionError>(render_table(Some(header), rows))
        }));
        match result {
            Ok(Ok(md)) => Ok(md),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExtractionError::Internal(
                "CSV reader panicked while extracting".into(),
            )),
        }
    }
}

impl CsvReader for CsvStreamReader {
    fn csv_info(&self, source: &Source) -> Result<CsvInfo, ExtractionError> {
        let rdr = self.open(source)?;
        self.csv_info_from_reader(rdr)
    }

    fn extract_csv(
        &self,
        source: &Source,
        row_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError> {
        let rdr = self.open(source)?;
        self.extract_from_reader(rdr, row_range)
    }
}

/// Build an `Arc<dyn CsvReader>` for wiring/composition.
pub fn make_csv_reader() -> Arc<dyn CsvReader> {
    Arc::new(CsvStreamReader::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A `Read` that always panics, used to prove a panic in the CSV read/parse
    /// path is converted into a domain error rather than aborting.
    struct PanickingRead;

    impl std::io::Read for PanickingRead {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            panic!("injected csv read panic")
        }
    }

    /// Build a reader over a panicking source so the `catch_unwind` branch in
    /// the parse helpers is genuinely exercised (the `csv` crate itself does not
    /// panic on crafted input, so a hostile `Read` is the reliable seam).
    fn panicking_reader() -> csv::Reader<Box<dyn Read>> {
        csv::Reader::from_reader(Box::new(PanickingRead) as Box<dyn Read>)
    }

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// Write a unique temp CSV (per call) so parallel tests never collide.
    fn tmp_csv(content: &str) -> std::path::PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("mcp-csv-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("data.csv");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        path
    }

    fn cleanup(path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn csv_info_counts_rows_and_header() {
        let path = tmp_csv("name,age\nAlice,30\nBob,25\n");
        let r = CsvStreamReader::new();
        let info = r.csv_info(&Source::Path(path.clone())).unwrap();
        assert_eq!(info.row_count, 2);
        assert_eq!(info.column_count, 2);
        assert_eq!(info.header, vec!["name", "age"]);
        cleanup(&path);
    }

    #[test]
    fn csv_extract_full_table() {
        let path = tmp_csv("a,b\n1,2\n3,4\n");
        let r = CsvStreamReader::new();
        let md = r.extract_csv(&Source::Path(path.clone()), None).unwrap();
        assert!(md.contains("| a | b |"));
        assert!(md.contains("| 1 | 2 |"));
        assert!(md.contains("| 3 | 4 |"));
        cleanup(&path);
    }

    #[test]
    fn csv_extract_all_without_range_includes_every_row() {
        // Explicitly exercise the `None` row-range branch (extract the whole
        // file): no `to` bound, so every data row is emitted.
        let path = tmp_csv("a,b\n1,2\n3,4\n5,6\n");
        let r = CsvStreamReader::new();
        let md = r.extract_csv(&Source::Path(path.clone()), None).unwrap();
        assert!(md.contains("| 1 | 2 |"));
        assert!(md.contains("| 3 | 4 |"));
        assert!(md.contains("| 5 | 6 |"));
        cleanup(&path);
    }

    #[test]
    fn csv_info_panic_is_isolated_to_internal_error() {
        let r = CsvStreamReader::new();
        let err = r.csv_info_from_reader(panicking_reader()).unwrap_err();
        assert!(matches!(err, ExtractionError::Internal(_)));
    }

    #[test]
    fn csv_extract_panic_is_isolated_to_internal_error() {
        let r = CsvStreamReader::new();
        let err = r.extract_from_reader(panicking_reader(), None).unwrap_err();
        assert!(matches!(err, ExtractionError::Internal(_)));
    }

    #[test]
    fn csv_extract_row_range() {
        let path = tmp_csv("a,b\n1,2\n3,4\n5,6\n");
        let r = CsvStreamReader::new();
        let md = r
            .extract_csv(&Source::Path(path.clone()), Some(1..3))
            .unwrap();
        assert!(md.contains("| 3 | 4 |"));
        assert!(md.contains("| 5 | 6 |"));
        assert!(!md.contains("| 1 | 2 |"));
        cleanup(&path);
    }

    #[test]
    fn csv_from_bytes() {
        let r = CsvStreamReader::new();
        let md = r
            .extract_csv(&Source::Bytes(b"a,b\nx,y".to_vec()), None)
            .unwrap();
        assert!(md.contains("| x | y |"));
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)]
    fn csv_from_greater_than_to_is_malformed() {
        let path = tmp_csv("a\n1\n");
        let r = CsvStreamReader::new();
        let err = r
            .extract_csv(&Source::Path(path.clone()), Some(3..1))
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
        cleanup(&path);
    }

    #[test]
    fn constructors() {
        let a = CsvStreamReader::new();
        let b = make_csv_reader();
        let _ = (a, b);
    }

    #[test]
    fn csv_open_missing_path_is_io_error() {
        // Opening a non-existent path must map the file error (line 51 branch).
        let r = CsvStreamReader::new();
        let missing = std::env::temp_dir().join(format!("mcp-csv-missing-{}", std::process::id()));
        let err = r.csv_info(&Source::Path(missing)).unwrap_err();
        assert!(matches!(err, ExtractionError::Io(_)));
    }

    #[test]
    fn csv_info_inconsistent_columns_is_malformed() {
        // A row with more fields than the header triggers a record parse error,
        // exercising the `Ok(Err(e)) => Err(e)` branch in csv_info_from_reader.
        let path = tmp_csv("a,b\n1,2,3\n");
        let r = CsvStreamReader::new();
        let err = r.csv_info(&Source::Path(path.clone())).unwrap_err();
        cleanup(&path);
        assert!(matches!(err, ExtractionError::Malformed(_)));
    }
}
