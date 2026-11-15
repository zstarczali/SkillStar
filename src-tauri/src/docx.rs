//! Office documents → Markdown: DOCX (headings, lists, tables, bold/italic,
//! page breaks), PPTX (one page per slide) and ODT.

use crate::extract::{chunk, Extracted, OutlineEntry, PageText, CHUNK_CHARS};
use roxmltree::{Document, Node};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

fn open_zip(path: &Path) -> Result<zip::ZipArchive<std::fs::File>, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", crate::settings::tr("Hibás vagy sérült fájl", "Invalid or damaged file")))
}

fn zip_text(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut f = z.by_name(name).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s)
}

fn parse(xml: &str) -> Result<Document<'_>, String> {
    roxmltree::Document::parse_with_options(xml, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() }).map_err(|e| e.to_string())
}

fn attr<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

fn child<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

fn on(n: Option<Node>) -> bool {
    match n {
        None => false,
        Some(n) => !matches!(attr(n, "val"), Some("0") | Some("false") | Some("none")),
    }
}

/// Heading level of a paragraph style ("Heading 2", "Címsor 2", outlineLvl 1 → 2; "Title" → 1).
fn heading_levels(styles: &str) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    let Ok(doc) = parse(styles) else { return map };
    for s in doc.descendants().filter(|n| n.tag_name().name() == "style") {
        let Some(id) = attr(s, "styleId") else { continue };
        let name = child(s, "name").and_then(|n| attr(n, "val")).unwrap_or("").to_lowercase();
        let lvl = child(s, "pPr").and_then(|p| child(p, "outlineLvl")).and_then(|o| attr(o, "val")).and_then(|v| v.parse::<usize>().ok());
        let level = if let Some(l) = lvl.filter(|&l| l < 9) {
            Some(l + 1)
        } else if name == "title" || name == "cím" {
            Some(1)
        } else {
            ["heading", "címsor", "cimsor", "überschrift", "titre"]
                .iter()
                .find_map(|p| name.strip_prefix(p))
                .and_then(|rest| rest.trim().parse::<usize>().ok())
                .filter(|l| (1..=9).contains(l))
        };
        if let Some(l) = level {
            map.insert(id.to_string(), l.min(6));
        }
    }
    map
}

/// numId → (ilvl → ordered?) from numbering.xml.
fn numbering(xml: &str) -> HashMap<String, HashMap<String, bool>> {
    let mut out = HashMap::new();
    let Ok(doc) = parse(xml) else { return out };
    let mut abs: HashMap<String, HashMap<String, bool>> = HashMap::new();
    for a in doc.descendants().filter(|n| n.tag_name().name() == "abstractNum") {
        let Some(id) = attr(a, "abstractNumId") else { continue };
        let mut lv = HashMap::new();
        for l in a.children().filter(|c| c.tag_name().name() == "lvl") {
            let fmt = child(l, "numFmt").and_then(|f| attr(f, "val")).unwrap_or("bullet");
            lv.insert(attr(l, "ilvl").unwrap_or("0").to_string(), !matches!(fmt, "bullet" | "none"));
        }
        abs.insert(id.to_string(), lv);
    }
    for n in doc.descendants().filter(|n| n.tag_name().name() == "num") {
        if let (Some(id), Some(a)) = (attr(n, "numId"), child(n, "abstractNumId").and_then(|a| attr(a, "val"))) {
            if let Some(lv) = abs.get(a) {
                out.insert(id.to_string(), lv.clone());
            }
        }
    }
    out
}

enum Seg {
    Text(String),
    Break,
}

struct Ctx {
    headings: HashMap<String, usize>,
    numbering: HashMap<String, HashMap<String, bool>>,
    /// running counters of ordered lists: (numId, ilvl) → n
    counters: HashMap<(String, String), usize>,
}

