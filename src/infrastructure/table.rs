//! Shared Markdown-table rendering used by the spreadsheet and CSV adapters.

/// Render a Markdown table from an optional header row and data rows.
///
/// Pipes within cells are escaped; short rows are padded to the widest row so
/// every column is present.
pub fn render_table(header: Option<Vec<String>>, rows: Vec<Vec<String>>) -> String {
    let max_data = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let ncols = header
        .as_ref()
        .map(|h| h.len())
        .max(Some(max_data))
        .unwrap_or(0);
    if ncols == 0 {
        return String::new();
    }
    let pad = |row: &[String]| {
        let mut v = row.to_vec();
        while v.len() < ncols {
            v.push(String::new());
        }
        v
    };
    let mut out = String::new();
    let write_row = |cells: Vec<String>, out: &mut String| {
        out.push('|');
        for c in cells {
            out.push(' ');
            out.push_str(&c.replace('|', "\\|"));
            out.push_str(" |");
        }
        out.push('\n');
    };

    if let Some(h) = header {
        write_row(pad(&h), &mut out);
        out.push('|');
        for _ in 0..ncols {
            out.push_str(" --- |");
        }
        out.push('\n');
    }
    for r in rows {
        write_row(pad(&r), &mut out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_table() {
        let md = render_table(
            Some(vec!["a".into(), "b".into()]),
            vec![vec!["1".into(), "2".into()]],
        );
        assert!(md.contains("| a | b |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| 1 | 2 |"));
    }

    #[test]
    fn escapes_and_pads() {
        let md = render_table(Some(vec!["a".into()]), vec![vec!["x|y".into(), "z".into()]]);
        assert!(md.contains("| a |  |"));
        assert!(md.contains("| x\\|y | z |"));
    }

    #[test]
    fn empty() {
        assert_eq!(render_table(None, vec![]), "");
    }
}
