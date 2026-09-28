//! LSP positions count UTF-16 code units, while the rule engine uses UTF-8 bytes.

use serde::{Deserialize, Serialize};

use crate::diagnostics::Span;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(super) struct Position {
    pub line: u32,
    pub character: u32,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(super) struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Change {
    pub range: Option<Range>,
    pub range_length: Option<u32>,
    pub text: String,
}

pub(super) struct TextMap<'a> {
    source: &'a str,
    lines: Vec<Span>,
}

impl<'a> TextMap<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        let mut cursor = 0;
        let bytes = source.as_bytes();
        while cursor < bytes.len() {
            if matches!(bytes[cursor], b'\r' | b'\n') {
                lines.push(Span::new(start, cursor));
                if bytes[cursor] == b'\r' && bytes.get(cursor + 1) == Some(&b'\n') {
                    cursor += 1;
                }
                start = cursor + 1;
            }
            cursor += 1;
        }
        lines.push(Span::new(start, source.len()));
        Self { source, lines }
    }

    pub fn position(&self, offset: usize) -> Position {
        let mut offset = offset.min(self.source.len());
        while !self.source.is_char_boundary(offset) {
            offset -= 1;
        }
        let line = self.lines.partition_point(|line| line.start <= offset) - 1;
        let span = self.lines[line];
        Position {
            line: line as u32,
            character: self.source[span.start..offset.min(span.end)]
                .encode_utf16()
                .count() as u32,
        }
    }

    pub fn range(&self, span: Span) -> Range {
        Range {
            start: self.position(span.start),
            end: self.position(span.end),
        }
    }

    pub fn offset(&self, position: Position) -> Result<usize, String> {
        let line = self
            .lines
            .get(position.line as usize)
            .ok_or("Line is outside the document.")?;
        let mut units = 0;
        for (offset, ch) in self.source[line.start..line.end].char_indices() {
            if units == position.character {
                return Ok(line.start + offset);
            }
            units += ch.len_utf16() as u32;
            if units > position.character {
                return Err("Position splits a UTF-16 surrogate pair.".into());
            }
        }
        // LSP specifies clamping positions beyond the end of a line.
        Ok(line.end)
    }

    pub fn span(&self, range: Range) -> Result<Span, String> {
        if range.start > range.end {
            return Err("Range starts after its end.".into());
        }
        Ok(Span::new(
            self.offset(range.start)?,
            self.offset(range.end)?,
        ))
    }
}

/// Apply a notification atomically, interpreting each range against the preceding edit.
pub(super) fn apply_changes(source: &str, changes: &[Change]) -> Result<String, String> {
    let mut updated = source.to_owned();
    for change in changes {
        if let Some(range) = change.range {
            let span = TextMap::new(&updated).span(range)?;
            if change.range_length.is_some_and(|length| {
                updated[span.start..span.end].encode_utf16().count() != length as usize
            }) {
                return Err("rangeLength does not match the replaced UTF-16 text.".into());
            }
            updated.replace_range(span.start..span.end, &change.text);
        } else {
            updated.clone_from(&change.text);
        }
    }
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_round_trips_multilingual_lines_and_line_endings() {
        let source = "中🦀e\u{301}\r\n日本\rabc\n";
        let map = TextMap::new(source);
        for (offset, ch) in source.char_indices() {
            if !matches!(ch, '\r' | '\n') {
                assert_eq!(map.offset(map.position(offset)).unwrap(), offset);
            }
        }
        assert_eq!(
            map.position(7),
            Position {
                line: 0,
                character: 3
            }
        );
        assert_eq!(
            map.offset(map.position(source.len())).unwrap(),
            source.len()
        );
        assert!(
            map.offset(Position {
                line: 0,
                character: 2
            })
            .is_err()
        );
        assert_eq!(
            map.offset(Position {
                line: 0,
                character: 999
            })
            .unwrap(),
            10
        );
    }

    #[test]
    fn sequential_edits_use_the_updated_source() {
        let changes: Vec<Change> = serde_json::from_value(json!([
            {"range":{"start":{"line":0,"character":1},"end":{"line":0,"character":3}},"rangeLength":2,"text":"日本"},
            {"range":{"start":{"line":0,"character":3},"end":{"line":1,"character":1}},"text":"!"}
        ])).unwrap();
        assert_eq!(apply_changes("中🦀\r\nx", &changes).unwrap(), "中日本!");
    }

    #[test]
    fn malformed_edit_batch_does_not_change_original_text() {
        let source = String::from("🦀");
        let changes: Vec<Change> = serde_json::from_value(json!([
            {"text":"a🦀"},
            {"range":{"start":{"line":0,"character":2},"end":{"line":0,"character":3}},"text":"x"}
        ]))
        .unwrap();
        assert!(apply_changes(&source, &changes).is_err());
        assert_eq!(source, "🦀");
    }
}
