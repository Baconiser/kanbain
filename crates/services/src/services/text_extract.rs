//! Text extraction from dropped ticket attachments (Phase 3a): PDF, HTML
//! (including Jira's "Word" export, which is HTML), DOCX, XML (Jira's XML
//! export) and plain text. Images and other binaries yield nothing.

use std::io::{Cursor, Read};

use quick_xml::{Reader, events::Event};

/// Longest extracted text kept per attachment, in bytes.
pub const MAX_TEXT_BYTES: usize = 200 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextFormat {
    Pdf,
    Html,
    Docx,
    Xml,
    Text,
}

impl TextFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Html => "html",
            Self::Docx => "docx",
            Self::Xml => "xml",
            Self::Text => "text",
        }
    }
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default()
}

fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(2048)]).to_ascii_lowercase();
    head.contains("<html") || head.contains("<!doctype html") || head.contains("<body")
}

/// Which extractor applies, from the name, MIME type and leading bytes.
pub fn detect_format(name: &str, mime: Option<&str>, bytes: &[u8]) -> Option<TextFormat> {
    let ext = extension(name);
    let mime = mime.unwrap_or("").to_ascii_lowercase();
    if bytes.starts_with(b"%PDF") || ext == "pdf" || mime == "application/pdf" {
        return Some(TextFormat::Pdf);
    }
    if ext == "docx" || mime.contains("wordprocessingml") {
        return Some(TextFormat::Docx);
    }
    if mime.starts_with("image/") || mime.starts_with("video/") || mime.starts_with("audio/") {
        return None;
    }
    // Jira's "Word" export is HTML saved as .doc.
    if matches!(ext.as_str(), "html" | "htm" | "doc")
        || mime == "text/html"
        || looks_like_html(bytes)
    {
        return Some(TextFormat::Html);
    }
    if ext == "xml" || mime.ends_with("/xml") || bytes.starts_with(b"<?xml") {
        return Some(TextFormat::Xml);
    }
    let printable = std::str::from_utf8(bytes)
        .map(|s| !s.chars().any(|c| c == '\0'))
        .unwrap_or(false);
    if printable {
        return Some(TextFormat::Text);
    }
    None
}

/// Extracts the text of an attachment. `Ok(None)` means there is no text
/// to extract (an image, or an unknown binary).
pub fn extract(
    name: &str,
    mime: Option<&str>,
    bytes: &[u8],
) -> Result<Option<(TextFormat, String)>, String> {
    let Some(format) = detect_format(name, mime, bytes) else {
        return Ok(None);
    };
    let text = match format {
        TextFormat::Pdf => extract_pdf(bytes)?,
        TextFormat::Html => html_to_text(&String::from_utf8_lossy(bytes)),
        TextFormat::Docx => extract_docx(bytes)?,
        TextFormat::Xml => extract_xml(&String::from_utf8_lossy(bytes))?,
        TextFormat::Text => String::from_utf8_lossy(bytes).into_owned(),
    };
    Ok(Some((format, tidy(&text))))
}

fn extract_pdf(bytes: &[u8]) -> Result<String, String> {
    // pdf-extract panics on some malformed files.
    let bytes = bytes.to_vec();
    std::panic::catch_unwind(move || pdf_extract::extract_text_from_mem(&bytes))
        .map_err(|_| "The PDF could not be read".to_string())?
        .map_err(|e| format!("The PDF could not be read: {e}"))
}

pub fn html_to_text(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120).unwrap_or_else(|_| html.to_string())
}

fn extract_docx(bytes: &[u8]) -> Result<String, String> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| format!("Not a DOCX file: {e}"))?;
    let mut xml = String::new();
    archive
        .by_name("word/document.xml")
        .map_err(|e| format!("Not a DOCX file: {e}"))?
        .read_to_string(&mut xml)
        .map_err(|e| format!("The DOCX file could not be read: {e}"))?;

    let mut reader = Reader::from_str(&xml);
    let mut out = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == b"w:t" => in_text = true,
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"w:t" => in_text = false,
                b"w:p" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                b"w:tab" => out.push('\t'),
                b"w:br" | b"w:cr" => out.push('\n'),
                _ => {}
            },
            Ok(Event::Text(t)) if in_text => {
                out.push_str(&t.decode().map_err(|e| e.to_string())?);
            }
            Ok(Event::GeneralRef(r)) if in_text => {
                if let Some(c) = resolve_entity(&String::from_utf8_lossy(&r)) {
                    out.push(c);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("The DOCX file could not be read: {e}")),
            _ => {}
        }
    }
    Ok(out)
}

fn resolve_entity(name: &str) -> Option<char> {
    match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix('x') {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)
        }
    }
}