/// Inline text of a paragraph (runs, hyperlinks, insertions), with page breaks.
fn runs(n: Node, segs: &mut Vec<Seg>) {
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "r" => {
                let rpr = child(c, "rPr");
                let bold = rpr.map(|p| on(child(p, "b"))).unwrap_or(false);
                let ital = rpr.map(|p| on(child(p, "i"))).unwrap_or(false);
                let mut text = String::new();
                for t in c.children().filter(|t| t.is_element()) {
                    match t.tag_name().name() {
                        "t" => text.push_str(t.text().unwrap_or("")),
                        "tab" => text.push('\t'),
                        "noBreakHyphen" => text.push('-'),
                        "br" if attr(t, "type") == Some("page") => {
                            push_styled(segs, &std::mem::take(&mut text), bold, ital);
                            segs.push(Seg::Break);
                        }
                        "br" | "cr" => text.push('\n'),
                        "lastRenderedPageBreak" => {
                            push_styled(segs, &std::mem::take(&mut text), bold, ital);
                            segs.push(Seg::Break);
                        }
                        _ => {}
                    }
                }
                push_styled(segs, &text, bold, ital);
            }
            "hyperlink" | "ins" | "smartTag" | "fldSimple" | "sdt" | "sdtContent" | "customXml" => runs(c, segs),
            _ => {}
        }
    }
}

fn push_styled(segs: &mut Vec<Seg>, text: &str, bold: bool, ital: bool) {
    if text.is_empty() {
        return;
    }
    let core = text.trim();
    let s = if core.is_empty() || !(bold || ital) || core.contains('\n') {
        text.to_string()
    } else {
        let mark = match (bold, ital) {
            (true, true) => "***",
            (true, false) => "**",
            _ => "*",
        };
        let lead = &text[..text.len() - text.trim_start().len()];
        let trail = &text[text.trim_end().len()..];
        format!("{lead}{mark}{core}{mark}{trail}")
    };
    if let Some(Seg::Text(last)) = segs.last_mut() {
        last.push_str(&s);
    } else {
        segs.push(Seg::Text(s));
    }
}

/// Joins `**a** **b**` style runs written separately.
fn merge_marks(s: &str) -> String {
    s.replace("****", "").replace("** **", " ").replace("\u{a0}", " ")
}

struct Out {
    pages: Vec<String>,
    outline: Vec<OutlineEntry>,
    title: Option<String>,
    breaks: usize,
}

impl Out {
    fn cur(&mut self) -> &mut String {
        self.pages.last_mut().unwrap()
    }
    fn brk(&mut self) {
        // a break on an empty page (e.g. Word's marker right at the top) is ignored
        if !self.cur().trim().is_empty() {
            self.pages.push(String::new());
            self.breaks += 1;
        }
    }
    fn block(&mut self, s: &str) {
        let c = self.cur();
        if !c.is_empty() && !c.ends_with("\n\n") {
            c.push_str(if c.ends_with('\n') { "\n" } else { "\n\n" });
        }
        c.push_str(s);
        c.push('\n');
    }
    fn line(&mut self, s: &str) {
        let c = self.cur();
        c.push_str(s);
        c.push('\n');
    }
}

