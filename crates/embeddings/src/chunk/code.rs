//! Code chunking by syntax (E12–E14). Definitions come from each grammar's
//! tags query, so the chunker is mostly language-agnostic:
//! - one chunk per function, method, class/struct/impl/interface, enum and
//!   type; a container over the budget is split into its members, each with
//!   the container's signature as context;
//! - small adjacent top-level items (imports, constants, tiny functions) are
//!   merged up to the budget;
//! - files without a grammar, and code outside definitions, fall back to
//!   blank-line blocks with a line-window cap.
//!
//! Every chunk starts with a context header: `repo/path › symbol path` and
//! the language. Start and end lines are kept for navigation.

use std::{collections::HashMap, sync::OnceLock};

use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};

use super::text::{Chunk, estimate_tokens, hash_text};

/// Tokens per code chunk (§5).
pub const CODE_BUDGET: usize = 512;
/// A single function may be this many budgets long before it is split.
const LEAF_FACTOR: usize = 3;
/// Lines per fallback window.
const WINDOW_LINES: usize = 60;
/// Files above this size are not indexed (E14).
pub const MAX_FILE_BYTES: usize = 500 * 1024;
/// Bump whenever chunk boundaries or headers change. Code is indexed
/// incrementally by blob SHA, so without this an old index would keep its
/// stale chunks until every file happens to be edited.
pub const CHUNKER_VERSION: u32 = 2;

/// What a code source's stored content hash is: the blob, as chunked by this
/// version of the chunker.
pub fn content_hash(blob_sha: &str) -> String {
    format!("{blob_sha}:c{CHUNKER_VERSION}")
}

struct CodeLanguage {
    language: Language,
    tags: Query,
}

/// Language name for a path, for every language with a grammar (E13).
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "py" | "pyi" => "python",
        "go" => "go",
        "java" => "java",
        "cs" => "csharp",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        _ => return None,
    })
}

fn languages() -> &'static HashMap<&'static str, CodeLanguage> {
    static LANGUAGES: OnceLock<HashMap<&'static str, CodeLanguage>> = OnceLock::new();
    LANGUAGES.get_or_init(|| {
        let js_tags = tree_sitter_javascript::TAGS_QUERY;
        let ts_tags = format!("{}\n{}", js_tags, tree_sitter_typescript::TAGS_QUERY);
        let entries: Vec<(&str, Language, String)> = vec![
            (
                "rust",
                tree_sitter_rust::LANGUAGE.into(),
                tree_sitter_rust::TAGS_QUERY.into(),
            ),
            (
                "typescript",
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                ts_tags.clone(),
            ),
            ("tsx", tree_sitter_typescript::LANGUAGE_TSX.into(), ts_tags),
            (
                "javascript",
                tree_sitter_javascript::LANGUAGE.into(),
                js_tags.into(),
            ),
            (
                "python",
                tree_sitter_python::LANGUAGE.into(),
                tree_sitter_python::TAGS_QUERY.into(),
            ),
            (
                "go",
                tree_sitter_go::LANGUAGE.into(),
                tree_sitter_go::TAGS_QUERY.into(),
            ),
            (
                "java",
                tree_sitter_java::LANGUAGE.into(),
                tree_sitter_java::TAGS_QUERY.into(),
            ),
            (
                "csharp",
                tree_sitter_c_sharp::LANGUAGE.into(),
                tree_sitter_c_sharp::TAGS_QUERY.into(),
            ),
            (
                "c",
                tree_sitter_c::LANGUAGE.into(),
                tree_sitter_c::TAGS_QUERY.into(),
            ),
            (
                "cpp",
                tree_sitter_cpp::LANGUAGE.into(),
                tree_sitter_cpp::TAGS_QUERY.into(),
            ),
        ];
        let mut map = HashMap::new();
        for (name, language, source) in entries {
            match Query::new(&language, &source) {
                Ok(tags) => {
                    map.insert(name, CodeLanguage { language, tags });
                }
                Err(e) => tracing::warn!("The {name} tags query does not compile: {e}"),
            }
        }
        map
    })
}

/// Languages whose grammar and tags query load.
pub fn supported_languages() -> Vec<&'static str> {
    let mut names: Vec<_> = languages().keys().copied().collect();
    names.sort_unstable();
    names
}

