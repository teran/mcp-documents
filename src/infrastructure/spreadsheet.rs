//! `CalamineSpreadsheetReader` — implements [`SpreadsheetReader`] using
//! `calamine` (reads both `.xls` and `.xlsx`).

use std::io::Cursor;
use std::ops::Range;
use std::sync::Arc;

use calamine::{Data, Reader, open_workbook_auto_from_rs};

use crate::domain::error::ExtractionError;
use crate::domain::meta::{SheetInfo, SpreadsheetInfo};
use crate::domain::ports::SpreadsheetReader;
use crate::domain::ranges::resolve_range;
use crate::domain::result::Source;

use super::error_map::map_calamine_error;
use super::file_reader::read_source_bytes;
use super::table::render_table;

/// Adapter listing sheets and extracting sheet/row ranges from spreadsheets.
#[derive(Default)]
pub struct CalamineSpreadsheetReader {}

impl CalamineSpreadsheetReader {
    /// Create a new reader.
    pub fn new() -> Self {
        Self {}
    }
}

/// Format a `calamine::Data` cell as a display string (empty → "").
pub fn cell_to_string(cell: &Data) -> String {
    cell.to_string()
}

impl SpreadsheetReader for CalamineSpreadsheetReader {
    fn spreadsheet_info(&self, source: &Source) -> Result<SpreadsheetInfo, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let result = std::panic::catch_unwind(|| {
            let mut wb = open_workbook_auto_from_rs(Cursor::new(bytes))?;
            let names = wb.sheet_names();
            let mut sheets = Vec::with_capacity(names.len());
            for name in names {
                let (rows, cols) = match wb.worksheet_range(&name) {
                    Ok(r) => (Some(r.height()), Some(r.width())),
                    Err(_) => (None, None),
                };
                sheets.push(SheetInfo { name, rows, cols });
            }
            Ok::<_, calamine::Error>(sheets)
        });
        match result {
            Ok(Ok(sheets)) => Ok(SpreadsheetInfo { sheets }),
            Ok(Err(e)) => Err(map_calamine_error(e)),
            Err(_) => Err(ExtractionError::Internal(
                "spreadsheet reader panicked while listing sheets".into(),
            )),
        }
    }

    fn extract_sheet(
        &self,
        source: &Source,
        sheet: Option<&str>,
        row_range: Option<Range<usize>>,
    ) -> Result<String, ExtractionError> {
        let bytes = read_source_bytes(source)?;
        let result = std::panic::catch_unwind(|| {
            let mut wb =
                open_workbook_auto_from_rs(Cursor::new(bytes)).map_err(map_calamine_error)?;
            let names = wb.sheet_names();

            let target = match sheet {
                Some(s) => {
                    if !names.iter().any(|n| n == s) {
                        return Err(ExtractionError::Malformed(format!(
                            "sheet '{s}' not found (available: {})",
                            names.join(", ")
                        )));
                    }
                    s.to_string()
                }
                None => names
                    .first()
                    .ok_or_else(|| ExtractionError::Malformed("workbook has no sheets".into()))?
                    .clone(),
            };

            let range = wb.worksheet_range(&target).map_err(map_calamine_error)?;
            let total_rows = range.height();
            let total_cols = range.width();

            let from = row_range.as_ref().map(|r| r.start);
            let to = row_range.as_ref().map(|r| r.end);
            let resolved = resolve_range(from, to, total_rows, "row")?;

            // First row of the used range is treated as the table header (mirrors
            // the CSV behavior); remaining rows are data.
            let mut header: Option<Vec<String>> = None;
            let mut rows: Vec<Vec<String>> = Vec::new();
            for r in resolved {
                let cells: Vec<String> = (0..total_cols)
                    .map(|c| cell_to_string(range.get((r, c)).unwrap_or(&Data::Empty)))
                    .collect();
                if r == 0 {
                    header = Some(cells);
                } else {
                    rows.push(cells);
                }
            }
            Ok::<_, ExtractionError>(render_table(header, rows))
        });
        match result {
            Ok(Ok(md)) => Ok(md),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(ExtractionError::Internal(
                "spreadsheet reader panicked while extracting a sheet".into(),
            )),
        }
    }
}

