//! Text out of documents: PDF (text layer, OCR for scanned pages), DOCX,
//! PPTX, ODT, HTML, plain text / Markdown, images (OCR), e-books (ebook.rs:
//! EPUB, MOBI / AZW3, FB2, RTF, DjVu, CBZ, HTMLZ) and downloaded websites (web.rs).

use crate::ocr::OcrCfg;
use crate::settings::tr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Debug, Default)]
pub struct PageText {
    pub no: i64,
    pub text: String,
    pub ocr: bool,
}

#[derive(Clone, Debug)]
pub struct OutlineEntry {
    pub level: i64,
    pub title: String,
    pub page: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Extracted {
    pub pages: Vec<PageText>,
    pub outline: Vec<OutlineEntry>,
    pub title: Option<String>,
}

pub const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "tif", "tiff", "bmp", "gif", "webp"];
pub const TEXT_EXTS: &[&str] = &["txt", "md", "markdown", "text", "csv", "log", "rst", "asm", "s", "c", "h", "json", "xml", "yaml", "yml"];

/// The kind of a file by its extension (None = not supported).
pub fn kind_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_string_lossy().to_lowercase();
    let name = path.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
    if name.ends_with(".fb2.zip") {
        return Some("fb2");
    }
    Some(match ext.as_str() {
        "pdf" => "pdf",
        "docx" | "docm" | "dotx" => "docx",
        "pptx" => "pptx",
        "odt" => "odt",
        "epub" => "epub",
        "mobi" | "azw" | "azw3" | "prc" | "pdb" => "mobi",
        "fb2" | "fbz" => "fb2",
        "rtf" => "rtf",
        "djvu" | "djv" => "djvu",
        "cbz" => "cbz",
        "htmlz" => "htmlz",
        "skweb" => "web",
        "html" | "htm" | "xhtml" => "html",
        "md" | "markdown" => "md",
        e if TEXT_EXTS.contains(&e) => "txt",
        e if IMAGE_EXTS.contains(&e) => "image",
        _ => return None,
    })
}

/// What a processing job needs to know.
pub struct Opts<'a> {
    pub ocr: OcrCfg,
    /// "auto" | "force" | "off"
    pub ocr_mode: &'a str,
    pub dpi: u32,
    pub min_chars: u32,
    pub pdfium: Option<std::path::PathBuf>,
    pub cancel: &'a AtomicBool,
    /// Pages finished by an earlier, interrupted run (page no → text): not done again
    pub done: std::collections::HashMap<i64, PageText>,
    /// Called after every finished page, so an interrupted run can be resumed
    pub save: Option<&'a dyn Fn(&PageText)>,
}

impl Opts<'_> {
    pub fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(CANCELLED.to_string())
        } else {
            Ok(())
        }
    }
}

pub const CANCELLED: &str = "\u{1}cancelled";

/// progress(stage, done, total)
pub type Progress<'a> = &'a mut dyn FnMut(&str, usize, usize);

pub fn extract(path: &Path, o: &Opts, progress: Progress) -> Result<Extracted, String> {
    let kind = kind_of(path).ok_or_else(|| tr("Nem támogatott fájltípus.", "Unsupported file type."))?;
    let mut x = match kind {
        "pdf" => crate::pdf::extract(path, o, progress)?,
        "docx" => crate::docx::extract(path)?,
        "pptx" => crate::docx::extract_pptx(path)?,
        "odt" => crate::docx::extract_odt(path)?,
        "epub" => crate::ebook::extract_epub(path)?,
        "mobi" => crate::ebook::extract_mobi(path)?,
        "fb2" => crate::ebook::extract_fb2(path)?,
        "rtf" => crate::ebook::extract_rtf(path)?,
        "htmlz" => crate::ebook::extract_htmlz(path)?,
        "cbz" => crate::ebook::extract_cbz(path, o, progress)?,
        "djvu" => crate::ebook::extract_djvu(path, o, progress)?,
        "web" => crate::web::extract(path)?,
        "html" => {
            let s = read_text(path)?;
            let (md, title) = html_to_text(&s);
            let mut x = from_text(&md, true);
            x.title = title.or(x.title);
            x
        }
        "md" => from_text(&read_text(path)?, true),
        "txt" => from_text(&read_text(path)?, false),
        "image" => {
            if o.ocr_mode == "off" {
                return Err(tr("A kép csak OCR-rel olvasható, de az OCR ki van kapcsolva.", "An image can only be read with OCR, but OCR is switched off."));
            }
            progress("ocr", 0, 1);
            let text = crate::ocr::ocr_file(path, &o.ocr).map_err(|e| e.to_string())?;
            progress("ocr", 1, 1);
            Extracted { pages: vec![PageText { no: 1, text, ocr: true }], ..Default::default() }
        }
        _ => unreachable!(),
    };
    for p in &mut x.pages {
        p.text = tidy(&p.text);
    }
    if x.title.as_deref().map(str::trim).unwrap_or("").is_empty() {
        x.title = None;
    }
    Ok(x)
}