#[derive(Debug, Clone)]
struct Def {
    /// Where the chunk starts: the doc comments and attributes above it.
    lead: usize,
    /// Where the definition itself starts (its signature line).
    start: usize,
    end: usize,
    label: String,
    children: Vec<Def>,
}

fn keyword(kind: &str, language: &str) -> &'static str {
    match kind {
        "function" | "method" => {
            if language == "rust" {
                "fn"
            } else if kind == "method" {
                "method"
            } else {
                "function"
            }
        }
        "class" => "class",
        "interface" => "interface",
        "module" => "mod",
        "macro" => "macro",
        "impl" => "impl",
        "type" => "type",
        "constant" => "const",
        "enum" => "enum",
        _ => "def",
    }
}

/// Doc comments, attributes and decorators directly above a definition
/// describe it, but grammars make them its siblings, so they'd otherwise
/// land in some gap block, away from the code they explain. Walks back over
/// them, stopping at a blank line or at a comment trailing other code.
fn leading_start(node: Node, text: &str) -> usize {
    let mut anchor = node;
    // `export function f`, `@decorator def f`: the wrapper owns the lead.
    while let Some(parent) = anchor.parent()
        && matches!(parent.kind(), "export_statement" | "decorated_definition")
    {
        anchor = parent;
    }
    let mut start = anchor.start_byte();
    let mut prev = anchor.prev_sibling();
    while let Some(node) = prev
        && is_leading(node.kind())
    {
        // Some grammars include the comment's newline; trim it so a blank
        // line still counts as one.
        let from = node.start_byte();
        let content_end = from + text[from..node.end_byte()].trim_end().len();
        let own_line = text[..from]
            .rsplit_once('\n')
            .map_or(&text[..from], |(_, line)| line)
            .trim()
            .is_empty();
        if !own_line || text[content_end..start].matches('\n').count() > 1 {
            break;
        }
        start = from;
        prev = node.prev_sibling();
    }
    start
}

fn is_leading(kind: &str) -> bool {
    matches!(
        kind,
        "comment"
            | "line_comment"
            | "block_comment"
            | "attribute_item"
            | "attribute_list"
            | "decorator"
            | "annotation"
            | "marker_annotation"
    )
}

/// Rust `impl` blocks are containers the tags query doesn't name.
fn rust_impls(node: Node, text: &str, out: &mut Vec<Def>) {
    if node.kind() == "impl_item" {
        let ty = node
            .child_by_field_name("type")
            .and_then(|n| n.utf8_text(text.as_bytes()).ok())
            .unwrap_or("?");
        let label = match node
            .child_by_field_name("trait")
            .and_then(|n| n.utf8_text(text.as_bytes()).ok())
        {
            Some(t) => format!("impl {t} for {ty}"),
            None => format!("impl {ty}"),
        };
        out.push(Def {
            lead: leading_start(node, text),
            start: node.start_byte(),
            end: node.end_byte(),
            label,
            children: vec![],
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        rust_impls(child, text, out);
    }
}

fn definitions(language: &str, lang: &CodeLanguage, text: &str) -> Option<Vec<Def>> {
    let mut parser = Parser::new();
    parser.set_language(&lang.language).ok()?;
    let tree = parser.parse(text, None)?;
    let names = lang.tags.capture_names();
    let mut flat: Vec<Def> = Vec::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&lang.tags, tree.root_node(), text.as_bytes());
    while let Some(m) = matches.next() {
        let mut def_node = None;
        let mut kind = "";
        let mut name = "";
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            if let Some(k) = capture_name.strip_prefix("definition.") {
                def_node = Some(capture.node);
                kind = k;
            } else if capture_name == "name" {
                name = capture.node.utf8_text(text.as_bytes()).unwrap_or("");
            }
        }
        if let Some(node) = def_node {
            flat.push(Def {
                lead: leading_start(node, text),
                start: node.start_byte(),
                end: node.end_byte(),
                label: format!("{} {}", keyword(kind, language), name.trim()),
                children: vec![],
            });
        }
    }
    if language == "rust" {
        rust_impls(tree.root_node(), text, &mut flat);
    }
    // Outer definitions first; the same range only once.
    flat.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    flat.dedup_by(|b, a| a.start == b.start && a.end == b.end);
    Some(nest(flat))
}

