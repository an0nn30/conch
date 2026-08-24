use async_lsp::lsp_types::{Position, Range, TextDocumentContentChangeEvent};
use ropey::Rope;

use super::types::LspChangeBatch;
use crate::editor_fs::MAX_EDIT_BYTES;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DocumentError {
    InvalidUtf16Offset(u32),
    InvalidPosition(Position),
    InvalidRange { from_utf16: u32, to_utf16: u32 },
    VersionMismatch { expected: i32, actual: i32 },
    NonMonotonicVersion { base: i32, next: i32 },
    DocumentIdMismatch { expected: String, actual: String },
    OverlappingChanges,
    TooLarge { size: usize, max: usize },
}

pub(crate) struct DocumentText {
    rope: Rope,
}

struct ResolvedOffset {
    char_index: usize,
    position: Position,
}

impl DocumentText {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
        }
    }

    pub(crate) fn position_at_utf16_offset(&self, target: u32) -> Result<Position, DocumentError> {
        Ok(self.resolve_utf16_offset(target)?.position)
    }

    fn resolve_utf16_offset(&self, target: u32) -> Result<ResolvedOffset, DocumentError> {
        let mut offset = 0_u32;
        let mut line = 0_u32;
        let mut character = 0_u32;
        let mut char_index = 0_usize;

        let mut scalars = self.rope.chars().peekable();
        while let Some(scalar) = scalars.next() {
            if offset == target {
                return Ok(ResolvedOffset {
                    char_index,
                    position: Position::new(line, character),
                });
            }

            let is_crlf = scalar == '\r' && scalars.peek() == Some(&'\n');
            let width = if is_crlf {
                2
            } else {
                scalar.len_utf16() as u32
            };
            if target < offset + width {
                return Err(DocumentError::InvalidUtf16Offset(target));
            }
            offset += width;

            if is_crlf {
                scalars.next();
                char_index += 1;
            }
            char_index += 1;
            if is_crlf || matches!(scalar, '\r' | '\n') {
                line += 1;
                character = 0;
            } else {
                character += width;
            }
        }

        if offset == target {
            Ok(ResolvedOffset {
                char_index,
                position: Position::new(line, character),
            })
        } else {
            Err(DocumentError::InvalidUtf16Offset(target))
        }
    }

    pub(crate) fn utf16_offset_at(&self, target: Position) -> Result<u32, DocumentError> {
        let mut offset = 0_u32;
        let mut line = 0_u32;
        let mut character = 0_u32;

        let mut scalars = self.rope.chars().peekable();
        while let Some(scalar) = scalars.next() {
            if line == target.line && character == target.character {
                return Ok(offset);
            }

            let is_crlf = scalar == '\r' && scalars.peek() == Some(&'\n');
            let width = if is_crlf {
                2
            } else {
                scalar.len_utf16() as u32
            };
            offset += width;
            if is_crlf {
                scalars.next();
            }
            if is_crlf || matches!(scalar, '\r' | '\n') {
                if line == target.line {
                    return Err(DocumentError::InvalidPosition(target));
                }
                line += 1;
                character = 0;
            } else {
                character += width;
                if line == target.line && target.character < character {
                    return Err(DocumentError::InvalidPosition(target));
                }
            }
        }

        if line == target.line && character == target.character {
            Ok(offset)
        } else {
            Err(DocumentError::InvalidPosition(target))
        }
    }
}

pub(crate) struct AppliedBatch {
    pub(crate) version: i32,
    pub(crate) changes: Vec<TextDocumentContentChangeEvent>,
}

pub(crate) struct VersionedDocument {
    document_id: String,
    text: DocumentText,
    version: i32,
}

impl VersionedDocument {
    pub(crate) fn new(document_id: &str, text: &str, version: i32) -> Self {
        Self {
            document_id: document_id.to_owned(),
            text: DocumentText::new(text),
            version,
        }
    }

    pub(crate) fn text(&self) -> String {
        self.text.rope.to_string()
    }

    pub(crate) fn version(&self) -> i32 {
        self.version
    }

    pub(crate) fn resync(&mut self, text: &str, next_version: i32) -> Result<(), DocumentError> {
        if next_version <= self.version {
            return Err(DocumentError::NonMonotonicVersion {
                base: self.version,
                next: next_version,
            });
        }
        if text.len() > MAX_EDIT_BYTES as usize {
            return Err(DocumentError::TooLarge {
                size: text.len(),
                max: MAX_EDIT_BYTES as usize,
            });
        }

        self.text = DocumentText::new(text);
        self.version = next_version;
        Ok(())
    }