/// XML exports (Jira's RSS-style XML): one line per element with text.
/// Short single-line values get their element name as a label; values that
/// hold escaped HTML (Jira descriptions and comments) are converted.
fn extract_xml(xml: &str) -> Result<String, String> {
    let mut reader = Reader::from_str(xml);
    let mut stack: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut out = String::new();
    let flush = |name: Option<&String>, text: &mut String, out: &mut String| {
        let value = text.trim();
        if !value.is_empty() {
            let value = if value.contains('<') && value.contains('>') {
                html_to_text(value).trim().to_string()
            } else {
                value.to_string()
            };
            match name {
                Some(name) if value.len() <= 200 && !value.contains('\n') => {
                    out.push_str(&format!("{name}: {value}\n"));
                }
                _ => {
                    out.push_str(&value);
                    out.push_str("\n\n");
                }
            }
        }
        text.clear();
    };
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                flush(stack.last(), &mut current, &mut out);
                stack.push(String::from_utf8_lossy(e.local_name().as_ref()).into_owned());
            }
            Ok(Event::End(_)) => {
                flush(stack.last(), &mut current, &mut out);
                stack.pop();
            }
            Ok(Event::Text(t)) => current.push_str(&t.decode().map_err(|e| e.to_string())?),
            Ok(Event::CData(t)) => current.push_str(&String::from_utf8_lossy(&t)),
            Ok(Event::GeneralRef(r)) => {
                if let Some(c) = resolve_entity(&String::from_utf8_lossy(&r)) {
                    current.push(c);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(format!("The XML file could not be read: {e}")),
            _ => {}
        }
    }
    flush(stack.last(), &mut current, &mut out);
    Ok(out)
}

/// Trims trailing spaces, collapses runs of blank lines and caps the size.
fn tidy(text: &str) -> String {
    let mut out = String::with_capacity(text.len().min(MAX_TEXT_BYTES));
    let mut blank = 0;
    for line in text.replace("\r\n", "\n").lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
    }
    let mut out = out.trim().to_string();
    if out.len() > MAX_TEXT_BYTES {
        let mut cut = MAX_TEXT_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn detects_formats() {
        assert_eq!(
            detect_format("a.pdf", None, b"%PDF-1.4"),
            Some(TextFormat::Pdf)
        );
        assert_eq!(
            detect_format("export.doc", None, b"<html><body>x</body></html>"),
            Some(TextFormat::Html)
        );
        assert_eq!(
            detect_format("jira.xml", None, b"<?xml version=\"1.0\"?><rss/>"),
            Some(TextFormat::Xml)
        );
        assert_eq!(
            detect_format("notes.md", None, b"# Hello"),
            Some(TextFormat::Text)
        );
        assert_eq!(
            detect_format("shot.png", Some("image/png"), b"\x89PNG\r\n"),
            None
        );
        assert_eq!(detect_format("blob.bin", None, b"\0\x01\x02"), None);
    }

    #[test]
    fn html_and_jira_word_export() {
        let html = "<html><body><h1>PROJ-7 Login</h1><p>Users <b>cannot</b> log in.</p>\
                    <ul><li>Step one</li></ul></body></html>";
        let (format, text) = extract("PROJ-7.doc", None, html.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(format, TextFormat::Html);
        assert!(text.contains("PROJ-7 Login"));
        assert!(text.contains("cannot"));
        assert!(text.contains("Step one"));
        assert!(!text.contains("<p>"));
    }

    #[test]
    fn jira_xml_export() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="0.92"><channel><item>
  <title>[PROJ-123] Password reset mail never arrives</title>
  <key id="1">PROJ-123</key>
  <description>&lt;p&gt;Users request a reset &amp;amp; wait.&lt;/p&gt;&lt;ul&gt;&lt;li&gt;No mail&lt;/li&gt;&lt;/ul&gt;</description>
</item></channel></rss>"#;
        let (format, text) = extract("export.xml", None, xml.as_bytes())
            .unwrap()
            .unwrap();
        assert_eq!(format, TextFormat::Xml);
        assert!(text.contains("title: [PROJ-123] Password reset mail never arrives"));
        assert!(text.contains("key: PROJ-123"));
        assert!(text.contains("Users request a reset & wait."));
        assert!(text.contains("No mail"));
        assert!(!text.contains("&lt;"));
    }

    #[test]
    fn docx_paragraphs() {
        let document = r#"<?xml version="1.0"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>
<w:p><w:r><w:t>First &amp; foremost</w:t></w:r></w:p>
<w:p><w:r><w:t xml:space="preserve">Second </w:t></w:r><w:r><w:t>line</w:t></w:r></w:p>
</w:body></w:document>"#;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            zip.start_file::<_, ()>("word/document.xml", Default::default())
                .unwrap();
            zip.write_all(document.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        let (format, text) = extract("ticket.docx", None, buf.get_ref())
            .unwrap()
            .unwrap();
        assert_eq!(format, TextFormat::Docx);
        assert_eq!(text, "First & foremost\nSecond line");
    }

    #[test]
    fn pdf_text() {
        let pdf = minimal_pdf("Reset link expires PROJ-9");
        let (format, text) = extract("t.pdf", None, &pdf).unwrap().unwrap();
        assert_eq!(format, TextFormat::Pdf);
        assert!(text.contains("Reset link expires PROJ-9"), "{text}");
    }

    #[test]
    fn broken_pdf_is_an_error() {
        assert!(extract("t.pdf", None, b"%PDF-1.4 garbage").is_err());
    }

    #[test]
    fn tidy_collapses_blank_lines() {
        assert_eq!(tidy("a  \r\n\r\n\r\n\nb\n"), "a\n\nb");
    }

    /// A one-page PDF showing `text` in Helvetica.
    pub fn minimal_pdf(text: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 712 Td ({text}) Tj ET");
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
                .to_string(),
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            ),
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
                .to_string(),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }
}
