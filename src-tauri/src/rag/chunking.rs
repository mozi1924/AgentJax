//! Markdown-aware text chunking for RAG document indexing.
//!
//! Uses break-point analysis with distance-decay scoring to find natural
//! split points at markdown structural boundaries (headings, code blocks,
//! horizontal rules, paragraphs, lists) rather than cutting mid-sentence.
//!
//! ## Algorithm (inspired by QMD, independently implemented)
//!
//! 1. **Scan break points** — Find all candidate split positions and score
//!    them by structure quality (h1=100, h2=90, ..., paragraph=20, newline=1).
//! 2. **Detect code fences** — Never split inside ``` fenced code blocks.
//! 3. **Distance-decay selection** — When a target cut position is too far
//!    past the last good break point, walk backward with squared-distance
//!    decay to find the best available split.
//! 4. **Overlap** — Overlap consecutive chunks by a configurable margin so
//!    context doesn't get lost at boundaries.

use crate::error::AgentJaxResult;

use super::types::{Chunk, Document};

// ── Default sizing constants ────────────────────────────────────────────────

/// Default maximum chunk size in characters (~900 tokens at 4 chars/token).
pub const DEFAULT_CHUNK_SIZE_CHARS: usize = 3600;
/// Default overlap between consecutive chunks (~135 tokens, 15%).
pub const DEFAULT_OVERLAP_CHARS: usize = 540;
/// Default search window for finding optimal break points (~200 tokens).
pub const DEFAULT_WINDOW_CHARS: usize = 800;

// ── Break-point detection ───────────────────────────────────────────────────

/// A potential split point in the document with a quality score.
#[derive(Debug, Clone)]
struct BreakPoint {
    /// Character position in the document.
    pos: usize,
    /// Quality score (higher = better place to split).
    score: u32,
}

/// A region delimited by ``` fences that must not be split.
#[derive(Debug, Clone)]
struct CodeFenceRegion {
    /// Position of the opening ``` (including leading newline).
    start: usize,
    /// Position just past the closing ```.
    end: usize,
}

/// (pattern_string, score, description) for break-point detection.
///
/// Patterns match the character *before* the break so that the break
/// position is `match.index`. Scores are spread wide so structural
/// boundaries decisively beat low-quality breaks.
const BREAK_PATTERNS: &[(&str, u32)] = &[
    ("\n# ", 100),       // h1 — best split point
    ("\n## ", 90),       // h2
    ("\n### ", 80),      // h3
    ("\n#### ", 70),     // h4
    ("\n##### ", 60),    // h5
    ("\n###### ", 50),   // h6
    ("\n```", 80),       // code block boundary (same weight as h3)
    ("\n---\n", 60),     // horizontal rule
    ("\n***\n", 60),     // horizontal rule (asterisks)
    ("\n___\n", 60),     // horizontal rule (underscores)
    ("\n\n", 20),        // paragraph boundary
    ("\n- ", 5),         // unordered list item
    ("\n* ", 5),         // unordered list item (asterisk)
    ("\n", 1),           // bare newline — minimal break
];

/// Scan document text for all candidate break points, keeping the
/// highest-scoring pattern at each position.
///
/// Prepends a synthetic newline so that headings at the very start of
/// the document are still detected; returned positions are adjusted back.
fn scan_break_points(text: &str) -> Vec<BreakPoint> {
    // Prepend a synthetic newline so `\n# ` matches a heading at position 0.
    let padded = format!("\n{text}");
    let mut seen: std::collections::BTreeMap<usize, u32> = std::collections::BTreeMap::new();

    for (pattern_str, score) in BREAK_PATTERNS {
        let mut search_start = 0;
        while let Some(found) = padded[search_start..].find(*pattern_str) {
            let pos_in_padded = search_start + found;
            // Adjust back: the real position in the original text is one less.
            let real_pos = pos_in_padded.saturating_sub(1);
            let entry = seen.entry(real_pos).or_insert(0);
            if *score > *entry {
                *entry = *score;
            }
            search_start = pos_in_padded + 1;
            if search_start >= padded.len() {
                break;
            }
        }
    }

    seen.into_iter()
        .map(|(pos, score)| BreakPoint { pos, score })
        .collect()
}