fn nest(flat: Vec<Def>) -> Vec<Def> {
    let mut roots: Vec<Def> = Vec::new();
    for def in flat {
        insert(&mut roots, def);
    }
    roots
}

fn insert(level: &mut Vec<Def>, def: Def) {
    if let Some(last) = level.last_mut()
        && def.start >= last.start
        && def.end <= last.end
    {
        insert(&mut last.children, def);
        return;
    }
    level.push(def);
}

/// Byte offset of each line start.
struct Lines {
    starts: Vec<usize>,
}

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Self { starts }
    }

    /// 1-based line of a byte offset.
    fn line(&self, offset: usize) -> usize {
        match self.starts.binary_search(&offset) {
            Ok(i) => i + 1,
            Err(i) => i,
        }
    }

    fn line_start(&self, offset: usize) -> usize {
        self.starts[self.line(offset) - 1]
    }
}

struct Builder<'a> {
    text: &'a str,
    header: String,
    language: Option<&'static str>,
    budget: usize,
    lines: Lines,
    out: Vec<Chunk>,
}

impl Builder<'_> {
    fn push(&mut self, symbol: Option<String>, context: &str, start: usize, end: usize) {
        let body = self.text[start..end].trim_end();
        if body.trim().is_empty() {
            return;
        }
        let mut header = self.header.clone();
        if let Some(symbol) = &symbol {
            header.push_str(" › ");
            header.push_str(symbol);
        }
        if let Some(language) = self.language {
            header.push_str(&format!("\n{language}"));
        }
        let text = if context.is_empty() {
            format!("{header}\n\n{body}")
        } else {
            format!("{header}\n\n{context}\n    …\n{body}")
        };
        self.out.push(Chunk {
            ordinal: self.out.len() as i64,
            token_count: estimate_tokens(&text) as i64,
            text_hash: hash_text(&text),
            text,
            symbol,
            language: self.language.map(str::to_string),
            start_line: Some(self.lines.line(start) as i64),
            end_line: Some(self.lines.line(end.saturating_sub(1).max(start)) as i64),
        });
    }

    /// Line windows of a range that is too big for one chunk.
    fn windows(&mut self, symbol: Option<String>, context: &str, start: usize, end: usize) {
        let first = self.lines.line(start);
        let last = self.lines.line(end.saturating_sub(1).max(start));
        let max_lines = WINDOW_LINES.max(1);
        let mut line = first;
        let mut part = 1;
        while line <= last {
            // Grow the window until the budget or the line cap is reached.
            let mut to = line;
            while to < last
                && to - line + 1 < max_lines
                && estimate_tokens(
                    &self.text[self.lines.starts[line - 1]..self.offset_after(to + 1)],
                ) <= self.budget
            {
                to += 1;
            }
            let from_byte = self.lines.starts[line - 1].max(start);
            let to_byte = self.offset_after(to).min(end);
            // One line over the budget: cut it by characters.
            let max_bytes = self.budget * 4;
            if to == line && to_byte - from_byte > max_bytes * 2 {
                let mut cut = from_byte;
                while cut < to_byte {
                    let mut next = (cut + max_bytes).min(to_byte);
                    while !self.text.is_char_boundary(next) {
                        next += 1;
                    }
                    let label = symbol.as_ref().map(|s| format!("{s} (part {part})"));
                    self.push(label, context, cut, next);
                    part += 1;
                    cut = next;
                }
                line = to + 1;
                continue;
            }
            let label = symbol.as_ref().map(|s| {
                if first == last && part == 1 {
                    s.clone()
                } else {
                    format!("{s} (part {part})")
                }
            });
            self.push(label, context, from_byte, to_byte);
            line = to + 1;
            part += 1;
        }
    }

    /// Byte offset just after 1-based line `line`.
    fn offset_after(&self, line: usize) -> usize {
        self.lines
            .starts
            .get(line)
            .copied()
            .unwrap_or(self.text.len())
    }

    fn emit(&mut self, def: &Def, path: &[String], context: &str) {
        let start = self.lines.line_start(def.lead);
        let end = def.end;
        let mut labels = path.to_vec();
        labels.push(def.label.clone());
        let symbol = labels.join(" › ");
        let tokens = estimate_tokens(&self.text[start..end]);
        let fits = if def.children.is_empty() {
            tokens <= self.budget * LEAF_FACTOR
        } else {
            tokens <= self.budget
        };
        if fits {
            self.push(Some(symbol), context, start, end);
        } else if def.children.is_empty() {
            self.windows(Some(symbol), context, start, end);
        } else {
            // A big container: its members, each under its signature.
            let signature_start = self.lines.line_start(def.start);
            let signature_end = self.offset_after(self.lines.line(def.start));
            let signature = self.text[signature_start..signature_end]
                .trim_end()
                .to_string();
            let context = if context.is_empty() {
                signature
            } else {
                format!("{context}\n{signature}")
            };
            for child in &def.children {
                self.emit(child, &labels, &context);
            }
        }
    }

    /// Top level: definitions and the code between them, small neighbours
    /// merged up to the budget.
    fn top_level(&mut self, defs: &[Def]) {
        enum Item<'d> {
            Def(&'d Def),
            Gap(usize, usize),
        }
        let mut items = Vec::new();
        let mut cursor = 0;
        for def in defs {
            let start = self.lines.line_start(def.lead);
            if start > cursor {
                items.extend(
                    self.gap_blocks(cursor, start)
                        .into_iter()
                        .map(|(a, b)| Item::Gap(a, b)),
                );
            }
            items.push(Item::Def(def));
            cursor = def.end.max(cursor);
        }
        if cursor < self.text.len() {
            items.extend(
                self.gap_blocks(cursor, self.text.len())
                    .into_iter()
                    .map(|(a, b)| Item::Gap(a, b)),
            );
        }

        let small = self.budget / 4;
        let mut group: Option<(usize, usize, Vec<String>)> = None;
        let flush = |builder: &mut Self, group: &mut Option<(usize, usize, Vec<String>)>| {
            if let Some((start, end, labels)) = group.take() {
                let symbol = if labels.is_empty() {
                    None
                } else {
                    Some(labels.into_iter().take(4).collect::<Vec<_>>().join(", "))
                };
                if estimate_tokens(&builder.text[start..end]) > builder.budget {
                    builder.windows(symbol, "", start, end);
                } else {
                    builder.push(symbol, "", start, end);
                }
            }
        };
        for item in items {
            let (start, end, label) = match item {
                Item::Def(def) => (
                    self.lines.line_start(def.lead),
                    def.end,
                    Some(def.label.clone()),
                ),
                Item::Gap(a, b) => (a, b, None),
            };
            let tokens = estimate_tokens(&self.text[start..end]);
            let is_small = tokens <= small;
            if is_small {
                let fits = group
                    .as_ref()
                    .is_some_and(|(s, _, _)| estimate_tokens(&self.text[*s..end]) <= self.budget);
                if !fits {
                    flush(self, &mut group);
                }
                let g = group.get_or_insert((start, end, vec![]));
                g.1 = end;
                if let Some(label) = label {
                    g.2.push(label);
                }
                continue;
            }
            flush(self, &mut group);
            match item {
                Item::Def(def) => self.emit(def, &[], ""),
                Item::Gap(a, b) => self.windows(None, "", a, b),
            }
        }
        flush(self, &mut group);
    }

    /// Non-empty blank-line-separated blocks in a range.
    fn gap_blocks(&self, start: usize, end: usize) -> Vec<(usize, usize)> {
        let mut blocks = Vec::new();
        let mut block_start: Option<usize> = None;
        let mut offset = start;
        for line in self.text[start..end].split_inclusive('\n') {
            let line_end = offset + line.len();
            if line.trim().is_empty() {
                if let Some(s) = block_start.take() {
                    blocks.push((s, offset));
                }
            } else if block_start.is_none() {
                block_start = Some(offset);
            }
            offset = line_end;
        }
        if let Some(s) = block_start {
            blocks.push((s, end));
        }
        blocks
    }
}

