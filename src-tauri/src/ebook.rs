//! E-books and other book formats: EPUB (2 / 3), Kindle / Mobipocket (MOBI, AZW,
//! AZW3, PRC, PDB), FictionBook (FB2, FBZ, FB2.ZIP), RTF, HTMLZ, comic books
//! (CBZ, OCR) and DjVu (text layer or OCR through DjVuLibre).
//!
//! Every format becomes Markdown-like text cut into pseudo pages (about
//! CHUNK_CHARS each, chapters always start a new page) with an outline.

use crate::extract::{decode_text, from_text, html_to_text, Extracted, Opts, OutlineEntry, PageText, Progress, IMAGE_EXTS};
use crate::settings::tr;
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Largest single entry read from an e-book archive.
const MAX_ENTRY: u64 = 64 * 1024 * 1024;

// ───────────────────────── shared ─────────────────────────

/// Chapters (title, Markdown text) → pages: each chapter starts on a new page,
/// its title (or its headings when no title is known) goes into the outline.
pub(crate) fn from_chapters(chapters: Vec<(Option<String>, String)>, toc: &[(i64, String, usize)], title: Option<String>) -> Extracted {
    let mut pages: Vec<PageText> = vec![];
    let mut outline = vec![];
    let mut first_page_of = vec![];
    for (ctitle, text) in &chapters {
        let text = text.trim();
        first_page_of.push(pages.len() as i64 + 1);
        if text.is_empty() {
            continue;
        }
        let start = pages.len() as i64;
        let x = from_text(text, true);
        if toc.is_empty() {
            match ctitle {
                Some(t) if !t.trim().is_empty() => outline.push(OutlineEntry { level: 1, title: t.trim().to_string(), page: start + 1 }),
                _ => outline.extend(x.outline.iter().map(|o| OutlineEntry { level: o.level, title: o.title.clone(), page: o.page + start })),
            }
        }
        for p in x.pages {
            pages.push(PageText { no: pages.len() as i64 + 1, text: p.text, ocr: false });
        }
    }
    for (level, t, ch) in toc {
        if let Some(&page) = first_page_of.get(*ch) {
            let page = page.min(pages.len().max(1) as i64);
            outline.push(OutlineEntry { level: *level, title: t.clone(), page });
        }
    }
    if pages.is_empty() {
        pages.push(PageText { no: 1, text: String::new(), ocr: false });
    }
    let title = title.filter(|t| !t.trim().is_empty()).or_else(|| outline.first().map(|o| o.title.clone()));
    Extracted { pages, outline, title }
}

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", tr("Hibás vagy nem ZIP alapú fájl", "Damaged or not a ZIP based file")))
}

/// An entry by name (exact, then case-insensitive), as bytes.
fn zip_bytes(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<Vec<u8>> {
    let real = if z.index_for_name(name).is_some() {
        name.to_string()
    } else {
        let low = name.to_lowercase();
        z.file_names().find(|n| n.to_lowercase() == low)?.to_string()
    };
    let f = z.by_name(&real).ok()?;
    let mut buf = vec![];
    f.take(MAX_ENTRY).read_to_end(&mut buf).ok()?;
    Some(buf)
}

fn zip_text(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    zip_bytes(z, name).map(|b| decode_xml_bytes(&b))
}

/// Bytes of an XML / HTML file → text, honouring a declared windows-1251 / koi8-r
/// (common in FictionBook files); the rest as in decode_text.
pub fn decode_xml_bytes(b: &[u8]) -> String {
    let head = String::from_utf8_lossy(&b[..b.len().min(300)]).to_ascii_lowercase();
    let enc = head
        .find("encoding=")
        .or_else(|| head.find("charset="))
        .map(|i| head[i..].split(['"', '\'', '=']).filter(|s| !s.trim().is_empty()).nth(1).unwrap_or("").trim().to_string())
        .unwrap_or_default();
    if enc.contains("1251") || enc == "cp1251" {
        return b.iter().map(|&c| cp1251(c)).collect();
    }
    decode_text(b)
}

pub(crate) fn cp1251(c: u8) -> char {
    const HI: [u16; 64] = [
        0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F, //
        0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0xFFFD, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, //
        0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407, //
        0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    ];
    match c {
        0..=0x7F => c as char,
        0x80..=0xBF => char::from_u32(HI[(c - 0x80) as usize] as u32).unwrap_or('\u{FFFD}'),
        _ => char::from_u32(0x0410 + (c - 0xC0) as u32).unwrap_or('\u{FFFD}'),
    }
}

/// `a/b/../c.html#x` → `a/c.html` (zip paths, `%20` decoded).
fn norm_zip_path(base: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or("");
    let href = percent_decode(href);
    let joined = if href.starts_with('/') { href.trim_start_matches('/').to_string() } else if base.is_empty() { href } else { format!("{base}/{href}") };
    let mut parts: Vec<&str> = vec![];
    for p in joined.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_xml(xml: &str) -> Result<Document<'_>, String> {
    let opt = roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() };
    Document::parse_with_options(xml, opt).map_err(|e| format!("XML: {e}"))
}

fn elems<'a, 'i>(n: Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

fn first_desc<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.descendants().find(|d| d.is_element() && d.tag_name().name() == name)
}