/// Reads a text file: UTF-8 (with or without BOM), UTF-16 with BOM, else Latin-2-ish fallback.
pub fn read_text(path: &Path) -> Result<String, String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    Ok(decode_text(&b))
}

pub fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(&b[3..]).into_owned();
    }
    if b.len() >= 2 && (b[0] == 0xFF && b[1] == 0xFE || b[0] == 0xFE && b[1] == 0xFF) {
        let le = b[0] == 0xFF;
        let units: Vec<u16> = b[2..].chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        return String::from_utf16_lossy(&units);
    }
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        // Windows-1250 for the Hungarian letters, Latin-1 for the rest
        Err(_) => b
            .iter()
            .map(|&c| match c {
                0xF5 => 'ő',
                0xD5 => 'Ő',
                0xFB => 'ű',
                0xDB => 'Ű',
                c => c as char,
            })
            .collect(),
    }
}

/// Normalized line ends, no trailing spaces, at most one empty line in a row.
pub fn tidy(s: &str) -> String {
    let s = s.replace("\r\n", "\n").replace('\r', "\n").replace('\u{0}', "").replace('\u{c}', "\n");
    let mut out = String::with_capacity(s.len());
    let mut blank = 0;
    for line in s.lines() {
        let l = line.trim_end();
        if l.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(l);
        out.push('\n');
    }
    out.trim_matches('\n').to_string()
}

/// Size of a pseudo page of a document without real pages.
pub const CHUNK_CHARS: usize = 3500;

/// Plain text / Markdown → pseudo pages (cut at paragraph boundaries) and,
/// for Markdown, an outline from the `#` headings.
pub fn from_text(text: &str, markdown: bool) -> Extracted {
    let text = text.replace("\r\n", "\n");
    let pages = chunk(&text, CHUNK_CHARS);
    let mut outline = vec![];
    let mut title = None;
    if markdown {
        let mut fence = false;
        for (i, p) in pages.iter().enumerate() {
            for line in p.lines() {
                let t = line.trim_start();
                if t.starts_with("```") || t.starts_with("~~~") {
                    fence = !fence;
                }
                if fence {
                    continue;
                }
                let hashes = t.chars().take_while(|&c| c == '#').count();
                if (1..=6).contains(&hashes) && t[hashes..].starts_with(' ') {
                    let h = t[hashes..].trim().trim_end_matches('#').trim().to_string();
                    if h.is_empty() {
                        continue;
                    }
                    if hashes == 1 && title.is_none() {
                        title = Some(h.clone());
                    }
                    outline.push(OutlineEntry { level: hashes as i64, title: h, page: i as i64 + 1 });
                }
            }
        }
    }
    Extracted {
        pages: pages.into_iter().enumerate().map(|(i, t)| PageText { no: i as i64 + 1, text: t, ocr: false }).collect(),
        outline,
        title,
    }
}

/// Splits a text into pieces of about `max` characters at blank lines
/// (or line ends when a paragraph is longer).
pub fn chunk(text: &str, max: usize) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut cur_len = 0;
    let mut fence = false;
    for line in text.split('\n') {
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            fence = !fence;
        }
        let len = line.chars().count() + 1;
        let para_end = line.trim().is_empty() && !fence;
        if cur_len >= max && (para_end || cur_len >= max * 2) {
            out.push(std::mem::take(&mut cur));
            cur_len = 0;
            if para_end {
                continue;
            }
        }
        cur.push_str(line);
        cur.push('\n');
        cur_len += len;
    }
    if !cur.trim().is_empty() || out.is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|s| s.trim_matches('\n').to_string()).collect()
}