/// Build an `Arc<dyn SpreadsheetReader>` for wiring/composition.
pub fn make_spreadsheet_reader() -> Arc<dyn SpreadsheetReader> {
    Arc::new(CalamineSpreadsheetReader::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal in-memory `.xlsx` (OOXML zip) from a list of sheet
    /// descriptors. `Some(name)` produces a valid worksheet; `None` produces a
    /// sheet whose worksheet XML is deliberately broken so
    /// `worksheet_range` fails. Passing an empty list yields a workbook with no
    /// sheets.
    fn make_xlsx(sheets: &[Option<&str>]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
</Types>"#,
        )
        .unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        )
        .unwrap();

        let mut wb = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#,
        );
        let mut rels = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, s) in sheets.iter().enumerate() {
            let idx = i + 1;
            let name = s.unwrap_or("SheetX");
            wb.push_str(&format!(
                r#"<sheet name="{name}" sheetId="{idx}" r:id="rId{idx}"/>"#
            ));
            rels.push_str(&format!(
                r#"<Relationship Id="rId{idx}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{idx}.xml"/>"#
            ));
        }
        wb.push_str("</sheets></workbook>");
        rels.push_str("</Relationships>");
        zip.start_file("xl/workbook.xml", opts).unwrap();
        zip.write_all(wb.as_bytes()).unwrap();
        zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();

        for (i, s) in sheets.iter().enumerate() {
            let idx = i + 1;
            let body = match s {
                Some(_) => r#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Hello</t></is></c></row></sheetData></worksheet>"#.to_string(),
                // Deliberately broken worksheet XML -> worksheet_range fails.
                None => "<worksheet><sheetData><broken".to_string(),
            };
            zip.start_file(format!("xl/worksheets/sheet{idx}.xml"), opts)
                .unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn cell_to_string_handles_variants() {
        assert_eq!(cell_to_string(&Data::Int(5)), "5");
        assert_eq!(cell_to_string(&Data::String("hi".into())), "hi");
        assert_eq!(cell_to_string(&Data::Empty), "");
    }

    #[test]
    fn constructors() {
        let a = CalamineSpreadsheetReader::new();
        let b = make_spreadsheet_reader();
        let _ = (a, b);
    }

    #[test]
    fn extract_sheet_not_found_is_malformed() {
        let r = CalamineSpreadsheetReader::new();
        let err = r
            .extract_sheet(
                &Source::Bytes(make_xlsx(&[Some("Sheet1")])),
                Some("Nope"),
                None,
            )
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
    }

    #[test]
    fn extract_sheet_with_no_sheets_is_malformed() {
        let r = CalamineSpreadsheetReader::new();
        let err = r
            .extract_sheet(&Source::Bytes(make_xlsx(&[])), None, None)
            .unwrap_err();
        assert!(matches!(err, ExtractionError::Malformed(_)));
        assert!(err.to_string().contains("no sheets"));
    }

    #[test]
    fn spreadsheet_info_broken_sheet_range_is_none() {
        // A workbook whose (only) sheet has a broken worksheet part: the sheet
        // is still listed but its range resolution fails -> (None, None).
        let r = CalamineSpreadsheetReader::new();
        let info = r
            .spreadsheet_info(&Source::Bytes(make_xlsx(&[None])))
            .unwrap();
        assert_eq!(info.sheets.len(), 1);
        assert_eq!(info.sheets[0].rows, None);
        assert_eq!(info.sheets[0].cols, None);
    }

    #[test]
    fn spreadsheet_info_on_garbage_is_mapped_error() {
        let r = CalamineSpreadsheetReader::new();
        let err = r
            .spreadsheet_info(&Source::Bytes(b"definitely not a workbook".to_vec()))
            .unwrap_err();
        assert!(matches!(
            err,
            ExtractionError::Malformed(_) | ExtractionError::Io(_)
        ));
    }

    #[test]
    fn extract_sheet_by_name_success() {
        let r = CalamineSpreadsheetReader::new();
        let md = r
            .extract_sheet(
                &Source::Bytes(make_xlsx(&[Some("Sheet1")])),
                Some("Sheet1"),
                None,
            )
            .unwrap();
        assert!(md.contains("Hello"));
    }

    #[test]
    fn extract_sheet_renders_multiple_data_rows() {
        // Build a workbook whose sheet has a header plus two data rows so the
        // row-push path (rows r > 0) is exercised.
        let cursor = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(cursor);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
</Types>"#,
        )
        .unwrap();
        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"#,
        )
        .unwrap();
        zip.start_file("xl/workbook.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="S" sheetId="1" r:id="rId1"/></sheets></workbook>"#,
        )
        .unwrap();
        zip.start_file("xl/_rels/workbook.xml.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"#,
        )
        .unwrap();
        zip.start_file("xl/worksheets/sheet1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData>
<row r="1"><c r="A1" t="inlineStr"><is><t>Hdr</t></is></c></row>
<row r="2"><c r="A2" t="inlineStr"><is><t>Data1</t></is></c></row>
<row r="3"><c r="A3" t="inlineStr"><is><t>Data2</t></is></c></row>
</sheetData></worksheet>"#,
        )
        .unwrap();
        let bytes = zip.finish().unwrap().into_inner();

        let r = CalamineSpreadsheetReader::new();
        let md = r.extract_sheet(&Source::Bytes(bytes), None, None).unwrap();
        assert!(md.contains("Data1"));
        assert!(md.contains("Data2"));
    }
}