fn paragraph(p: Node, cx: &mut Ctx, out: &mut Out, in_list: &mut bool) {
    let ppr = child(p, "pPr");
    if ppr.map(|x| on(child(x, "pageBreakBefore"))).unwrap_or(false) {
        out.brk();
    }
    let style = ppr.and_then(|x| child(x, "pStyle")).and_then(|s| attr(s, "val")).unwrap_or("");
    let direct = ppr.and_then(|x| child(x, "outlineLvl")).and_then(|o| attr(o, "val")).and_then(|v| v.parse::<usize>().ok()).filter(|&l| l < 9).map(|l| l + 1);
    let level = direct.or_else(|| cx.headings.get(style).copied());
    let num = ppr.and_then(|x| child(x, "numPr"));
    let mut segs = vec![];
    runs(p, &mut segs);
    let mut first = true;
    for seg in segs {
        match seg {
            Seg::Break => out.brk(),
            Seg::Text(t) => {
                let t = merge_marks(&t);
                let text = t.trim();
                if text.is_empty() {
                    continue;
                }
                if let (Some(l), true) = (level, first) {
                    let plain = text.replace('*', "");
                    let plain = plain.trim().to_string();
                    if out.title.is_none() && l == 1 {
                        out.title = Some(plain.clone());
                    }
                    let page = out.pages.len() as i64;
                    out.outline.push(OutlineEntry { level: l as i64, title: plain.clone(), page });
                    *in_list = false;
                    out.block(&format!("{} {}", "#".repeat(l), plain));
                } else if let (Some(n), true) = (num, first) {
                    let id = child(n, "numId").and_then(|x| attr(x, "val")).unwrap_or("0").to_string();
                    let ilvl = child(n, "ilvl").and_then(|x| attr(x, "val")).unwrap_or("0").to_string();
                    if id == "0" {
                        out.block(text);
                        *in_list = false;
                    } else {
                        let ordered = cx.numbering.get(&id).and_then(|m| m.get(&ilvl)).copied().unwrap_or(false);
                        let depth = ilvl.parse::<usize>().unwrap_or(0).min(6);
                        let marker = if ordered {
                            let k = cx.counters.entry((id.clone(), ilvl.clone())).or_insert(0);
                            *k += 1;
                            // deeper levels restart when a shallower item comes
                            let deeper: Vec<_> = cx.counters.keys().filter(|(i, l)| *i == id && l.parse::<usize>().unwrap_or(0) > depth).cloned().collect();
                            for k in deeper {
                                cx.counters.remove(&k);
                            }
                            format!("{}.", cx.counters[&(id.clone(), ilvl.clone())])
                        } else {
                            "-".to_string()
                        };
                        let line = format!("{}{} {}", "   ".repeat(depth), marker, text.replace('\n', " "));
                        if *in_list { out.line(&line) } else { out.block(&line) }
                        *in_list = true;
                    }
                } else {
                    if *in_list && first {
                        *in_list = false;
                    }
                    out.block(text);
                }
                first = false;
            }
        }
    }
}

fn cell_text(cell: Node, cx: &mut Ctx) -> String {
    let mut parts = vec![];
    for p in cell.descendants().filter(|n| n.tag_name().name() == "p") {
        let mut segs = vec![];
        runs(p, &mut segs);
        let t: String = segs.into_iter().filter_map(|s| if let Seg::Text(t) = s { Some(t) } else { None }).collect();
        let t = merge_marks(&t).split_whitespace().collect::<Vec<_>>().join(" ");
        if !t.is_empty() {
            parts.push(t);
        }
    }
    let _ = cx;
    parts.join("<br>").replace('|', "\\|")
}

fn table(t: Node, cx: &mut Ctx, out: &mut Out) {
    let rows: Vec<Vec<String>> = t
        .children()
        .filter(|r| r.tag_name().name() == "tr")
        .map(|r| r.children().filter(|c| c.tag_name().name() == "tc").map(|c| cell_text(c, cx)).collect())
        .filter(|r: &Vec<String>| !r.is_empty())
        .collect();
    if rows.is_empty() {
        return;
    }
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(1);
    let mut s = String::new();
    for (i, r) in rows.iter().enumerate() {
        let mut cells = r.clone();
        cells.resize(cols, String::new());
        s.push_str("| ");
        s.push_str(&cells.join(" | "));
        s.push_str(" |\n");
        if i == 0 {
            s.push('|');
            s.push_str(&" --- |".repeat(cols));
            s.push('\n');
        }
    }
    out.block(s.trim_end());
}

fn body(n: Node, cx: &mut Ctx, out: &mut Out, in_list: &mut bool) {
    for c in n.children().filter(|c| c.is_element()) {
        match c.tag_name().name() {
            "p" => paragraph(c, cx, out, in_list),
            "tbl" => {
                *in_list = false;
                table(c, cx, out);
            }
            "sdt" | "sdtContent" | "customXml" | "ins" => body(c, cx, out, in_list),
            _ => {}
        }
    }
}