/// Find all ``` fenced code blocks in the text.
///
/// Returns regions that must not be split. Unclosed fences extend to
/// the end of the document.
fn find_code_fences(text: &str) -> Vec<CodeFenceRegion> {
    let mut regions = Vec::new();
    let mut in_fence = false;
    let mut fence_start = 0;
    let mut search_start = 0;

    while let Some(found) = text[search_start..].find("\n```") {
        let pos = search_start + found;
        if !in_fence {
            fence_start = pos;
            in_fence = true;
        } else {
            regions.push(CodeFenceRegion {
                start: fence_start,
                end: pos + "\n```".len(),
            });
            in_fence = false;
        }
        search_start = pos + 1;
        if search_start >= text.len() {
            break;
        }
    }

    // Unclosed fence extends to end of document.
    if in_fence {
        regions.push(CodeFenceRegion {
            start: fence_start,
            end: text.len(),
        });
    }

    regions
}

/// Check whether a position falls inside any code fence region.
fn is_inside_code_fence(pos: usize, fences: &[CodeFenceRegion]) -> bool {
    fences.iter().any(|f| pos > f.start && pos < f.end)
}

/// Find the best cut position using scored break points with distance decay.
///
/// Walks backward from `target_pos` within `window_chars`, computing a
/// decayed score for each break point: `score * (1 - (dist/window)² * decay)`.
/// This means headings far back still beat low-quality breaks near the target.
fn find_best_cutoff(
    break_points: &[BreakPoint],
    target_pos: usize,
    window_chars: usize,
    decay_factor: f32,
    code_fences: &[CodeFenceRegion],
) -> usize {
    let window_start = target_pos.saturating_sub(window_chars);
    let mut best_score: f32 = -1.0;
    let mut best_pos = target_pos;

    for bp in break_points {
        if bp.pos < window_start {
            continue;
        }
        if bp.pos > target_pos {
            break; // sorted, no more candidates
        }

        if is_inside_code_fence(bp.pos, code_fences) {
            continue;
        }

        let distance = (target_pos - bp.pos) as f32;
        let normalized = distance / window_chars as f32;
        let multiplier = 1.0 - (normalized * normalized) * decay_factor;
        let final_score = bp.score as f32 * multiplier;

        if final_score > best_score {
            best_score = final_score;
            best_pos = bp.pos;
        }
    }

    best_pos
}

/// Core chunk algorithm operating on precomputed break points and code fences.
fn chunk_with_break_points(
    content: &str,
    break_points: &[BreakPoint],
    code_fences: &[CodeFenceRegion],
    max_chars: usize,
    overlap_chars: usize,
    window_chars: usize,
) -> Vec<(String, usize)> {
    if content.len() <= max_chars {
        return vec![(content.to_string(), 0)];
    }

    let mut chunks: Vec<(String, usize)> = Vec::new();
    let mut char_pos = 0;

    while char_pos < content.len() {
        let target_end = (char_pos + max_chars).min(content.len());
        let mut end_pos = target_end;

        if end_pos < content.len() {
            let best = find_best_cutoff(break_points, target_end, window_chars, 0.7, code_fences);
            if best > char_pos && best <= target_end {
                end_pos = best;
            }
        }

        if end_pos <= char_pos {
            end_pos = target_end;
        }

        // Ensure we're on a valid UTF-8 boundary
        while end_pos > char_pos && !content.is_char_boundary(end_pos) {
            end_pos -= 1;
        }

        chunks.push((content[char_pos..end_pos].to_string(), char_pos));

        if end_pos >= content.len() {
            break;
        }

        // Advance with overlap
        let next_pos = end_pos.saturating_sub(overlap_chars);
        // Ensure forward progress
        char_pos = if next_pos > char_pos { next_pos } else { end_pos };
    }

    chunks
}

// ── Public API ──────────────────────────────────────────────────────────────

/// A markdown-aware text chunker.
///
/// Splits documents at natural structural boundaries (headings, paragraphs,
/// code blocks, etc.) rather than at arbitrary character positions. Uses
/// distance-decay scoring to pick the best available break point near the
/// target chunk size.
#[derive(Debug, Clone)]
pub struct MarkdownChunker {
    /// Max chunk size in characters.
    pub chunk_size_chars: usize,
    /// Overlap between consecutive chunks in characters.
    pub overlap_chars: usize,
    /// Search window for finding optimal break points.
    pub window_chars: usize,
}