/// Minified or generated text: very long lines on average (E14).
pub fn looks_minified(text: &str) -> bool {
    let lines = text.lines().count().max(1);
    let longest = text.lines().map(str::len).max().unwrap_or(0);
    longest > 2000 && text.len() / lines > 300
}

/// Chunks a source file. `location` is `repo/path`. Minified files give no
/// chunks.
pub fn chunk_code(location: &str, path: &str, text: &str, budget: usize) -> Vec<Chunk> {
    if looks_minified(text) {
        return vec![];
    }
    let language = language_for_path(path);
    let mut builder = Builder {
        text,
        header: location.to_string(),
        language,
        budget,
        lines: Lines::new(text),
        out: Vec::new(),
    };
    let defs = language
        .and_then(|name| languages().get(name).map(|lang| (name, lang)))
        .and_then(|(name, lang)| definitions(name, lang, text));
    match defs {
        Some(defs) => builder.top_level(&defs),
        // No grammar: blank-line blocks, merged up to the budget.
        None => builder.top_level(&[]),
    }
    builder.out
}

/// Whether a file is worth indexing (E14): not a lockfile, vendored,
/// generated, minified or binary file, and not too big.
pub fn indexable(path: &str, size: usize, excludes: &[String]) -> bool {
    if size == 0 || size > MAX_FILE_BYTES {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    const SKIP_DIRS: &[&str] = &[
        "node_modules/",
        "target/",
        "dist/",
        "build/",
        "vendor/",
        ".git/",
        "__pycache__/",
        ".next/",
        "coverage/",
        ".sqlx/",
    ];
    if SKIP_DIRS
        .iter()
        .any(|d| lower.starts_with(d) || lower.contains(&format!("/{d}")))
    {
        return false;
    }
    const SKIP_FILES: &[&str] = &[
        "cargo.lock",
        "package-lock.json",
        "pnpm-lock.yaml",
        "yarn.lock",
        "poetry.lock",
        "composer.lock",
        "go.sum",
        "gemfile.lock",
    ];
    if SKIP_FILES.contains(&file) {
        return false;
    }
    const SKIP_SUFFIXES: &[&str] = &[
        ".min.js", ".min.css", ".map", ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".svg",
        ".pdf", ".zip", ".gz", ".tar", ".woff", ".woff2", ".ttf", ".otf", ".mp3", ".mp4", ".wav",
        ".exe", ".dll", ".so", ".dylib", ".bin", ".sqlite", ".db", ".lock", ".snap",
    ];
    if SKIP_SUFFIXES.iter().any(|s| file.ends_with(s)) {
        return false;
    }
    !excludes.iter().any(|glob| glob_matches(glob.trim(), path))
}

/// `*` matches within a path segment, `**` across segments; a pattern
/// without `/` matches the file name anywhere.
pub fn glob_matches(pattern: &str, path: &str) -> bool {
    if pattern.is_empty() {
        return false;
    }
    fn matches(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(b'*'), _) if p.get(1) == Some(&b'*') => {
                let rest = p[2..].strip_prefix(b"/").unwrap_or(&p[2..]);
                (0..=s.len()).any(|i| matches(rest, &s[i..]))
            }
            (Some(b'*'), _) => (0..=s.len())
                .take_while(|&i| i == 0 || s[i - 1] != b'/')
                .any(|i| matches(&p[1..], &s[i..])),
            (Some(b'?'), Some(c)) if *c != b'/' => matches(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => matches(&p[1..], &s[1..]),
            _ => false,
        }
    }
    if pattern.contains('/') {
        matches(pattern.as_bytes(), path.as_bytes())
    } else {
        let file = path.rsplit('/').next().unwrap_or(path);
        matches(pattern.as_bytes(), file.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUST: &str = r#"use std::fmt;

const LIMIT: usize = 3;

pub struct Issue {
    pub id: u32,
}

impl Issue {
    pub fn create(id: u32) -> Self {
        Self { id }
    }

    pub fn update(&mut self, id: u32) {
        self.id = id;
    }
}

fn helper() {}
"#;

    #[test]
    fn grammars_load() {
        assert_eq!(
            supported_languages(),
            vec![
                "c",
                "cpp",
                "csharp",
                "go",
                "java",
                "javascript",
                "python",
                "rust",
                "tsx",
                "typescript"
            ]
        );
    }

    #[test]
    fn small_rust_file_merges_items() {
        let chunks = chunk_code("app/src/issue.rs", "src/issue.rs", RUST, CODE_BUDGET);
        assert_eq!(chunks.len(), 1, "{chunks:#?}");
        let symbol = chunks[0].symbol.as_deref().unwrap();
        assert!(
            symbol.contains("impl Issue") && symbol.contains("class Issue"),
            "{symbol}"
        );
        assert_eq!(chunks[0].start_line, Some(1));
        assert!(chunks[0].text.starts_with("app/src/issue.rs › "));
    }

    #[test]
    fn big_impl_is_split_into_methods_with_signature() {
        // A small budget forces the impl apart.
        let chunks = chunk_code("app/src/issue.rs", "src/issue.rs", RUST, 32);
        let create = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("impl Issue › fn create"))
            .unwrap_or_else(|| panic!("{chunks:#?}"));
        assert_eq!((create.start_line, create.end_line), (Some(10), Some(12)));
        assert!(
            create
                .text
                .contains("impl Issue {\n    …\n    pub fn create")
        );
        assert!(create.text.contains("\nrust\n"));
        assert!(
            chunks
                .iter()
                .any(|c| c.symbol.as_deref() == Some("impl Issue › fn update"))
        );
    }

    #[test]
    fn long_function_stays_whole_up_to_three_budgets() {
        let body: String = (0..30)
            .map(|i| format!("    let x{i} = {i} * 2;\n"))
            .collect();
        let src = format!("fn long() {{\n{body}}}\n");
        let budget = estimate_tokens(&src) / 2;
        let chunks = chunk_code("r/a.rs", "a.rs", &src, budget);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].symbol.as_deref(), Some("fn long"));
        let tiny = chunk_code("r/a.rs", "a.rs", &src, budget / 3);
        assert!(tiny.len() > 1);
        assert!(
            tiny[0]
                .symbol
                .as_deref()
                .unwrap()
                .starts_with("fn long (part 1)")
        );
    }

    #[test]
    fn python_and_typescript_classes() {
        let py = "class Cart:\n    def total(self):\n        return 1\n\n    def add(self, item):\n        pass\n";
        let chunks = chunk_code("r/cart.py", "cart.py", py, 16);
        let symbols: Vec<_> = chunks.iter().filter_map(|c| c.symbol.clone()).collect();
        assert!(
            symbols.iter().any(|s| s == "class Cart › function total"),
            "{symbols:?}"
        );
        let ts = "export class Store {\n  load(): void {\n    fetch('/a');\n  }\n}\nexport function save(x: number) {\n  return x;\n}\n";
        let chunks = chunk_code("r/store.ts", "store.ts", ts, CODE_BUDGET);
        let symbol = chunks[0].symbol.clone().unwrap();
        assert!(
            symbol.contains("class Store") && symbol.contains("function save"),
            "{symbol}"
        );
    }

    #[test]
    fn doc_comments_and_attributes_stay_with_their_definition() {
        let src = "// License header, not about Issue.\n\n/// A ticket on the board.\n#[derive(Debug)]\npub struct Issue {\n    pub id: u32,\n}\n\nimpl Issue {\n    pub fn find(id: u32) -> u32 {\n        id\n    }\n\n    /// Creates a ticket and assigns its number.\n    pub fn create(id: u32) -> Self {\n        Self { id }\n    }\n}\n";
        let chunks = chunk_code("r/issue.rs", "issue.rs", src, 24);
        let find = |symbol: &str| {
            chunks
                .iter()
                .find(|c| c.symbol.as_deref() == Some(symbol))
                .unwrap_or_else(|| panic!("no {symbol}: {chunks:#?}"))
        };
        let strukt = find("class Issue");
        assert_eq!(strukt.start_line, Some(3));
        assert!(
            strukt
                .text
                .contains("/// A ticket on the board.\n#[derive(Debug)]")
        );
        // The blank line keeps the license header out.
        assert!(!strukt.text.contains("License"));
        // A split impl: the method brings its docs, the context stays the
        // `impl` line rather than some comment.
        let create = find("impl Issue › fn create");
        assert_eq!(create.start_line, Some(14));
        assert!(
            create
                .text
                .contains("impl Issue {\n    …\n    /// Creates a ticket and assigns its number.\n    pub fn create"),
            "{}",
            create.text
        );
    }

    #[test]
    fn decorators_and_jsdoc_lead_their_definition() {
        let py =
            "import os\n\n# Totals in cents.\n@cached\ndef total(cart):\n    return sum(cart)\n";
        let chunks = chunk_code("r/cart.py", "cart.py", py, 8);
        let total = chunks
            .iter()
            .find(|c| c.symbol.as_deref() == Some("function total"))
            .unwrap_or_else(|| panic!("{chunks:#?}"));
        assert_eq!(total.start_line, Some(3), "{total:#?}");
        let ts = "const a = 1; // trailing, not a doc\nexport function save(x: number) {\n  return x;\n}\n\n/** Loads the store. */\nexport function load(): number {\n  return 1;\n}\n";
        let chunks = chunk_code("r/store.ts", "store.ts", ts, 8);
        let symbol = |name: &str| {
            chunks
                .iter()
                .find(|c| c.symbol.as_deref() == Some(name))
                .unwrap_or_else(|| panic!("no {name}: {chunks:#?}"))
        };
        assert_eq!(symbol("function load").start_line, Some(6));
        // A comment trailing other code is not a doc comment.
        assert_eq!(symbol("function save").start_line, Some(2));
    }

    #[test]
    fn content_hash_names_the_chunker_version() {
        assert_eq!(content_hash("abc"), format!("abc:c{CHUNKER_VERSION}"));
        assert_ne!(content_hash("abc"), "abc");
    }

    #[test]
    fn unknown_language_falls_back_to_blocks() {
        let text = "a: 1\nb: 2\n\nc: 3\n";
        let chunks = chunk_code("r/x.yaml", "x.yaml", text, CODE_BUDGET);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].symbol, None);
        assert_eq!(chunks[0].language, None);
        assert_eq!(
            (chunks[0].start_line, chunks[0].end_line),
            (Some(1), Some(4))
        );
    }

    #[test]
    fn minified_and_long_lines() {
        let minified = format!("{}\n", "a=1;".repeat(1000));
        assert!(chunk_code("r/x.js", "x.js", &minified, CODE_BUDGET).is_empty());
        let long_line = format!("fn a() {{}}\n\n// {}\n", "word ".repeat(2000));
        let chunks = chunk_code("r/x.rs", "x.rs", &long_line, CODE_BUDGET);
        assert!(
            chunks
                .iter()
                .all(|c| c.token_count < (CODE_BUDGET * 2) as i64),
            "{}",
            chunks.len()
        );
    }

    #[test]
    fn what_is_indexed() {
        assert!(indexable("src/main.rs", 100, &[]));
        assert!(!indexable("Cargo.lock", 100, &[]));
        assert!(!indexable("web/node_modules/x/index.js", 100, &[]));
        assert!(!indexable("app.min.js", 100, &[]));
        assert!(!indexable("big.rs", MAX_FILE_BYTES + 1, &[]));
        let generated = vec!["shared/types.ts".to_string(), "*.generated.cs".to_string()];
        assert!(!indexable("shared/types.ts", 100, &generated));
        assert!(!indexable("src/Api.generated.cs", 100, &generated));
        assert!(indexable("src/types.ts", 100, &generated));
        assert!(glob_matches(
            "crates/**/tests/*.rs",
            "crates/db/src/tests/a.rs"
        ));
        assert!(!glob_matches("crates/*/a.rs", "crates/db/src/a.rs"));
    }
}