fn all_text(n: Node) -> String {
    n.descendants().filter(|d| d.is_text()).filter_map(|d| d.text()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

// ───────────────────────── EPUB ─────────────────────────

pub fn extract_epub(path: &Path) -> Result<Extracted, String> {
    let mut z = open_zip(path)?;
    let not_epub = || tr("Nem EPUB könyv (hiányzik a META-INF/container.xml).", "Not an EPUB book (META-INF/container.xml is missing).");
    let container = zip_text(&mut z, "META-INF/container.xml").ok_or_else(not_epub)?;
    let cdoc = parse_xml(&container)?;
    let opf_path = cdoc
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .ok_or_else(not_epub)?
        .to_string();
    let base = opf_path.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default();
    let opf = zip_text(&mut z, &opf_path).ok_or_else(not_epub)?;
    let odoc = parse_xml(&opf)?;
    let title = odoc.descendants().find(|n| n.is_element() && n.tag_name().name() == "title" && n.tag_name().namespace().is_some_and(|ns| ns.contains("purl.org/dc"))).map(all_text);

    // manifest: id → (path, media type, properties)
    let mut manifest: HashMap<String, (String, String, String)> = HashMap::new();
    for it in odoc.descendants().filter(|n| n.is_element() && n.tag_name().name() == "item") {
        if let (Some(id), Some(href)) = (it.attribute("id"), it.attribute("href")) {
            manifest.insert(id.to_string(), (norm_zip_path(&base, href), it.attribute("media-type").unwrap_or("").to_string(), it.attribute("properties").unwrap_or("").to_string()));
        }
    }
    let spine_node = first_desc(odoc.root_element(), "spine");
    let mut spine: Vec<String> = vec![];
    if let Some(sp) = spine_node {
        for r in elems(sp).filter(|n| n.tag_name().name() == "itemref") {
            if let Some((p, mt, _)) = r.attribute("idref").and_then(|id| manifest.get(id)) {
                if mt.contains("html") || mt.is_empty() || p.ends_with("htm") || p.ends_with("html") {
                    spine.push(p.clone());
                }
            }
        }
    }
    if spine.is_empty() {
        return Err(tr("Az EPUB nem tartalmaz olvasható fejezetet.", "The EPUB has no readable chapter."));
    }

    // DRM: encrypted chapters (font obfuscation is fine)
    if let Some(enc) = zip_text(&mut z, "META-INF/encryption.xml") {
        if let Ok(ed) = parse_xml(&enc) {
            let font_alg = |a: &str| a.contains("idpf.org/2008/embedding") || a.contains("ns.adobe.com/pdf/enc#RC");
            for data in ed.descendants().filter(|n| n.is_element() && n.tag_name().name() == "EncryptedData") {
                let alg = first_desc(data, "EncryptionMethod").and_then(|m| m.attribute("Algorithm")).unwrap_or("");
                let uri = first_desc(data, "CipherReference").and_then(|c| c.attribute("URI")).unwrap_or("");
                let uri = norm_zip_path("", uri);
                if !font_alg(alg) && spine.iter().any(|s| *s == uri) {
                    return Err(tr("Az EPUB másolásvédett (DRM), a szövege nem olvasható ki.", "The EPUB is copy protected (DRM); its text cannot be read."));
                }
            }
        }
    }

    // table of contents: EPUB 3 nav document, else the NCX
    let chapter_of = |file: &str| spine.iter().position(|s| s == file);
    let mut toc: Vec<(i64, String, usize)> = vec![];
    let nav = manifest.values().find(|(_, _, props)| props.split_whitespace().any(|p| p == "nav")).map(|(p, _, _)| p.clone());
    if let Some(nav_path) = nav {
        if let Some(x) = zip_text(&mut z, &nav_path) {
            if let Ok(nd) = parse_xml(&x) {
                let nav_base = nav_path.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default();
                let toc_nav = nd
                    .descendants()
                    .find(|n| n.is_element() && n.tag_name().name() == "nav" && n.attributes().any(|a| a.name() == "type" && a.value() == "toc"))
                    .or_else(|| first_desc(nd.root_element(), "nav"));
                if let Some(tn) = toc_nav {
                    for a in tn.descendants().filter(|n| n.is_element() && n.tag_name().name() == "a") {
                        let depth = a.ancestors().filter(|n| n.is_element() && n.tag_name().name() == "ol").count().max(1) as i64;
                        let t = all_text(a);
                        if let (Some(href), false) = (a.attribute("href"), t.is_empty()) {
                            if let Some(ch) = chapter_of(&norm_zip_path(&nav_base, href)) {
                                toc.push((depth.min(6), t, ch));
                            }
                        }
                    }
                }
            }
        }
    }
    if toc.is_empty() {
        let ncx = spine_node
            .and_then(|s| s.attribute("toc"))
            .and_then(|id| manifest.get(id))
            .map(|m| m.0.clone())
            .or_else(|| manifest.values().find(|(_, mt, _)| mt.contains("dtbncx")).map(|m| m.0.clone()));
        if let Some(ncx_path) = ncx {
            if let Some(x) = zip_text(&mut z, &ncx_path) {
                if let Ok(nd) = parse_xml(&x) {
                    let ncx_base = ncx_path.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default();
                    for np in nd.descendants().filter(|n| n.is_element() && n.tag_name().name() == "navPoint") {
                        let depth = np.ancestors().filter(|n| n.is_element() && n.tag_name().name() == "navPoint").count() as i64;
                        let label = elems(np).find(|n| n.tag_name().name() == "navLabel").map(all_text).unwrap_or_default();
                        let src = elems(np).find(|n| n.tag_name().name() == "content").and_then(|c| c.attribute("src"));
                        if let (Some(src), false) = (src, label.is_empty()) {
                            if let Some(ch) = chapter_of(&norm_zip_path(&ncx_base, src)) {
                                toc.push((depth.clamp(1, 6), label, ch));
                            }
                        }
                    }
                }
            }
        }
    }

    let mut chapters = vec![];
    for file in &spine {
        let html = zip_bytes(&mut z, file).map(|b| decode_xml_bytes(&b)).unwrap_or_default();
        let (text, ctitle) = html_to_text(&html);
        chapters.push((ctitle.filter(|t| title.as_deref() != Some(t.as_str())), text));
    }
    // a TOC that points to one file only (e.g. everything in one chapter) is still right;
    // without any TOC entry the chapter titles / headings are used
    Ok(from_chapters(chapters, &toc, title))
}

// ───────────────────────── HTMLZ (calibre) ─────────────────────────

pub fn extract_htmlz(path: &Path) -> Result<Extracted, String> {
    let mut z = open_zip(path)?;
    let name = z
        .file_names()
        .filter(|n| n.to_lowercase().ends_with(".html") || n.to_lowercase().ends_with(".htm"))
        .min_by_key(|n| (!n.to_lowercase().ends_with("index.html"), n.len()))
        .map(str::to_string)
        .ok_or_else(|| tr("Az archívumban nincs HTML fájl.", "There is no HTML file in the archive."))?;
    let html = zip_text(&mut z, &name).unwrap_or_default();
    let (text, title) = html_to_text(&html);
    let mut x = from_text(&text, true);
    x.title = title.or(x.title);
    Ok(x)
}

// ───────────────────────── FictionBook ─────────────────────────

pub fn extract_fb2(path: &Path) -> Result<Extracted, String> {
    let raw = std::fs::read(path).map_err(|e| e.to_string())?;
    let bytes = if raw.starts_with(b"PK") {
        let mut z = open_zip(path)?;
        let name = z
            .file_names()
            .find(|n| n.to_lowercase().ends_with(".fb2"))
            .map(str::to_string)
            .ok_or_else(|| tr("Az archívumban nincs .fb2 fájl.", "There is no .fb2 file in the archive."))?;
        zip_bytes(&mut z, &name).unwrap_or_default()
    } else {
        raw
    };
    let mut xml = decode_xml_bytes(&bytes);
    // the declaration may name an encoding roxmltree does not know: the text is already decoded
    if xml.trim_start().starts_with("<?xml") {
        if let Some(e) = xml.find("?>") {
            xml = xml[e + 2..].to_string();
        }
    }
    let doc = parse_xml(&xml)?;
    let root = doc.root_element();
    let title = first_desc(root, "title-info").and_then(|ti| elems(ti).find(|n| n.tag_name().name() == "book-title")).map(all_text);
    let mut chapters: Vec<(Option<String>, String)> = vec![];
    for body in elems(root).filter(|n| n.tag_name().name() == "body") {
        let notes = body.attribute("name").is_some_and(|n| n.contains("note"));
        let mut md = String::new();
        if notes {
            md.push_str(&format!("# {}\n\n", tr("Jegyzetek", "Notes")));
        }
        fb2_block(body, 0, &mut md);
        // every top-level section a chapter of its own
        if notes {
            chapters.push((None, md));
        } else {
            for part in split_top_headings(&md) {
                chapters.push((None, part));
            }
        }
    }
    Ok(from_chapters(chapters, &[], title))
}

/// Cuts Markdown before every level-1 heading.
fn split_top_headings(md: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    for line in md.lines() {
        if line.starts_with("# ") && !out.last().unwrap().trim().is_empty() {
            out.push(String::new());
        }
        let last = out.last_mut().unwrap();
        last.push_str(line);
        last.push('\n');
    }
    out.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

fn fb2_inline(n: Node) -> String {
    let mut s = String::new();
    for c in n.children() {
        if c.is_text() {
            s.push_str(&c.text().unwrap_or("").replace('\n', " "));
        } else if c.is_element() {
            let inner = fb2_inline(c);
            let t = inner.trim();
            match c.tag_name().name() {
                "emphasis" if !t.is_empty() => s.push_str(&format!("*{t}*")),
                "strong" if !t.is_empty() => s.push_str(&format!("**{t}**")),
                "strikethrough" if !t.is_empty() => s.push_str(&format!("~~{t}~~")),
                "code" if !t.is_empty() => s.push_str(&format!("`{t}`")),
                "image" => {}
                _ => s.push_str(&inner),
            }
        }
    }
    s
}

fn fb2_block(n: Node, depth: usize, md: &mut String) {
    for c in elems(n) {
        match c.tag_name().name() {
            "section" => fb2_block(c, depth + 1, md),
            "title" => {
                let t: Vec<String> = elems(c).map(|p| fb2_inline(p).trim().to_string()).filter(|s| !s.is_empty()).collect();
                let t = if t.is_empty() { fb2_inline(c).trim().to_string() } else { t.join(" – ") };
                if !t.is_empty() {
                    md.push_str(&format!("{} {}\n\n", "#".repeat(depth.clamp(1, 6)), t));
                }
            }
            "p" => {
                let t = fb2_inline(c);
                if !t.trim().is_empty() {
                    md.push_str(t.trim());
                    md.push_str("\n\n");
                }
            }
            "subtitle" => md.push_str(&format!("**{}**\n\n", fb2_inline(c).trim())),
            "empty-line" => md.push('\n'),
            "poem" | "stanza" => {
                fb2_block(c, depth, md);
                md.push('\n');
            }
            "v" => {
                md.push_str(fb2_inline(c).trim());
                md.push_str("  \n");
            }
            "epigraph" | "cite" | "annotation" => {
                let mut inner = String::new();
                fb2_block(c, depth, &mut inner);
                for l in inner.trim().lines() {
                    md.push_str("> ");
                    md.push_str(l);
                    md.push('\n');
                }
                md.push('\n');
            }
            "text-author" => md.push_str(&format!("— {}\n\n", fb2_inline(c).trim())),
            "table" => {
                for (i, tr_) in elems(c).filter(|r| r.tag_name().name() == "tr").enumerate() {
                    let cells: Vec<String> = elems(tr_).map(|td| fb2_inline(td).trim().replace('|', "\\|")).collect();
                    md.push_str(&format!("| {} |\n", cells.join(" | ")));
                    if i == 0 {
                        md.push_str(&format!("|{}\n", " --- |".repeat(cells.len().max(1))));
                    }
                }
                md.push('\n');
            }
            "image" | "binary" => {}
            _ => fb2_block(c, depth, md),
        }
    }
}

// ───────────────────────── Mobipocket / Kindle ─────────────────────────

fn be16(b: &[u8], at: usize) -> Option<u16> {
    b.get(at..at + 2).map(|x| u16::from_be_bytes([x[0], x[1]]))
}
fn be32(b: &[u8], at: usize) -> Option<u32> {
    b.get(at..at + 4).map(|x| u32::from_be_bytes([x[0], x[1], x[2], x[3]]))
}

/// PalmDOC LZ77 decompression.
pub fn palmdoc_decompress(src: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(src.len() * 2);
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        i += 1;
        match c {
            1..=8 => {
                let end = (i + c as usize).min(src.len());
                out.extend_from_slice(&src[i..end]);
                i = end;
            }
            0x80..=0xBF => {
                if i >= src.len() {
                    break;
                }
                let pair = ((c as usize) << 8) | src[i] as usize;
                i += 1;
                let dist = (pair >> 3) & 0x7FF;
                let len = (pair & 7) + 3;
                if dist == 0 || dist > out.len() {
                    continue;
                }
                let start = out.len() - dist;
                for k in 0..len {
                    out.push(out[start + k]);
                }
            }
            0xC0..=0xFF => {
                out.push(b' ');
                out.push(c ^ 0x80);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Size of the trailing entries of a text record (MOBI "extra data flags").
fn trailing_size(rec: &[u8], flags: u16) -> usize {
    let mut size = 0usize;
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            // backward variable-width integer at the end of what is left
            let mut end = rec.len().saturating_sub(size);
            let mut val = 0usize;
            let mut shift = 0;
            while end > 0 {
                let v = rec[end - 1];
                val |= ((v & 0x7F) as usize) << shift;
                shift += 7;
                end -= 1;
                if v & 0x80 != 0 || shift >= 28 {
                    break;
                }
            }
            size += val;
        }
        f >>= 1;
    }
    if flags & 1 != 0 {
        if let Some(&b) = rec.len().checked_sub(size + 1).and_then(|i| rec.get(i)) {
            size += (b & 3) as usize + 1;
        }
    }
    size.min(rec.len())
}

pub fn extract_mobi(path: &Path) -> Result<Extracted, String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    let bad = || tr("Nem Mobipocket / Kindle könyv, vagy sérült.", "Not a Mobipocket / Kindle book, or damaged.");
    let typ = b.get(60..68).ok_or_else(bad)?;
    let n = be16(&b, 76).ok_or_else(bad)? as usize;
    let offs: Vec<usize> = (0..n).map(|i| be32(&b, 78 + i * 8).map(|o| o as usize)).collect::<Option<_>>().ok_or_else(bad)?;
    let rec = |i: usize| -> &[u8] {
        let a = offs.get(i).copied().unwrap_or(b.len()).min(b.len());
        let e = offs.get(i + 1).copied().unwrap_or(b.len()).min(b.len()).max(a);
        &b[a..e]
    };
    let r0 = rec(0);
    let compression = be16(r0, 0).ok_or_else(bad)?;
    let text_len = be32(r0, 4).unwrap_or(0) as usize;
    let count = be16(r0, 8).unwrap_or(0) as usize;
    let encryption = be16(r0, 12).unwrap_or(0);
    if encryption != 0 {
        return Err(tr("A könyv másolásvédett (DRM), a szövege nem olvasható ki.", "The book is copy protected (DRM); its text cannot be read."));
    }
    if compression == 17480 {
        return Err(tr("Ez a Kindle-könyv HUFF/CDIC tömörítést használ, amit a SkillStar nem ismer. Alakítsd át EPUB-ra (pl. calibre).", "This Kindle book uses HUFF/CDIC compression, which SkillStar does not read. Convert it to EPUB (e.g. with calibre)."));
    }
    let mobi = r0.get(16..20) == Some(b"MOBI");
    let (utf8, flags, title) = if mobi {
        let hlen = be32(r0, 20).unwrap_or(0) as usize;
        let enc = be32(r0, 28).unwrap_or(1252);
        let flags = if hlen >= 0xE4 { be16(r0, 242).unwrap_or(0) } else { 0 };
        let no = be32(r0, 84).unwrap_or(0) as usize;
        let nl = be32(r0, 88).unwrap_or(0) as usize;
        let title = r0.get(no..no + nl).map(|t| if enc == 65001 { String::from_utf8_lossy(t).into_owned() } else { decode_text(t) });
        (enc == 65001, flags, title.map(|t| t.replace('\u{a0}', " ").trim().to_string()))
    } else {
        let name = b.get(0..32).map(|t| String::from_utf8_lossy(t.split(|&c| c == 0).next().unwrap_or(&[])).into_owned());
        (false, 0, name)
    };
    let mut raw: Vec<u8> = Vec::with_capacity(text_len.min(64 << 20));
    for i in 1..=count.min(n.saturating_sub(1)) {
        let r = rec(i);
        let r = &r[..r.len() - trailing_size(r, flags)];
        match compression {
            2 => raw.extend(palmdoc_decompress(r)),
            _ => raw.extend_from_slice(r),
        }
    }
    if text_len > 0 && raw.len() > text_len {
        raw.truncate(text_len);
    }
    let text = if utf8 { String::from_utf8_lossy(&raw).into_owned() } else { decode_text(&raw) };
    let is_html = mobi || typ.starts_with(b"BOOKMOBI") || text.contains("<html") || text.contains("<p");
    if is_html {
        // Mobipocket page breaks start a new section
        let parts: Vec<(Option<String>, String)> = text
            .split("<mbp:pagebreak")
            .enumerate()
            // every part after a break starts with the rest of the break tag ("/>")
            .map(|(k, p)| html_to_text(if k > 0 { p.find('>').map(|i| &p[i + 1..]).unwrap_or(p) } else { p }).0)
            .filter(|t| !t.trim().is_empty())
            .map(|t| (None, t))
            .collect();
        Ok(from_chapters(parts, &[], title))
    } else {
        let mut x = from_text(&text, false);
        x.title = title;
        Ok(x)
    }
}

// ───────────────────────── RTF ─────────────────────────

pub fn rtf_to_text(src: &str) -> String {
    const SKIP: &[&str] = &[
        "fonttbl", "colortbl", "stylesheet", "info", "pict", "header", "footer", "headerl", "headerr", "headerf", "footerl", "footerr", "footerf", "xmlnstbl",
        "listtable", "listoverridetable", "rsidtbl", "generator", "themedata", "colorschememapping", "datastore", "latentstyles", "fldinst", "object",
        "objdata", "mmathPr", "filetbl", "revtbl", "pgdsctbl", "bkmkstart", "bkmkend", "nonshppict", "shppict", "wgrffmtfilter", "listtext", "pntext",
    ];
    #[derive(Clone, Copy)]
    struct St {
        skip: bool,
        uc: usize,
    }
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len() / 2);
    let mut stack: Vec<St> = vec![];
    let mut st = St { skip: false, uc: 1 };
    let mut pending_skip = 0usize; // chars to drop after \uN
    let mut cp = 1252u32;
    let mut i = 0;
    let mut group_start = false;
    let push_byte = |out: &mut String, v: u8, cp: u32| {
        let c = if cp == 1251 { cp1251(v) } else { decode_text(&[v]).chars().next().unwrap_or('?') };
        out.push(c);
    };
    while i < b.len() {
        let c = b[i];
        match c {
            b'{' => {
                stack.push(st);
                group_start = true;
                i += 1;
                continue;
            }
            b'}' => {
                st = stack.pop().unwrap_or(St { skip: false, uc: 1 });
                group_start = false;
                i += 1;
                continue;
            }
            b'\\' => {
                i += 1;
                let Some(&n) = b.get(i) else { break };
                if n.is_ascii_alphabetic() {
                    let s = i;
                    while i < b.len() && b[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    let word = &src[s..i];
                    let ns = i;
                    if i < b.len() && (b[i] == b'-' || b[i].is_ascii_digit()) {
                        i += 1;
                        while i < b.len() && b[i].is_ascii_digit() {
                            i += 1;
                        }
                    }
                    let param: Option<i32> = src[ns..i].parse().ok();
                    if i < b.len() && b[i] == b' ' {
                        i += 1;
                    }
                    if group_start && SKIP.contains(&word) {
                        st.skip = true;
                    }
                    group_start = false;
                    if st.skip {
                        continue;
                    }
                    match word {
                        "par" | "line" | "row" => out.push('\n'),
                        "sect" | "page" => out.push_str("\n\n"),
                        "tab" | "cell" => out.push('\t'),
                        "emdash" => out.push('—'),
                        "endash" => out.push('–'),
                        "bullet" => out.push('•'),
                        "lquote" => out.push('‘'),
                        "rquote" => out.push('’'),
                        "ldblquote" => out.push('“'),
                        "rdblquote" => out.push('”'),
                        "ansicpg" => cp = param.unwrap_or(1252) as u32,
                        "uc" => st.uc = param.unwrap_or(1).max(0) as usize,
                        "u" => {
                            let v = param.unwrap_or(0);
                            let v = if v < 0 { (v + 65536) as u32 } else { v as u32 };
                            out.push(char::from_u32(v).unwrap_or('\u{FFFD}'));
                            pending_skip = st.uc;
                        }
                        _ => {}
                    }
                    continue;
                }
                // control symbols
                group_start = if n == b'*' { group_start } else { false };
                i += 1;
                match n {
                    b'*' => st.skip = true,
                    b'\'' => {
                        if let Some(v) = src.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                            i += 2;
                            if pending_skip > 0 {
                                pending_skip -= 1;
                            } else if !st.skip {
                                push_byte(&mut out, v, cp);
                            }
                        }
                    }
                    b'~' if !st.skip => out.push('\u{a0}'),
                    b'_' if !st.skip => out.push('-'),
                    b'\\' | b'{' | b'}' if !st.skip => out.push(n as char),
                    b'\n' | b'\r' if !st.skip => out.push('\n'),
                    _ => {}
                }
                continue;
            }
            b'\r' | b'\n' => {
                i += 1;
                continue;
            }
            _ => {}
        }
        group_start = false;
        // a plain character (UTF-8 in the source is kept as is)
        let ch_len = src[i..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
        if pending_skip > 0 {
            pending_skip -= 1;
        } else if !st.skip {
            out.push_str(&src[i..i + ch_len]);
        }
        i += ch_len;
    }
    out.replace('\u{a0}', " ")
}

pub fn extract_rtf(path: &Path) -> Result<Extracted, String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    if !b.starts_with(b"{\\rtf") {
        return Err(tr("Nem RTF dokumentum.", "Not an RTF document."));
    }
    // RTF is 7-bit: a lossy read keeps every control word
    let src = String::from_utf8_lossy(&b);
    Ok(from_text(&rtf_to_text(&src), false))
}

// ───────────────────────── comic books (CBZ) ─────────────────────────

fn natural_key(s: &str) -> Vec<(u8, u64, String)> {
    let mut out = vec![];
    let mut num = String::new();
    let mut txt = String::new();
    for c in s.to_lowercase().chars() {
        if c.is_ascii_digit() {
            if !txt.is_empty() {
                out.push((1, 0, std::mem::take(&mut txt)));
            }
            num.push(c);
        } else {
            if !num.is_empty() {
                out.push((0, num.parse().unwrap_or(0), String::new()));
                num.clear();
            }
            txt.push(c);
        }
    }
    if !num.is_empty() {
        out.push((0, num.parse().unwrap_or(0), String::new()));
    }
    if !txt.is_empty() {
        out.push((1, 0, txt));
    }
    out
}

pub fn extract_cbz(path: &Path, o: &Opts, progress: Progress) -> Result<Extracted, String> {
    if o.ocr_mode == "off" {
        return Err(tr("A képregény oldalai csak OCR-rel olvashatók, de az OCR ki van kapcsolva.", "The pages of a comic book can only be read with OCR, but OCR is switched off."));
    }
    let mut z = open_zip(path)?;
    let mut names: Vec<String> = z
        .file_names()
        .filter(|n| !n.ends_with('/') && !n.contains("__MACOSX") && n.rsplit('.').next().is_some_and(|e| IMAGE_EXTS.contains(&e.to_lowercase().as_str())))
        .map(str::to_string)
        .collect();
    names.sort_by_key(|n| natural_key(n));
    if names.is_empty() {
        return Err(tr("Az archívumban nincs kép.", "There is no image in the archive."));
    }
    let total = names.len();
    let mut pages = vec![];
    for (i, name) in names.iter().enumerate() {
        o.check()?;
        let no = i as i64 + 1;
        if let Some(p) = o.done.get(&no) {
            pages.push(p.clone());
            continue;
        }
        progress("ocr", i, total);
        let bytes = zip_bytes(&mut z, name).unwrap_or_default();
        let text = crate::ocr::ocr_png(&bytes, &o.ocr).map_err(|e| e.to_string())?;
        let p = PageText { no, text, ocr: true };
        if let Some(save) = o.save {
            save(&p);
        }
        pages.push(p);
    }
    progress("ocr", total, total);
    Ok(Extracted { pages, outline: vec![], title: None })
}

// ───────────────────────── DjVu (DjVuLibre) ─────────────────────────

/// A DjVuLibre tool (djvutxt, ddjvu, djvused) from PATH or the usual install folders.
fn djvu_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_string() };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if cfg!(windows) {
        for v in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(d) = std::env::var_os(v) {
                dirs.push(PathBuf::from(&d).join("DjVuLibre"));
                dirs.push(PathBuf::from(&d).join("Programs").join("DjVuLibre"));
            }
        }
    } else {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/Applications/DjView.app/Contents/bin"].map(PathBuf::from));
    }
    dirs.into_iter().map(|d| d.join(&exe)).find(|p| p.is_file())
}

