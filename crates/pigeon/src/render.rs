//! How results read: the fields an action's columns name, each cell as
//! short text, and the whole result as text for the command line.

use std::fmt::Write;

use serde_json::Value;

use crate::catalog::Action;

/// The field at the dotted path `column` of `item`, or null.
#[must_use]
pub fn field<'a>(item: &'a Value, column: &str) -> &'a Value {
    column
        .split('.')
        .try_fold(item, |value, name| value.get(name))
        .unwrap_or(&Value::Null)
}

/// A byte count in the largest unit that keeps it at least 1.
#[must_use]
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1000 * 1000 && unit + 1 < UNITS.len() {
        value /= 1000;
        unit += 1;
    }
    if value >= 1000 && unit + 1 < UNITS.len() {
        let tenths = value / 100;
        format!("{}.{} {}", tenths / 10, tenths % 10, UNITS[unit + 1])
    } else {
        format!("{value} {}", UNITS[unit])
    }
}

/// One value as a table cell: flags as yes or nothing, sizes in units,
/// nested values as compact JSON.
#[must_use]
pub fn cell(column: &str, value: &Value) -> String {
    match value {
        Value::Null | Value::Bool(false) => String::new(),
        Value::Bool(true) => "yes".to_owned(),
        Value::Number(number) if column.ends_with("size") => {
            number.as_u64().map_or_else(|| number.to_string(), size)
        }
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| cell(column, item))
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(_) => value.to_string(),
    }
}

/// The header of `column`: its last name.
#[must_use]
pub fn header(column: &str) -> &str {
    column.rsplit('.').next().unwrap_or(column)
}

/// `items` as an aligned table of `columns`.
fn table(columns: &[&str], items: &[Value]) -> String {
    let rows: Vec<Vec<String>> = items
        .iter()
        .map(|item| {
            columns
                .iter()
                .map(|column| cell(column, field(item, column)))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = columns.iter().map(|column| header(column).len()).collect();
    for row in &rows {
        for (width, text) in widths.iter_mut().zip(row) {
            *width = (*width).max(text.chars().count());
        }
    }
    let mut out = String::new();
    let headers = columns.iter().map(|column| header(column).to_uppercase());
    for line in std::iter::once(headers.collect::<Vec<_>>()).chain(rows) {
        let mut text = String::new();
        for (text_cell, width) in line.iter().zip(&widths) {
            let _ = write!(text, "{text_cell:width$}  ");
        }
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out
}

/// The result of `action` as text for a person.
#[must_use]
pub fn text(action: &Action, result: &Value) -> String {
    match result {
        Value::Null => String::new(),
        Value::Array(items) if items.is_empty() => "nothing\n".to_owned(),
        Value::Array(items) if !action.columns.is_empty() => table(action.columns, items),
        Value::Object(fields) => {
            let width = fields.keys().map(String::len).max().unwrap_or(0);
            let mut out = String::new();
            for (name, value) in fields {
                let _ = writeln!(out, "{name:width$}  {}", cell(name, value));
            }
            out
        }
        other => format!("{}\n", cell("", other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::find;
    use serde_json::json;

    #[test]
    fn sizes_keep_three_significant_digits() {
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_234), "1.2 KB");
        assert_eq!(size(12_345_678), "12.3 MB");
        assert_eq!(size(5_000_000_000_000_000), "5000 TB");
    }

    #[test]
    fn lists_read_as_tables_of_their_columns() {
        let files = json!([
            {"path": "@alice/a.txt", "owner": "alice", "content": {"size": 2048}, "time": "t", "held": true, "outdated": false, "writable": true},
        ]);
        let text = text(find("file", "list").unwrap(), &files);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines[0],
            "PATH          OWNER  SIZE    TIME  HELD  OUTDATED  WRITABLE"
        );
        assert_eq!(
            lines[1],
            "@alice/a.txt  alice  2.0 KB  t     yes             yes"
        );
        let key = json!({"key": "cheapmo-abc"});
        assert_eq!(
            super::text(find("group", "key").unwrap(), &key),
            "key  cheapmo-abc\n"
        );
    }
}
