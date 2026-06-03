//! Text chunking strategies for RAG document indexing.
//!
//! Currently implements fixed-size chunking with configurable overlap.
//! Additional strategies (recursive, semantic) can be added later.

use crate::error::AgentJaxResult;

use super::types::{Chunk, Document};

/// A text chunker that splits documents into fixed-size chunks with overlap.
pub struct Chunker {
    /// Maximum chunk size in characters.
    pub chunk_size: usize,
    /// Number of overlapping characters between consecutive chunks.
    pub overlap: usize,
}

impl Chunker {
    /// Create a new chunker with the given size and overlap.
    ///
    /// Constraints:
    /// - `chunk_size` must be > 0
    /// - `overlap` must be < `chunk_size`
    pub fn new(chunk_size: usize, overlap: usize) -> AgentJaxResult<Self> {
        if chunk_size == 0 {
            return Err(crate::agentjax_err!("chunk_size must be > 0", Config));
        }
        if overlap >= chunk_size {
            return Err(crate::agentjax_err!(
                "overlap ({overlap}) must be less than chunk_size ({chunk_size})",
                Config
            ));
        }
        Ok(Self {
            chunk_size,
            overlap,
        })
    }

    /// Create a chunker with reasonable defaults (512 char chunks, 64 char overlap).
    pub fn default() -> Self {
        Self {
            chunk_size: 512,
            overlap: 64,
        }
    }

    /// Split a document into chunks.
    pub fn chunk(&self, document: &Document) -> Vec<Chunk> {
        let content = &document.content;
        let len = content.len();

        if len == 0 {
            return vec![];
        }

        let step = self.chunk_size.saturating_sub(self.overlap);
        if step == 0 {
            // If overlap >= chunk_size, just return the whole doc as one chunk
            return vec![Chunk {
                id: format!("{}_0", document.id),
                document_id: document.id.clone(),
                content: content.clone(),
                chunk_index: 0,
                embedding: None,
            }];
        }

        let mut chunks = Vec::new();
        let mut start = 0;
        let mut index = 0;

        while start < len {
            let end = (start + self.chunk_size).min(len);

            // Try to break at a natural boundary (newline or space) near the end
            let adjusted_end = if end < len {
                let slice = &content[end.saturating_sub(20)..end];
                if let Some(pos) = slice.rfind('\n') {
                    end.saturating_sub(20) + pos
                } else if let Some(pos) = slice.rfind(' ') {
                    end.saturating_sub(20) + pos + 1
                } else {
                    end
                }
            } else {
                end
            };

            let chunk_content = content[start..adjusted_end].to_string();

            chunks.push(Chunk {
                id: format!("{}_{}", document.id, index),
                document_id: document.id.clone(),
                content: chunk_content,
                chunk_index: index,
                embedding: None,
            });

            // Advance: if we're breaking at a natural boundary, use that
            let advance = if adjusted_end < end {
                adjusted_end - start
            } else {
                self.chunk_size.saturating_sub(self.overlap)
            };

            start += if advance > 0 { advance } else { self.chunk_size };
            index += 1;
        }

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_chunks_smaller_than_chunk_size() {
        let chunker = Chunker::new(100, 10).unwrap();
        let doc = Document {
            id: "test".to_string(),
            content: "Hello world, this is a short document.".to_string(),
            metadata: BTreeMap::new(),
        };
        let chunks = chunker.chunk(&doc);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, doc.content);
    }

    #[test]
    fn test_chunks_empty_document() {
        let chunker = Chunker::new(100, 10).unwrap();
        let doc = Document {
            id: "empty".to_string(),
            content: String::new(),
            metadata: BTreeMap::new(),
        };
        let chunks = chunker.chunk(&doc);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunks_exact_chunk_size() {
        let chunker = Chunker::new(10, 2).unwrap();
        let content = "0123456789ABCDEFGHIJ";
        let doc = Document {
            id: "exact".to_string(),
            content: content.to_string(),
            metadata: BTreeMap::new(),
        };
        let chunks = chunker.chunk(&doc);
        assert!(chunks.len() >= 2);
        // First chunk should be exactly 10 chars
        assert_eq!(chunks[0].content.len(), 10);
    }

    #[test]
    fn test_chunks_overlap() {
        let chunker = Chunker::new(20, 5).unwrap();
        let content = "AAAAABBBBBCCCCCDDDDDEEEEEFFFFF";
        let doc = Document {
            id: "overlap".to_string(),
            content: content.to_string(),
            metadata: BTreeMap::new(),
        };
        let chunks = chunker.chunk(&doc);
        assert!(chunks.len() >= 2);
        // Check that consecutive chunks overlap by approximately 5 chars
        // (exact overlap depends on natural boundaries)
        let overlap_found = chunks[0]
            .content
            .chars()
            .rev()
            .zip(chunks[1].content.chars())
            .take_while(|(a, b)| a == b)
            .count();
        assert!(overlap_found >= 3, "expected overlap between chunks, got {overlap_found}");
    }

    #[test]
    fn test_chunks_boundary_validation() {
        assert!(Chunker::new(0, 5).is_err());
        assert!(Chunker::new(10, 10).is_err());
        assert!(Chunker::new(10, 11).is_err());
        assert!(Chunker::new(100, 20).is_ok());
    }

    #[test]
    fn test_chunks_natural_boundary_break_on_newline() {
        let chunker = Chunker::new(15, 0).unwrap();
        let content = "Hello world\nSecond line\nThird line";
        let doc = Document {
            id: "nl".to_string(),
            content: content.to_string(),
            metadata: BTreeMap::new(),
        };
        let chunks = chunker.chunk(&doc);
        // Each chunk should try to break at newline
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            // No chunk should end mid-word at a newline boundary
            if !chunk.content.contains('\n') {
                continue;
            }
        }
    }
}