impl MarkdownChunker {
    /// Create a new markdown chunker.
    ///
    /// Constraints: `chunk_size_chars > 0`, `overlap_chars < chunk_size_chars`.
    pub fn new(
        chunk_size_chars: usize,
        overlap_chars: usize,
        window_chars: usize,
    ) -> AgentJaxResult<Self> {
        if chunk_size_chars == 0 {
            return Err(crate::agentjax_err!("chunk_size must be > 0", Config));
        }
        if overlap_chars >= chunk_size_chars {
            return Err(crate::agentjax_err!(
                "overlap ({overlap_chars}) must be less than chunk_size ({chunk_size_chars})",
                Config
            ));
        }
        Ok(Self {
            chunk_size_chars,
            overlap_chars,
            window_chars,
        })
    }

    /// Create a chunker with production defaults:
    /// - 3600 char chunks (~900 tokens)
    /// - 540 char overlap (15%)
    /// - 800 char search window (~200 tokens)
    pub fn with_defaults() -> Self {
        Self {
            chunk_size_chars: DEFAULT_CHUNK_SIZE_CHARS,
            overlap_chars: DEFAULT_OVERLAP_CHARS,
            window_chars: DEFAULT_WINDOW_CHARS,
        }
    }

    /// Split a document into chunks.
    ///
    /// Returns a vector of `Chunk` values with IDs derived from the document
    /// ID and chunk index. Embeddings are not populated (the caller attaches
    /// them after embedding).
    pub fn chunk(&self, document: &Document) -> Vec<Chunk> {
        if document.content.is_empty() {
            return vec![];
        }

        let break_points = scan_break_points(&document.content);
        let code_fences = find_code_fences(&document.content);

        let raw_chunks = chunk_with_break_points(
            &document.content,
            &break_points,
            &code_fences,
            self.chunk_size_chars,
            self.overlap_chars,
            self.window_chars,
        );

        raw_chunks
            .into_iter()
            .enumerate()
            .map(|(i, (text, _pos))| Chunk {
                id: format!("{}_{i}", document.id),
                document_id: document.id.clone(),
                content: text,
                chunk_index: i,
                embedding: None,
            })
            .collect()
    }
}

// ── Backward-compatible alias ───────────────────────────────────────────────