#[cfg(test)]
mod measure {
    use super::*;

    /// `cargo test -p embeddings measure_this_repo -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn measure_this_repo() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let files = std::process::Command::new("git")
            .args(["ls-files"])
            .current_dir(&root)
            .output()
            .unwrap();
        let started = std::time::Instant::now();
        let (mut indexed, mut chunks, mut tokens) = (0, 0, 0);
        let mut by_language: HashMap<String, usize> = HashMap::new();
        let mut biggest = 0;
        for path in String::from_utf8_lossy(&files.stdout).lines() {
            let Ok(text) = std::fs::read_to_string(root.join(path)) else {
                continue;
            };
            if !indexable(path, text.len(), &["shared/types.ts".to_string()]) {
                continue;
            }
            indexed += 1;
            for chunk in chunk_code(&format!("kanbain/{path}"), path, &text, CODE_BUDGET) {
                chunks += 1;
                tokens += chunk.token_count as usize;
                biggest = biggest.max(chunk.token_count as usize);
                *by_language
                    .entry(chunk.language.unwrap_or_else(|| "other".into()))
                    .or_default() += 1;
            }
        }
        println!(
            "{indexed} files, {chunks} chunks, ~{tokens} tokens (largest chunk {biggest}), chunked in {:?}",
            started.elapsed()
        );
        let mut langs: Vec<_> = by_language.into_iter().collect();
        langs.sort_by(|a, b| b.1.cmp(&a.1));
        println!("{langs:?}");
    }
}