/// A very small HTML → text conversion: headings become `#`, list items `-`,
/// blocks get line breaks, scripts and styles disappear, entities decoded.
pub fn html_to_text(html: &str) -> (String, Option<String>) {
    let lower = html.to_ascii_lowercase();
    let mut title = None;
    if let (Some(a), Some(b)) = (lower.find("<title"), lower.find("</title>")) {
        if let Some(gt) = lower[a..].find('>') {
            let t = decode_entities(&html[a + gt + 1..b.max(a + gt + 1)]).trim().to_string();
            if !t.is_empty() {
                title = Some(t);
            }
        }
    }
    let mut out = String::new();
    let mut i = 0;
    let bytes = html.as_bytes();
    let mut skip_until: Option<&str> = None;
    while i < html.len() {
        if bytes[i] == b'<' {
            let end = match html[i..].find('>') {
                Some(e) => i + e,
                None => break,
            };
            let tag = lower[i + 1..end].trim_start_matches('/').split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("").to_string();
            let closing = lower[i + 1..].starts_with('/');
            if let Some(stop) = skip_until {
                if closing && tag == stop {
                    skip_until = None;
                }
                i = end + 1;
                continue;
            }
            if html[i..].starts_with("<!--") {
                i = html[i..].find("-->").map(|e| i + e + 3).unwrap_or(html.len());
                continue;
            }
            match tag.as_str() {
                "script" | "style" | "head" | "noscript" | "svg" if !closing => skip_until = Some(match tag.as_str() { "script" => "script", "style" => "style", "head" => "head", "noscript" => "noscript", _ => "svg" }),
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" if !closing => {
                    let n = tag[1..].parse::<usize>().unwrap_or(1);
                    out.push_str("\n\n");
                    out.push_str(&"#".repeat(n));
                    out.push(' ');
                }
                "li" if !closing => out.push_str("\n- "),
                "br" => out.push('\n'),
                "td" | "th" if !closing => out.push_str(" | "),
                "p" | "div" | "section" | "article" | "tr" | "table" | "ul" | "ol" | "pre" | "blockquote" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "header" | "footer" | "dl" | "dt" | "dd" => out.push_str("\n\n"),
                _ => {}
            }
            i = end + 1;
        } else {
            let next = html[i..].find('<').map(|n| i + n).unwrap_or(html.len());
            if skip_until.is_none() {
                let chunk = decode_entities(&html[i..next]);
                let collapsed: String = chunk.split_whitespace().collect::<Vec<_>>().join(" ");
                if !collapsed.is_empty() {
                    if chunk.starts_with(char::is_whitespace) && !out.ends_with([' ', '\n']) {
                        out.push(' ');
                    }
                    // a text line starting with '#' (e.g. a comment in code) is not a heading
                    let line_start = out.trim_end_matches(' ').is_empty() || out.trim_end_matches(' ').ends_with('\n');
                    if line_start && collapsed.starts_with('#') {
                        out.push('\\');
                    }
                    out.push_str(&collapsed);
                    if chunk.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                }
            }
            i = next;
        }
    }
    let lines: Vec<String> = out.lines().map(|l| l.trim().to_string()).collect();
    (tidy(&lines.join("\n")), title)
}

pub fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(a) = rest.find('&') {
        out.push_str(&rest[..a]);
        rest = &rest[a..];
        // ';' is ASCII, so its byte position is a char boundary (slicing `rest` at 12 bytes
        // could cut a multi-byte character)
        let Some(semi) = rest.as_bytes().iter().take(12).position(|&b| b == b';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            "ndash" => Some('–'),
            "mdash" => Some('—'),
            "hellip" => Some('…'),
            "laquo" => Some('«'),
            "raquo" => Some('»'),
            "copy" => Some('©'),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse::<u32>().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Path::new("a/B.PDF")), Some("pdf"));
        assert_eq!(kind_of(Path::new("x.docx")), Some("docx"));
        assert_eq!(kind_of(Path::new("x.jpeg")), Some("image"));
        assert_eq!(kind_of(Path::new("x.md")), Some("md"));
        assert_eq!(kind_of(Path::new("x.exe")), None);
        assert_eq!(kind_of(Path::new("Book.EPUB")), Some("epub"));
        assert_eq!(kind_of(Path::new("b.azw3")), Some("mobi"));
        assert_eq!(kind_of(Path::new("b.fb2.zip")), Some("fb2"));
        assert_eq!(kind_of(Path::new("site.skweb")), Some("web"));
    }

    #[test]
    fn chunks_at_paragraphs() {
        let para = "word ".repeat(100); // 500 chars
        let text = (0..20).map(|_| para.clone()).collect::<Vec<_>>().join("\n\n");
        let pages = chunk(&text, 1200);
        assert!(pages.len() >= 6 && pages.len() <= 8, "{}", pages.len());
        assert!(pages.iter().all(|p| !p.starts_with('\n') && !p.trim().is_empty()));
        assert_eq!(chunk("short", 1000), vec!["short".to_string()]);
    }

    #[test]
    fn markdown_outline() {
        let x = from_text("# Title\n\nintro\n\n## Part one\n\n```\n# not a heading\n```\n\n### Deep ###\n", true);
        let t: Vec<_> = x.outline.iter().map(|o| (o.level, o.title.as_str())).collect();
        assert_eq!(t, vec![(1, "Title"), (2, "Part one"), (3, "Deep")]);
        assert_eq!(x.title.as_deref(), Some("Title"));
    }

    #[test]
    fn html_text() {
        let (t, title) = html_to_text("<html><head><title>Doc &amp; co</title><style>p{}</style></head><body><h2>Head</h2><p>One&nbsp;two <b>three</b></p><ul><li>a</li><li>b</li></ul><script>x()</script></body></html>");
        assert_eq!(title.as_deref(), Some("Doc & co"));
        assert_eq!(t, "## Head\n\nOne two three\n\n- a\n- b");
        // a '#' line of the text is escaped, a multi-byte character after '&' does not panic
        let (t, _) = html_to_text("<pre># comment</pre><p>R&amp;D &…………… x</p>");
        assert_eq!(t, "\\# comment\n\nR&D &…………… x");
    }

    #[test]
    fn text_decoding() {
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'a']), "a");
        assert_eq!(decode_text(&[0xFF, 0xFE, b'h', 0, b'i', 0]), "hi");
        assert_eq!(decode_text(&[b'f', 0xF5, b'z']), "főz");
        assert_eq!(tidy("a  \r\n\r\n\r\n\nb\r\n"), "a\n\nb");
    }
}