fn run_tool(exe: &Path, args: &[&OsStr]) -> Result<Vec<u8>, String> {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = c.args(args).output().map_err(|e| format!("{}: {e}", exe.display()))?;
    if !out.status.success() {
        return Err(format!("{}: {}", exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

pub fn extract_djvu(path: &Path, o: &Opts, progress: Progress) -> Result<Extracted, String> {
    let missing = || {
        tr(
            "A DjVu fájlokhoz a DjVuLibre kell (djvutxt, ddjvu). Telepítsd: djvu.sourceforge.net (Windows), brew install djvulibre (macOS), apt install djvulibre-bin (Linux).",
            "DjVu files need DjVuLibre (djvutxt, ddjvu). Install it: djvu.sourceforge.net (Windows), brew install djvulibre (macOS), apt install djvulibre-bin (Linux).",
        )
    };
    let txt = djvu_tool("djvutxt").ok_or_else(missing)?;
    let p = path.as_os_str();
    // the number of pages
    let total = djvu_tool("djvused")
        .and_then(|ds| run_tool(&ds, &[OsStr::new("-e"), OsStr::new("n"), p]).ok())
        .and_then(|o| String::from_utf8_lossy(&o).trim().parse::<usize>().ok())
        .unwrap_or(0);
    let full = run_tool(&txt, &[p])?;
    let full = String::from_utf8_lossy(&full).into_owned();
    // djvutxt ends every page with a form feed
    let mut texts: Vec<String> = full.split('\u{c}').map(|s| s.to_string()).collect();
    if texts.last().is_some_and(|s| s.trim().is_empty()) {
        texts.pop();
    }
    let total = total.max(texts.len()).max(1);
    texts.resize(total, String::new());
    let ddjvu = djvu_tool("ddjvu");
    let mut pages = vec![];
    for (i, t) in texts.into_iter().enumerate() {
        o.check()?;
        let no = i as i64 + 1;
        if let Some(p) = o.done.get(&no) {
            pages.push(p.clone());
            continue;
        }
        progress("page", i, total);
        let need_ocr = o.ocr_mode == "force" || (o.ocr_mode != "off" && t.chars().filter(|c| !c.is_whitespace()).count() < o.min_chars as usize);
        let page = match (&ddjvu, need_ocr) {
            (Some(dd), true) => {
                progress("ocr", i, total);
                let tmp = std::env::temp_dir().join(format!("skillstar-djvu-{}-{no}.tif", std::process::id()));
                let page_arg = format!("-page={no}");
                let r = run_tool(dd, &[OsStr::new("-format=tiff"), OsStr::new(&page_arg), p, tmp.as_os_str()])
                    .and_then(|_| crate::ocr::ocr_file(&tmp, &o.ocr).map_err(|e| e.to_string()));
                let _ = std::fs::remove_file(&tmp);
                match r {
                    Ok(text) => PageText { no, text, ocr: true },
                    Err(_) if !t.trim().is_empty() => PageText { no, text: t, ocr: false },
                    Err(e) => return Err(e),
                }
            }
            _ => PageText { no, text: t, ocr: false },
        };
        if let Some(save) = o.save {
            save(&page);
        }
        pages.push(page);
    }
    progress("page", total, total);
    if pages.iter().all(|p| p.text.trim().is_empty()) && ddjvu.is_none() && o.ocr_mode != "off" {
        return Err(tr("A DjVu fájlnak nincs szövegrétege, az OCR-hez a DjVuLibre ddjvu programja kell.", "The DjVu file has no text layer; OCR needs the ddjvu program of DjVuLibre."));
    }
    Ok(Extracted { pages, outline: vec![], title: None })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn palmdoc_cases() {
        // "abc" literals, back reference (distance 3, length 3), space + 'x', a run of 2 literals
        let src = [b'a', b'b', b'c', 0x80, 0x18, b'x' ^ 0x80, 2, b'!', b'?'];
        assert_eq!(palmdoc_decompress(&src), b"abcabc x!?");
    }

    #[test]
    fn trailing_entries() {
        // one trailing entry of size 3 (…, 0x83 = value 3 with the stop bit), multibyte flag off
        let rec = [b'h', b'i', 0x00, 0x00, 0x83];
        assert_eq!(trailing_size(&rec, 0b10), 3);
        // multibyte only: last byte & 3 = 1 → 2 bytes
        let rec = [b'h', b'i', b'x', 0x01];
        assert_eq!(trailing_size(&rec, 0b1), 2);
    }

    #[test]
    fn rtf_basics() {
        let t = rtf_to_text(r"{\rtf1\ansi\ansicpg1250{\fonttbl{\f0 Arial;}}{\*\generator x;}\pard Hello \b world\b0 !\par \'e1rv\u337?z\par{\info{\title T}}End}");
        assert_eq!(t.trim(), "Hello world!\nárvőz\nEnd");
    }

    #[test]
    fn cp1251_letters() {
        assert_eq!([0xCF, 0xF0, 0xE8].iter().map(|&c| cp1251(c)).collect::<String>(), "При");
    }

    #[test]
    fn zip_paths() {
        assert_eq!(norm_zip_path("OEBPS/text", "../images/a%20b.png#x"), "OEBPS/images/a b.png");
        assert_eq!(norm_zip_path("", "ch1.xhtml"), "ch1.xhtml");
    }

    #[test]
    fn epub_and_fb2() {
        let dir = std::env::temp_dir().join(format!("skillstar-ebook-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // a tiny EPUB 2 with an NCX
        let epub = dir.join("t.epub");
        {
            let f = std::fs::File::create(&epub).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o: zip::write::SimpleFileOptions = Default::default();
            let mut put = |n: &str, s: &str| {
                z.start_file(n, o).unwrap();
                z.write_all(s.as_bytes()).unwrap();
            };
            put("mimetype", "application/epub+zip");
            put("META-INF/container.xml", r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#);
            put("OEBPS/content.opf", r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Test Book</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#);
            put("OEBPS/toc.ncx", r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint id="a"><navLabel><text>One</text></navLabel><content src="text/c1.xhtml"/></navPoint><navPoint id="b"><navLabel><text>Two</text></navLabel><content src="text/c2.xhtml#s"/></navPoint></navMap></ncx>"#);
            put("OEBPS/text/c1.xhtml", "<html><body><h1>One</h1><p>First chapter.</p></body></html>");
            put("OEBPS/text/c2.xhtml", "<html><body><h1>Two</h1><p>Second chapter.</p></body></html>");
            z.finish().unwrap();
        }
        let x = extract_epub(&epub).unwrap();
        assert_eq!(x.title.as_deref(), Some("Test Book"));
        assert_eq!(x.pages.len(), 2);
        assert!(x.pages[1].text.contains("Second chapter."));
        let toc: Vec<_> = x.outline.iter().map(|o| (o.title.as_str(), o.page)).collect();
        assert_eq!(toc, vec![("One", 1), ("Two", 2)]);

        let fb2 = dir.join("t.fb2");
        std::fs::write(&fb2, r#"<?xml version="1.0" encoding="utf-8"?><FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0"><description><title-info><book-title>Könyv</book-title></title-info></description><body><section><title><p>Első</p></title><p>Szöveg <emphasis>dőlt</emphasis>.</p></section><section><title><p>Második</p></title><p>Más.</p></section></body></FictionBook>"#).unwrap();
        let x = extract_fb2(&fb2).unwrap();
        assert_eq!(x.title.as_deref(), Some("Könyv"));
        assert_eq!(x.pages.len(), 2);
        assert!(x.pages[0].text.contains("Szöveg *dőlt*."));
        assert_eq!(x.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>(), vec!["Első", "Második"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mobi_uncompressed() {
        // PalmDB header + 2 records: record 0 = PalmDOC header (no MOBI), record 1 = text
        let text = b"Hello Kindle";
        let mut r0 = vec![0u8; 16];
        r0[0..2].copy_from_slice(&1u16.to_be_bytes()); // no compression
        r0[4..8].copy_from_slice(&(text.len() as u32).to_be_bytes());
        r0[8..10].copy_from_slice(&1u16.to_be_bytes());
        let mut b = vec![0u8; 78];
        b[0..4].copy_from_slice(b"Test");
        b[60..68].copy_from_slice(b"TEXtREAd");
        b[76..78].copy_from_slice(&2u16.to_be_bytes());
        let start = 78 + 2 * 8 + 2;
        b.extend_from_slice(&(start as u32).to_be_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&((start + r0.len()) as u32).to_be_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&[0, 0]);
        b.extend_from_slice(&r0);
        b.extend_from_slice(text);
        let p = std::env::temp_dir().join(format!("skillstar-t-{}.pdb", std::process::id()));
        std::fs::write(&p, &b).unwrap();
        let x = extract_mobi(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        assert_eq!(x.pages[0].text, "Hello Kindle");
        assert_eq!(x.title.as_deref(), Some("Test"));
    }
}

#[cfg(test)]
mod samples {
    /// SKILLSTAR_SAMPLES="a.epub;b.mobi" cargo test --lib samples -- --ignored --nocapture
    #[test]
    #[ignore]
    fn real_files() {
        let list = std::env::var("SKILLSTAR_SAMPLES").unwrap_or_default();
        for p in list.split(';').filter(|s| !s.is_empty()) {
            let path = std::path::Path::new(p);
            let r = match crate::extract::kind_of(path) {
                Some("epub") => super::extract_epub(path),
                Some("mobi") => super::extract_mobi(path),
                Some("fb2") => super::extract_fb2(path),
                Some("rtf") => super::extract_rtf(path),
                k => Err(format!("kind {k:?}")),
            };
            match r {
                Ok(x) => {
                    let chars: usize = x.pages.iter().map(|p| p.text.chars().count()).sum();
                    let mid = &x.pages[x.pages.len() / 2].text;
                    println!("OK {} | title {:?} | {} pages, {} chars, {} toc | toc[..3] {:?}\n   sample: {:?}", path.file_name().unwrap().to_string_lossy(), x.title, x.pages.len(), chars, x.outline.len(),
                        x.outline.iter().take(3).map(|o| (o.level, o.title.as_str(), o.page)).collect::<Vec<_>>(), mid.chars().take(160).collect::<String>());
                }
                Err(e) => println!("ERR {}: {e}", path.display()),
            }
        }
    }
}
