#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FileId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceSpan {
    pub file_id: FileId,
    pub byte_start: usize,
    pub byte_end: usize,
}

impl SourceSpan {
    pub const fn new(file_id: FileId, byte_start: usize, byte_end: usize) -> Self {
        Self {
            file_id,
            byte_start,
            byte_end,
        }
    }

    pub const fn len(self) -> usize {
        self.byte_end.saturating_sub(self.byte_start)
    }

    pub const fn is_empty(self) -> bool {
        self.byte_start == self.byte_end
    }
}