    pub(crate) fn apply_batch(
        &mut self,
        batch: LspChangeBatch,
    ) -> Result<AppliedBatch, DocumentError> {
        if batch.document_id != self.document_id {
            return Err(DocumentError::DocumentIdMismatch {
                expected: self.document_id.clone(),
                actual: batch.document_id,
            });
        }
        if batch.base_version != self.version {
            return Err(DocumentError::VersionMismatch {
                expected: self.version,
                actual: batch.base_version,
            });
        }
        if batch.next_version <= batch.base_version {
            return Err(DocumentError::NonMonotonicVersion {
                base: batch.base_version,
                next: batch.next_version,
            });
        }

        struct ValidatedChange {
            from_utf16: u32,
            to_utf16: u32,
            from_char: usize,
            to_char: usize,
            inserted_text: String,
            range: Range,
        }

        let mut validated = Vec::with_capacity(batch.changes.len());
        for change in batch.changes {
            if change.from_utf16 > change.to_utf16 {
                return Err(DocumentError::InvalidRange {
                    from_utf16: change.from_utf16,
                    to_utf16: change.to_utf16,
                });
            }
            let from = self.text.resolve_utf16_offset(change.from_utf16)?;
            let to = self.text.resolve_utf16_offset(change.to_utf16)?;
            validated.push(ValidatedChange {
                from_utf16: change.from_utf16,
                to_utf16: change.to_utf16,
                from_char: from.char_index,
                to_char: to.char_index,
                inserted_text: change.inserted_text,
                range: Range::new(from.position, to.position),
            });
        }

        validated.sort_by_key(|change| (change.from_utf16, change.to_utf16));
        for pair in validated.windows(2) {
            if pair[1].from_utf16 < pair[0].to_utf16 || pair[1].from_utf16 == pair[0].from_utf16 {
                return Err(DocumentError::OverlappingChanges);
            }
        }

        let mut resulting_size = self.text.rope.len_bytes();
        for change in &validated {
            let removed_bytes = self.text.rope.char_to_byte(change.to_char)
                - self.text.rope.char_to_byte(change.from_char);
            resulting_size = resulting_size
                .checked_sub(removed_bytes)
                .and_then(|size| size.checked_add(change.inserted_text.len()))
                .ok_or(DocumentError::TooLarge {
                    size: usize::MAX,
                    max: MAX_EDIT_BYTES as usize,
                })?;
        }
        if resulting_size > MAX_EDIT_BYTES as usize {
            return Err(DocumentError::TooLarge {
                size: resulting_size,
                max: MAX_EDIT_BYTES as usize,
            });
        }

        validated.reverse();

        let mut protocol_changes = Vec::with_capacity(validated.len());
        for change in validated {
            self.text.rope.remove(change.from_char..change.to_char);
            self.text
                .rope
                .insert(change.from_char, &change.inserted_text);
            protocol_changes.push(TextDocumentContentChangeEvent {
                range: Some(change.range),
                range_length: None,
                text: change.inserted_text,
            });
        }
        self.version = batch.next_version;

        Ok(AppliedBatch {
            version: self.version,
            changes: protocol_changes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{DocumentError, DocumentText, VersionedDocument};
    use crate::editor_fs::MAX_EDIT_BYTES;
    use crate::lsp::types::{LspChangeBatch, LspTextChange};
    use async_lsp::lsp_types::{Position, Range};

    fn change_batch<const N: usize>(
        document_id: &str,
        base_version: i32,
        next_version: i32,
        changes: [(u32, u32, &str); N],
    ) -> LspChangeBatch {
        LspChangeBatch {
            document_id: document_id.into(),
            base_version,
            next_version,
            changes: changes
                .into_iter()
                .map(|(from_utf16, to_utf16, inserted_text)| LspTextChange {
                    from_utf16,
                    to_utf16,
                    inserted_text: inserted_text.into(),
                })
                .collect(),
        }
    }

    #[test]
    fn ascii_offsets_round_trip() {
        let doc = DocumentText::new("abc\ndef");

        assert_eq!(
            doc.position_at_utf16_offset(5).unwrap(),
            Position::new(1, 1)
        );
        assert_eq!(doc.utf16_offset_at(Position::new(1, 1)).unwrap(), 5);
    }

    #[test]
    fn utf16_offsets_round_trip_across_emoji() {
        let doc = DocumentText::new("a😀b\nç");

        assert_eq!(
            doc.position_at_utf16_offset(3).unwrap(),
            Position::new(0, 3)
        );
        assert_eq!(doc.utf16_offset_at(Position::new(1, 1)).unwrap(), 6);
    }

    #[test]
    fn combining_marks_and_bmp_scalars_each_count_as_one_utf16_unit() {
        let doc = DocumentText::new("e\u{301}中\nß");

        assert_eq!(
            doc.position_at_utf16_offset(3).unwrap(),
            Position::new(0, 3)
        );
        assert_eq!(doc.utf16_offset_at(Position::new(1, 1)).unwrap(), 5);
    }

    #[test]
    fn end_of_document_after_trailing_newline_is_addressable() {
        let doc = DocumentText::new("😀\n");

        assert_eq!(
            doc.position_at_utf16_offset(3).unwrap(),
            Position::new(1, 0)
        );
        assert_eq!(doc.utf16_offset_at(Position::new(1, 0)).unwrap(), 3);
    }

    #[test]
    fn rejects_offsets_inside_a_surrogate_pair() {
        let doc = DocumentText::new("a😀b");

        assert_eq!(
            doc.position_at_utf16_offset(2),
            Err(DocumentError::InvalidUtf16Offset(2))
        );
        assert_eq!(
            doc.utf16_offset_at(Position::new(0, 2)),
            Err(DocumentError::InvalidPosition(Position::new(0, 2)))
        );
    }

    #[test]
    fn rejects_positions_and_offsets_past_the_document() {
        let doc = DocumentText::new("ab\nç");

        assert_eq!(
            doc.position_at_utf16_offset(5),
            Err(DocumentError::InvalidUtf16Offset(5))
        );
        assert_eq!(
            doc.utf16_offset_at(Position::new(1, 2)),
            Err(DocumentError::InvalidPosition(Position::new(1, 2)))
        );
        assert_eq!(
            doc.utf16_offset_at(Position::new(2, 0)),
            Err(DocumentError::InvalidPosition(Position::new(2, 0)))
        );
    }

    #[test]
    fn crlf_is_one_unaddressable_line_ending() {
        let doc = DocumentText::new("a\r\nb");

        assert_eq!(
            doc.position_at_utf16_offset(1).unwrap(),
            Position::new(0, 1)
        );
        assert_eq!(
            doc.position_at_utf16_offset(3).unwrap(),
            Position::new(1, 0)
        );
        assert_eq!(doc.utf16_offset_at(Position::new(1, 0)).unwrap(), 3);
        assert_eq!(
            doc.position_at_utf16_offset(2),
            Err(DocumentError::InvalidUtf16Offset(2))
        );
    }

    #[test]
    fn applies_code_mirror_changes_high_to_low() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abcdef", 1);
        let batch = LspChangeBatch {
            document_id: "file:///tmp/a.ts".into(),
            base_version: 1,
            next_version: 2,
            changes: vec![
                LspTextChange {
                    from_utf16: 4,
                    to_utf16: 6,
                    inserted_text: "Z".into(),
                },
                LspTextChange {
                    from_utf16: 1,
                    to_utf16: 2,
                    inserted_text: "XY".into(),
                },
            ],
        };

        doc.apply_batch(batch).unwrap();

        assert_eq!(doc.text(), "aXYcdZ");
        assert_eq!(doc.version(), 2);
    }

    #[test]
    fn derives_every_lsp_range_from_the_pre_change_snapshot() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "a😀b\nçd", 8);
        let batch = LspChangeBatch {
            document_id: "file:///tmp/a.ts".into(),
            base_version: 8,
            next_version: 9,
            changes: vec![
                LspTextChange {
                    from_utf16: 1,
                    to_utf16: 3,
                    inserted_text: "X".into(),
                },
                LspTextChange {
                    from_utf16: 6,
                    to_utf16: 7,
                    inserted_text: "Z".into(),
                },
            ],
        };

        let applied = doc.apply_batch(batch).unwrap();

        assert_eq!(doc.text(), "aXb\nçZ");
        assert_eq!(applied.version, 9);
        assert_eq!(applied.changes.len(), 2);
        assert_eq!(
            applied.changes[0].range,
            Some(Range::new(Position::new(1, 1), Position::new(1, 2)))
        );
        assert_eq!(applied.changes[0].text, "Z");
        assert_eq!(
            applied.changes[1].range,
            Some(Range::new(Position::new(0, 1), Position::new(0, 3)))
        );
        assert_eq!(applied.changes[1].text, "X");
    }