fn core_title(z: &mut zip::ZipArchive<std::fs::File>) -> Option<String> {
    let xml = zip_text(z, "docProps/core.xml")?;
    let doc = parse(&xml).ok()?;
    let t = doc.descendants().find(|n| n.tag_name().name() == "title")?.text()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn finish(out: Out) -> Extracted {
    let Out { mut pages, mut outline, title, breaks } = out;
    while pages.len() > 1 && pages.last().map(|p| p.trim().is_empty()).unwrap_or(false) {
        pages.pop();
    }
    // no page breaks at all: cut into pseudo pages (and move the outline along)
    if breaks == 0 && pages.len() == 1 && pages[0].chars().count() > CHUNK_CHARS * 2 {
        let parts = chunk(&pages[0], CHUNK_CHARS);
        let mut idx = 0;
        for o in &mut outline {
            let head = format!("{} {}", "#".repeat(o.level as usize), o.title);
            while idx < parts.len() && !parts[idx].contains(&head) {
                idx += 1;
            }
            if idx == parts.len() {
                break;
            }
            o.page = idx as i64 + 1;
        }
        pages = parts;
    }
    Extracted {
        pages: pages.into_iter().enumerate().map(|(i, t)| PageText { no: i as i64 + 1, text: t, ocr: false }).collect(),
        outline,
        title,
    }
}

pub fn extract(path: &Path) -> Result<Extracted, String> {
    let mut z = open_zip(path)?;
    let xml = zip_text(&mut z, "word/document.xml").ok_or_else(|| crate::settings::tr("Nem DOCX dokumentum (hiányzik a word/document.xml).", "Not a DOCX document (word/document.xml is missing)."))?;
    from_parts(&xml, zip_text(&mut z, "word/styles.xml").as_deref(), zip_text(&mut z, "word/numbering.xml").as_deref(), core_title(&mut z))
}

pub fn from_parts(document: &str, styles: Option<&str>, numbering_xml: Option<&str>, title: Option<String>) -> Result<Extracted, String> {
    let doc = parse(document)?;
    let mut cx = Ctx {
        headings: styles.map(heading_levels).unwrap_or_default(),
        numbering: numbering_xml.map(numbering).unwrap_or_default(),
        counters: HashMap::new(),
    };
    let mut out = Out { pages: vec![String::new()], outline: vec![], title: None, breaks: 0 };
    let mut in_list = false;
    if let Some(b) = doc.descendants().find(|n| n.tag_name().name() == "body") {
        body(b, &mut cx, &mut out, &mut in_list);
    }
    if title.is_some() {
        out.title = title;
    }
    Ok(finish(out))
}

// ───────────────────────── PPTX ─────────────────────────

pub fn extract_pptx(path: &Path) -> Result<Extracted, String> {
    let mut z = open_zip(path)?;
    let mut slides: Vec<(usize, String)> = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().map(|f| f.name().to_string()))
        .filter_map(|n| {
            let num = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?.parse::<usize>().ok()?;
            Some((num, n))
        })
        .collect();
    slides.sort();
    let title = core_title(&mut z);
    let mut pages = vec![];
    let mut outline = vec![];
    for (i, (_, name)) in slides.iter().enumerate() {
        let xml = zip_text(&mut z, name).unwrap_or_default();
        let doc = parse(&xml)?;
        let mut blocks: Vec<String> = vec![];
        let mut slide_title = None;
        for sp in doc.descendants().filter(|n| n.tag_name().name() == "sp" || n.tag_name().name() == "graphicFrame") {
            let is_title = sp.descendants().any(|n| n.tag_name().name() == "ph" && matches!(attr(n, "type"), Some("title") | Some("ctrTitle")));
            let mut paras = vec![];
            for p in sp.descendants().filter(|n| n.tag_name().name() == "p" && n.tag_name().namespace().map(|ns| ns.contains("drawingml")).unwrap_or(true)) {
                let t: String = p.descendants().filter(|n| n.tag_name().name() == "t").filter_map(|n| n.text()).collect();
                let t = t.trim().to_string();
                if !t.is_empty() {
                    let lvl = child(p, "pPr").and_then(|x| attr(x, "lvl")).and_then(|v| v.parse::<usize>().ok()).unwrap_or(0);
                    paras.push((lvl, t));
                }
            }
            if paras.is_empty() {
                continue;
            }
            if is_title && slide_title.is_none() {
                let t = paras.iter().map(|p| p.1.clone()).collect::<Vec<_>>().join(" ");
                slide_title = Some(t.clone());
                blocks.insert(0, format!("## {t}"));
            } else if paras.len() == 1 {
                blocks.push(paras[0].1.clone());
            } else {
                blocks.push(paras.iter().map(|(l, t)| format!("{}- {t}", "   ".repeat((*l).min(6)))).collect::<Vec<_>>().join("\n"));
            }
        }
        if let Some(t) = &slide_title {
            outline.push(OutlineEntry { level: 1, title: t.clone(), page: i as i64 + 1 });
        }
        pages.push(PageText { no: i as i64 + 1, text: blocks.join("\n\n"), ocr: false });
    }
    let title = title.or_else(|| outline.first().map(|o| o.title.clone()));
    Ok(Extracted { pages, outline, title })
}

