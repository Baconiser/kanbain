//! Markdown-aware text chunking (E15). Text is split at headings and
//! paragraphs into chunks of about `budget` tokens with a small overlap.
//! Every chunk starts with a header naming its ticket, so a hit is always
//! attributable.

use sha2::{Digest, Sha256};

/// Tokens of text chunks (§5).
pub const TEXT_BUDGET: usize = 400;
pub const TEXT_OVERLAP: usize = 40;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub ordinal: i64,
    /// What gets embedded, header included.
    pub text: String,
    pub token_count: i64,
    pub text_hash: String,
    /// Code chunks only.
    pub symbol: Option<String>,
    pub language: Option<String>,
    pub start_line: Option<i64>,
    pub end_line: Option<i64>,
}

/// Rough token count: four characters a token, the usual estimate for
/// English text and code.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().count().div_ceil(4)
}

pub fn hash_text(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Blocks at Markdown headings and blank lines. Code fences stay whole.
fn blocks(body: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let flush = |current: &mut Vec<&str>, blocks: &mut Vec<String>| {
        let block = current.join("\n").trim().to_string();
        if !block.is_empty() {
            blocks.push(block);
        }
        current.clear();
    };
    for line in body.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            current.push(line);
            continue;
        }
        if in_fence {
            current.push(line);
            continue;
        }
        if trimmed.starts_with('#') {
            flush(&mut current, &mut blocks);
            current.push(line);
        } else if trimmed.is_empty() {
            flush(&mut current, &mut blocks);
        } else {
            current.push(line);
        }
    }
    flush(&mut current, &mut blocks);
    // A heading belongs with the paragraph that follows it.
    let mut merged: Vec<String> = Vec::new();
    for block in blocks {
        match merged.last_mut() {
            Some(last) if last.lines().count() == 1 && last.trim_start().starts_with('#') => {
                last.push('\n');
                last.push_str(&block);
            }
            _ => merged.push(block),
        }
    }
    merged
}

/// Splits one block that is over budget into word windows.
fn split_long(block: &str, budget: usize) -> Vec<String> {
    let words: Vec<&str> = block.split_whitespace().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    for word in words {
        if !current.is_empty() && estimate_tokens(&current) + estimate_tokens(word) + 1 > budget {
            out.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// The last ~`overlap` tokens of `text`, at a word boundary.
fn tail(text: &str, overlap: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut taken: Vec<&str> = Vec::new();
    let mut tokens = 0;
    for word in words.iter().rev() {
        tokens += estimate_tokens(word) + 1;
        if tokens > overlap {
            break;
        }
        taken.push(word);
    }
    taken.reverse();
    taken.join(" ")
}

/// Chunks `body` under `header`. Empty bodies give no chunks.
pub fn chunk_text(header: &str, body: &str, budget: usize, overlap: usize) -> Vec<Chunk> {
    let header = header.trim();
    let room = budget.saturating_sub(estimate_tokens(header) + 2).max(32);
    let mut pieces: Vec<String> = Vec::new();
    for block in blocks(body) {
        if estimate_tokens(&block) > room {
            pieces.extend(split_long(&block, room));
        } else {
            pieces.push(block);
        }
    }

    let mut contents: Vec<String> = Vec::new();
    let mut current = String::new();
    for piece in pieces {
        if !current.is_empty() && estimate_tokens(&current) + estimate_tokens(&piece) + 1 > room {
            let carry = tail(&current, overlap);
            contents.push(std::mem::take(&mut current));
            if !carry.is_empty() && estimate_tokens(&carry) + estimate_tokens(&piece) < room {
                current = format!("… {carry}");
            }
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(&piece);
    }
    if !current.trim().is_empty() {
        contents.push(current);
    }

    contents
        .into_iter()
        .enumerate()
        .map(|(i, content)| {
            let text = if header.is_empty() {
                content
            } else {
                format!("{header}\n\n{content}")
            };
            Chunk {
                ordinal: i as i64,
                token_count: estimate_tokens(&text) as i64,
                text_hash: hash_text(&text),
                text,
                symbol: None,
                language: None,
                start_line: None,
                end_line: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_is_one_chunk_with_header() {
        let chunks = chunk_text(
            "KAN-1 · PROJ-1 · Login",
            "# Login\n\nUsers cannot log in.",
            TEXT_BUDGET,
            TEXT_OVERLAP,
        );
        assert_eq!(chunks.len(), 1);
        assert_eq!(
            chunks[0].text,
            "KAN-1 · PROJ-1 · Login\n\n# Login\nUsers cannot log in."
        );
        assert!(chunk_text("H", "  \n", TEXT_BUDGET, TEXT_OVERLAP).is_empty());
    }

    #[test]
    fn long_text_splits_at_headings_with_overlap() {
        let para = |word: &str| vec![word; 120].join(" ");
        let body = format!(
            "## Part one\n\n{}\n\n## Part two\n\n{}\n\n{}",
            para("alpha"),
            para("beta"),
            para("gamma")
        );
        let chunks = chunk_text("KAN-2 · Big", &body, 200, 20);
        assert!(chunks.len() >= 3, "{}", chunks.len());
        for chunk in &chunks {
            assert!(chunk.text.starts_with("KAN-2 · Big\n\n"));
            assert!(chunk.token_count <= 230, "{}", chunk.token_count);
        }
        assert!(chunks[0].text.contains("## Part one"));
        // The next chunk repeats the end of the previous one.
        assert!(chunks[1].text.contains("… alpha"));
        assert_eq!(
            chunks.iter().map(|c| c.ordinal).collect::<Vec<_>>(),
            (0..chunks.len() as i64).collect::<Vec<_>>()
        );
    }

    #[test]
    fn unchanged_chunks_keep_their_hash() {
        let one = "one ".repeat(25);
        let two = "two ".repeat(25);
        let a = chunk_text("H", &format!("## A\n\n{one}\n\n## B\n\n{two}"), 40, 0);
        let b = chunk_text(
            "H",
            &format!("## A\n\n{one}\n\n## B\n\n{two} changed"),
            40,
            0,
        );
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].text_hash, b[0].text_hash);
        assert_ne!(a[1].text_hash, b[1].text_hash);
    }

    #[test]
    fn code_fences_stay_together() {
        let chunks = chunk_text("H", "```\nfn a() {}\n\nfn b() {}\n```", 400, 0);
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].text.contains("fn a() {}\n\nfn b() {}"));
    }
}