    #[test]
    fn rejects_two_insertions_at_the_same_snapshot_offset() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abc", 4);
        let batch = LspChangeBatch {
            document_id: "file:///tmp/a.ts".into(),
            base_version: 4,
            next_version: 5,
            changes: vec![
                LspTextChange {
                    from_utf16: 1,
                    to_utf16: 1,
                    inserted_text: "X".into(),
                },
                LspTextChange {
                    from_utf16: 1,
                    to_utf16: 1,
                    inserted_text: "Y".into(),
                },
            ],
        };

        assert_eq!(
            doc.apply_batch(batch).map(|_| ()),
            Err(DocumentError::OverlappingChanges)
        );
        assert_eq!(doc.text(), "abc");
        assert_eq!(doc.version(), 4);
    }

    #[test]
    fn stale_base_version_requires_resync_without_mutating() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abc", 7);

        let result = doc.apply_batch(change_batch("file:///tmp/a.ts", 6, 8, [(0, 1, "X")]));

        assert_eq!(
            result.map(|_| ()),
            Err(DocumentError::VersionMismatch {
                expected: 7,
                actual: 6,
            })
        );
        assert_eq!(doc.text(), "abc");
        assert_eq!(doc.version(), 7);
    }

    #[test]
    fn rejects_non_monotonic_next_version() {
        for next_version in [4, 3] {
            let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abc", 4);

            let result = doc.apply_batch(change_batch(
                "file:///tmp/a.ts",
                4,
                next_version,
                [(0, 1, "X")],
            ));

            assert_eq!(
                result.map(|_| ()),
                Err(DocumentError::NonMonotonicVersion {
                    base: 4,
                    next: next_version,
                })
            );
            assert_eq!(doc.text(), "abc");
            assert_eq!(doc.version(), 4);
        }
    }

    #[test]
    fn invalid_later_change_rejects_the_whole_batch_atomically() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "a😀b", 1);
        let result = doc.apply_batch(change_batch(
            "file:///tmp/a.ts",
            1,
            2,
            [(0, 1, "X"), (2, 3, "Y")],
        ));

        assert_eq!(
            result.map(|_| ()),
            Err(DocumentError::InvalidUtf16Offset(2))
        );
        assert_eq!(doc.text(), "a😀b");
        assert_eq!(doc.version(), 1);
    }

    #[test]
    fn rejects_reversed_change_range() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abc", 1);

        let result = doc.apply_batch(change_batch("file:///tmp/a.ts", 1, 2, [(3, 1, "X")]));

        assert_eq!(
            result.map(|_| ()),
            Err(DocumentError::InvalidRange {
                from_utf16: 3,
                to_utf16: 1,
            })
        );
        assert_eq!(doc.text(), "abc");
        assert_eq!(doc.version(), 1);
    }

    #[test]
    fn rejects_overlapping_snapshot_ranges() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "abcdef", 1);

        let result = doc.apply_batch(change_batch(
            "file:///tmp/a.ts",
            1,
            2,
            [(1, 4, "X"), (3, 5, "Y")],
        ));

        assert_eq!(result.map(|_| ()), Err(DocumentError::OverlappingChanges));
        assert_eq!(doc.text(), "abcdef");
        assert_eq!(doc.version(), 1);
    }

    #[test]
    fn applies_insertion_at_end_of_document() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "a😀", 1);

        let applied = doc
            .apply_batch(change_batch("file:///tmp/a.ts", 1, 3, [(3, 3, "\n")]))
            .unwrap();

        assert_eq!(doc.text(), "a😀\n");
        assert_eq!(doc.version(), 3);
        assert_eq!(
            applied.changes[0].range,
            Some(Range::new(Position::new(0, 3), Position::new(0, 3)))
        );
    }

    #[test]
    fn rejects_batch_that_exceeds_editor_size_limit() {
        let max = MAX_EDIT_BYTES as usize;
        let original = "a".repeat(max);
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", &original, 1);

        let result = doc.apply_batch(change_batch(
            "file:///tmp/a.ts",
            1,
            2,
            [(max as u32, max as u32, "b")],
        ));

        assert_eq!(
            result.map(|_| ()),
            Err(DocumentError::TooLarge { size: max + 1, max })
        );
        assert_eq!(doc.text().len(), max);
        assert_eq!(doc.version(), 1);
    }

    #[test]
    fn explicit_resync_replaces_text_at_the_supplied_newer_version() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "stale", 7);

        doc.resync("fresh😀\n", 10).unwrap();

        assert_eq!(doc.text(), "fresh😀\n");
        assert_eq!(doc.version(), 10);
    }

    #[test]
    fn explicit_resync_rejects_a_non_monotonic_version() {
        let mut doc = VersionedDocument::new("file:///tmp/a.ts", "current", 7);

        let result = doc.resync("stale", 7);

        assert_eq!(
            result,
            Err(DocumentError::NonMonotonicVersion { base: 7, next: 7 })
        );
        assert_eq!(doc.text(), "current");
        assert_eq!(doc.version(), 7);
    }
}