// ───────────────────────── ODT ─────────────────────────

pub fn extract_odt(path: &Path) -> Result<Extracted, String> {
    let mut z = open_zip(path)?;
    let xml = zip_text(&mut z, "content.xml").ok_or_else(|| crate::settings::tr("Nem ODT dokumentum.", "Not an ODT document."))?;
    let doc = parse(&xml)?;
    let mut out = Out { pages: vec![String::new()], outline: vec![], title: None, breaks: 0 };
    fn text_of(n: Node) -> String {
        let mut s = String::new();
        for d in n.children() {
            if d.is_text() {
                s.push_str(d.text().unwrap_or(""));
            } else if d.is_element() {
                match d.tag_name().name() {
                    "s" => s.push(' '),
                    "tab" => s.push('\t'),
                    "line-break" => s.push('\n'),
                    "note" => {}
                    _ => s.push_str(&text_of(d)),
                }
            }
        }
        s
    }
    fn walk(n: Node, out: &mut Out, depth: usize) {
        for c in n.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "h" => {
                    let l = attr(c, "outline-level").and_then(|v| v.parse::<usize>().ok()).unwrap_or(1).clamp(1, 6);
                    let t = text_of(c).trim().to_string();
                    if t.is_empty() {
                        continue;
                    }
                    if out.title.is_none() && l == 1 {
                        out.title = Some(t.clone());
                    }
                    let page = out.pages.len() as i64;
                    out.outline.push(OutlineEntry { level: l as i64, title: t.clone(), page });
                    out.block(&format!("{} {t}", "#".repeat(l)));
                }
                "p" => {
                    if c.descendants().any(|d| d.tag_name().name() == "soft-page-break") {
                        out.brk();
                    }
                    let t = text_of(c).trim().to_string();
                    if !t.is_empty() {
                        out.block(&t);
                    }
                }
                "soft-page-break" => out.brk(),
                "list" => {
                    for item in c.children().filter(|i| i.tag_name().name() == "list-item") {
                        for p in item.children().filter(|p| p.is_element()) {
                            if p.tag_name().name() == "list" {
                                let mut inner = Out { pages: vec![String::new()], outline: vec![], title: None, breaks: 0 };
                                walk(p, &mut inner, depth + 1);
                                for l in inner.pages.concat().lines().filter(|l| !l.trim().is_empty()) {
                                    out.line(l);
                                }
                            } else {
                                let t = text_of(p).trim().to_string();
                                if !t.is_empty() {
                                    let line = format!("{}- {t}", "   ".repeat(depth));
                                    out.line(&line);
                                }
                            }
                        }
                    }
                    out.cur().push('\n');
                }
                "table" => {
                    let rows: Vec<Vec<String>> = c
                        .descendants()
                        .filter(|r| r.tag_name().name() == "table-row")
                        .map(|r| r.children().filter(|x| x.tag_name().name() == "table-cell").map(|x| text_of(x).split_whitespace().collect::<Vec<_>>().join(" ").replace('|', "\\|")).collect())
                        .collect();
                    if let Some(first) = rows.first() {
                        let cols = first.len().max(1);
                        let mut s = String::new();
                        for (i, r) in rows.iter().enumerate() {
                            let mut r = r.clone();
                            r.resize(cols, String::new());
                            s.push_str(&format!("| {} |\n", r.join(" | ")));
                            if i == 0 {
                                s.push_str(&format!("|{}\n", " --- |".repeat(cols)));
                            }
                        }
                        out.block(s.trim_end());
                    }
                }
                "section" | "text" | "body" => walk(c, out, depth),
                _ => {}
            }
        }
    }
    if let Some(b) = doc.descendants().find(|n| n.tag_name().name() == "text" && n.parent().map(|p| p.tag_name().name() == "body").unwrap_or(false)) {
        walk(b, &mut out, 0);
    }
    Ok(finish(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

    #[test]
    fn docx_to_markdown() {
        let styles = format!(
            r#"<w:styles {W}>
              <w:style w:styleId="Cmsor1"><w:name w:val="heading 1"/></w:style>
              <w:style w:styleId="Sajat"><w:name w:val="Saját"/><w:pPr><w:outlineLvl w:val="1"/></w:pPr></w:style>
            </w:styles>"#
        );
        let numbering = format!(
            r#"<w:numbering {W}>
              <w:abstractNum w:abstractNumId="1"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/></w:lvl></w:abstractNum>
              <w:abstractNum w:abstractNumId="2"><w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/></w:lvl></w:abstractNum>
              <w:num w:numId="5"><w:abstractNumId w:val="1"/></w:num>
              <w:num w:numId="6"><w:abstractNumId w:val="2"/></w:num>
            </w:numbering>"#
        );
        let doc = format!(
            r#"<w:document {W}><w:body>
              <w:p><w:pPr><w:pStyle w:val="Cmsor1"/></w:pPr><w:r><w:t>Bevezetés</w:t></w:r></w:p>
              <w:p><w:r><w:t xml:space="preserve">Ez </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>fontos</w:t></w:r><w:r><w:t xml:space="preserve"> szöveg.</w:t></w:r></w:p>
              <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="5"/></w:numPr></w:pPr><w:r><w:t>első</w:t></w:r></w:p>
              <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="5"/></w:numPr></w:pPr><w:r><w:t>második</w:t></w:r></w:p>
              <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="6"/></w:numPr></w:pPr><w:r><w:t>pont</w:t></w:r></w:p>
              <w:tbl><w:tr><w:tc><w:p><w:r><w:t>A</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>B|C</w:t></w:r></w:p></w:tc></w:tr>
                     <w:tr><w:tc><w:p><w:r><w:t>1</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>2</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
              <w:p><w:r><w:br w:type="page"/></w:r></w:p>
              <w:p><w:pPr><w:pStyle w:val="Sajat"/></w:pPr><w:r><w:lastRenderedPageBreak/><w:t>Második rész</w:t></w:r></w:p>
              <w:p><w:r><w:rPr><w:i/></w:rPr><w:t>dőlt</w:t></w:r></w:p>
            </w:body></w:document>"#
        );
        let x = from_parts(&doc, Some(&styles), Some(&numbering), None).unwrap();
        assert_eq!(x.pages.len(), 2, "{:?}", x.pages);
        assert_eq!(
            x.pages[0].text,
            "# Bevezetés\n\nEz **fontos** szöveg.\n\n1. első\n2. második\n- pont\n\n| A | B\\|C |\n| --- | --- |\n| 1 | 2 |\n"
        );
        assert_eq!(x.pages[1].text, "## Második rész\n\n*dőlt*\n");
        let o: Vec<_> = x.outline.iter().map(|o| (o.level, o.title.as_str(), o.page)).collect();
        assert_eq!(o, vec![(1, "Bevezetés", 1), (2, "Második rész", 2)]);
        assert_eq!(x.title.as_deref(), Some("Bevezetés"));
    }

    #[test]
    fn long_docx_without_breaks_is_chunked() {
        let para = format!("<w:p><w:r><w:t>{}</w:t></w:r></w:p>", "szó ".repeat(200));
        let doc = format!(
            r#"<w:document {W}><w:body><w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>Cím</w:t></w:r></w:p>{}<w:p><w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>Vége</w:t></w:r></w:p>{}</w:body></w:document>"#,
            para.repeat(20),
            para.repeat(5)
        );
        let x = from_parts(&doc, None, None, Some("Meta cím".into())).unwrap();
        assert!(x.pages.len() > 3);
        assert_eq!(x.title.as_deref(), Some("Meta cím"));
        assert_eq!(x.outline[0].page, 1);
        assert!(x.outline[1].page > 3);
        assert!(x.pages[(x.outline[1].page - 1) as usize].text.contains("# Vége"));
    }
}