/// The canonical chunker type used by `RagIndex`.
///
/// This is now the markdown-aware chunker. The old fixed-size `Chunker`
/// has been replaced.
pub type Chunker = MarkdownChunker;

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn make_doc(id: &str, content: &str) -> Document {
        Document {
            id: id.to_string(),
            content: content.to_string(),
            metadata: BTreeMap::new(),
        }
    }

    // ── Break-point detection ──────────────────────────────────────────

    #[test]
    fn test_scan_break_points_finds_headings() {
        let text = "# Intro\n\nSome text.\n## Section\nMore text.\n### Sub\nDetail.";
        let points = scan_break_points(text);
        let h1_count = points.iter().filter(|p| p.score == 100).count();
        let h2_count = points.iter().filter(|p| p.score == 90).count();
        let h3_count = points.iter().filter(|p| p.score == 80).count();
        assert!(h1_count >= 1, "expected at least one h1 break");
        assert!(h2_count >= 1, "expected at least one h2 break");
        assert!(h3_count >= 1, "expected at least one h3 break");
    }

    #[test]
    fn test_scan_break_points_sorted() {
        let text = "a\n## B\na\n# A\na\n### C";
        let points = scan_break_points(text);
        for w in points.windows(2) {
            assert!(w[0].pos <= w[1].pos, "break points must be sorted");
        }
    }

    #[test]
    fn test_scan_break_points_code_fence() {
        let text = "Before\n```\ncode here\n```\nAfter";
        let points = scan_break_points(text);
        let fence_points: Vec<_> = points.iter().filter(|p| p.score == 80).collect();
        assert_eq!(
            fence_points.len(),
            2,
            "should find both opening and closing fences"
        );
    }

    // ── Code fence detection ───────────────────────────────────────────

    #[test]
    fn test_find_code_fences_paired() {
        let text = "Before\n```\ncode\n```\nAfter";
        let fences = find_code_fences(text);
        assert_eq!(fences.len(), 1, "paired fences → one region");
        assert!(is_inside_code_fence(fences[0].start + 2, &fences));
        assert!(!is_inside_code_fence(0, &fences)); // "Before" is outside
    }

    #[test]
    fn test_find_code_fences_unclosed() {
        let text = "Before\n```\nunclosed code";
        let fences = find_code_fences(text);
        assert_eq!(fences.len(), 1, "unclosed fence → one region to end");
        assert_eq!(fences[0].end, text.len());
    }

    // ── Best cutoff ────────────────────────────────────────────────────

    #[test]
    fn test_find_best_cutoff_prefers_heading() {
        let text = "Short line\n# Heading\nLots of content here to push past the window...";
        let points = scan_break_points(text);
        let fences = find_code_fences(text);
        let best = find_best_cutoff(&points, text.len(), 800, 0.7, &fences);
        let heading_pos = text.find("\n# ").unwrap();
        assert_eq!(best, heading_pos);
    }

    #[test]
    fn test_find_best_cutoff_avoids_code_fence() {
        let text = "Before\n```\ncode inside\n```\nAfter more content";
        let points = scan_break_points(text);
        let fences = find_code_fences(text);
        let best = find_best_cutoff(&points, text.len(), 800, 0.7, &fences);
        assert!(!is_inside_code_fence(best, &fences));
    }

    // ── Full chunker ───────────────────────────────────────────────────

    #[test]
    fn test_chunker_empty_document() {
        let chunker = MarkdownChunker::new(100, 10, 50).unwrap();
        let chunks = chunker.chunk(&make_doc("empty", ""));
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_chunker_small_document() {
        let chunker = MarkdownChunker::new(3600, 540, 800).unwrap();
        let chunks = chunker.chunk(&make_doc("small", "Just a short note."));
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "Just a short note.");
    }

    #[test]
    fn test_chunker_splits_at_headings() {
        let chunker = MarkdownChunker::new(30, 5, 20).unwrap();
        let content =
            "Some initial text here.\n## Section Alpha\nMore content in alpha.\n## Section Beta\nEven more.";
        let chunks = chunker.chunk(&make_doc("doc", content));
        assert!(chunks.len() >= 2, "should split at headings");
        let has_heading_chunk = chunks
            .iter()
            .any(|c| c.content.contains("## Section"));
        assert!(has_heading_chunk);
    }

    #[test]
    fn test_chunker_code_fence_not_split() {
        let chunker = MarkdownChunker::new(100, 20, 50).unwrap();
        let content =
            "Intro text here.\n\n```\nfn test() {\n  return 42;\n}\n```\n\nOutro text.";
        let chunks = chunker.chunk(&make_doc("code", content));
        // With 100-char chunks, the entire doc should fit in one chunk
        // because the doc is ~75 chars. If somehow split, code fence
        // content must stay together.
        let has_full_fence = chunks.iter().any(|c| {
            c.content.contains("fn test()") && c.content.contains("return 42")
        });
        assert!(has_full_fence, "code fence content should not be split");
    }

    #[test]
    fn test_chunker_overlap() {
        // With a generous overlap and content that spans many chunks,
        // verify the chunker produces multiple chunks with correct indices.
        let chunker = MarkdownChunker::new(80, 20, 40).unwrap();
        let mut lines = Vec::new();
        for i in 0..20 {
            lines.push(format!("Line {i:02}: some content here for chunk testing"));
        }
        let content = lines.join("\n");
        let chunks = chunker.chunk(&make_doc("overlap", &content));
        assert!(
            chunks.len() >= 3,
            "expected at least 3 chunks, got {}",
            chunks.len()
        );
        // Verify chunk indices are sequential
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, i);
        }
    }

    #[test]
    fn test_chunker_boundary_validation() {
        assert!(MarkdownChunker::new(0, 5, 50).is_err());
        assert!(MarkdownChunker::new(10, 10, 50).is_err());
        assert!(MarkdownChunker::new(10, 11, 50).is_err());
        assert!(MarkdownChunker::new(100, 20, 50).is_ok());
    }

    #[test]
    fn test_chunker_with_defaults_works() {
        let chunker = MarkdownChunker::with_defaults();
        let content = "# Title\n\nThis is a long document with multiple sections.\n\n## Section 1\n\nContent for section one.\n\n## Section 2\n\nContent for section two.";
        let chunks = chunker.chunk(&make_doc("default", content));
        assert!(!chunks.is_empty());
        for (i, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.chunk_index, i);
            assert_eq!(chunk.document_id, "default");
        }
    }
}
