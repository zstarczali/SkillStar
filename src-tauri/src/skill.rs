//! Builds the skill package from the processed documents:
//!
//! ```text
//! <out>/<name>/                     Claude Code plugin folder
//!   .claude-plugin/plugin.json
//!   README.md
//!   skills/<name>/SKILL.md          (knowledge root: ../../knowledge/)
//!   knowledge/index.md              table of contents + keyword index → document / page
//!   knowledge/DOCUMENTS.md          what each document is, chapters, "use when"
//!   knowledge/sources/<doc>.md      full text, `--- PAGE n ---` markers
//! <out>/<name>.plugin               the same folder as one zip
//! <out>/<name>-skill.zip            a single skill (SKILL.md + knowledge/) for claude.ai / Claude Desktop
//! ```

use crate::db::{self, Document, OutlineItem, Page, Skill};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Marker file: only folders that SkillStar made are ever emptied.
const MARKER: &str = ".skillstar";

/// kebab-case ASCII name (accents folded), at most 64 characters. Words that
/// skill names may not contain ("claude", "anthropic") are dropped.
pub fn slugify(input: &str, fallback: &str) -> String {
    let mut s = String::new();
    for c in input.chars().flat_map(|c| c.to_lowercase()) {
        let f = match c {
            'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' | 'ā' | 'ą' => "a",
            'é' | 'è' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' => "e",
            'í' | 'ì' | 'î' | 'ï' | 'ī' => "i",
            'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ő' | 'ø' | 'ō' => "o",
            'ú' | 'ù' | 'û' | 'ü' | 'ű' | 'ū' | 'ů' => "u",
            'ý' | 'ÿ' => "y",
            'ç' | 'č' | 'ć' => "c",
            'š' | 'ś' | 'ş' => "s",
            'ž' | 'ź' | 'ż' => "z",
            'ň' | 'ñ' | 'ń' => "n",
            'ř' => "r",
            'ď' => "d",
            'ť' => "t",
            'ł' | 'ľ' | 'ĺ' => "l",
            'ß' => "ss",
            'æ' => "ae",
            'œ' => "oe",
            c if c.is_ascii_alphanumeric() => {
                s.push(c);
                continue;
            }
            _ => "-",
        };
        s.push_str(f);
    }
    let words: Vec<&str> = s.split('-').filter(|w| !w.is_empty() && *w != "claude" && *w != "anthropic").collect();
    let mut out = String::new();
    for w in words {
        if out.len() + w.len() + 1 > 64 {
            break;
        }
        if !out.is_empty() {
            out.push('-');
        }
        out.push_str(w);
    }
    if out.is_empty() {
        fallback.to_string()
    } else {
        out
    }
}

struct Doc {
    d: Document,
    pages: Vec<Page>,
    outline: Vec<OutlineItem>,
    file: String,
    code: String,
}

impl Doc {
    fn title(&self) -> &str {
        if self.d.title.trim().is_empty() {
            &self.d.name
        } else {
            self.d.title.trim()
        }
    }
}

/// Texts in the language of the skill.
struct L {
    hu: bool,
}

impl L {
    fn t<'a>(&self, hu: &'a str, en: &'a str) -> &'a str {
        if self.hu {
            hu
        } else {
            en
        }
    }
}

// ───────────────────────── keyword index ─────────────────────────

const STOP: &[&str] = &[
    // en
    "this", "that", "with", "from", "have", "were", "will", "there", "their", "which", "would", "about", "these", "those", "other", "into", "than", "then", "them", "they",
    "when", "what", "your", "some", "more", "also", "such", "only", "over", "each", "most", "very", "must", "been", "being", "here", "where", "after", "before", "under",
    "between", "should", "could", "does", "done", "make", "made", "used", "using", "many", "much", "same", "both", "well", "just", "like", "page", "pages", "first",
    "second", "next", "following", "example", "see", "figure", "table", "chapter", "section", "while", "through", "because", "however", "therefore", "within", "without",
    "upon", "shall", "these", "whose", "every", "another", "even", "since", "until", "again", "once", "only", "because", "against", "during", "above", "below",
    // hu
    "hogy", "nem", "egy", "meg", "csak", "vagy", "mint", "már", "még", "ezt", "azt", "ami", "aki", "amely", "amelyek", "akkor", "ahol", "mert", "pedig", "illetve",
    "ezek", "azok", "ezen", "azon", "ennek", "annak", "neki", "nekik", "lesz", "lett", "volt", "voltak", "vannak", "van", "lehet", "kell", "kell", "csak", "minden",
    "más", "másik", "után", "előtt", "alatt", "felett", "között", "szerint", "mellett", "nélkül", "által", "számára", "miatt", "valamint", "továbbá", "ugyanis", "tehát",
    "mint", "amit", "amik", "amikor", "ahogy", "mely", "melyek", "melyet", "ezért", "azért", "így", "úgy", "itt", "ott", "most", "majd", "pedig", "vagyis", "azaz",
    "oldal", "oldalon", "fejezet", "ábra", "táblázat", "példa", "például", "például", "esetén", "esetben", "során", "egyes", "egyik", "ilyen", "olyan", "amelynek",
    "lesznek", "legyen", "lehetnek", "kellett", "nagyon", "sokkal", "kevés", "több", "kevesebb", "részére", "valamint", "illetőleg", "mindig", "soha", "egész",
    // de
    "und", "der", "die", "das", "ist", "nicht", "mit", "auf", "für", "eine", "einer", "eines", "dem", "den", "des", "sich", "auch", "werden", "wird", "sind", "oder",
];

fn tokens(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !(c.is_alphanumeric() || c == '$' || c == '_' || c == '-'))
        .map(|w| w.trim_matches(|c| c == '-' || c == '_'))
        .filter(|w| {
            let n = w.chars().count();
            (4..=32).contains(&n) && w.chars().any(|c| c.is_alphabetic())
        })
        .map(|w| w.to_lowercase())
}

#[derive(Clone, Debug, Serialize)]
pub struct Term {
    pub term: String,
    /// (document code, page, count)
    pub places: Vec<(String, i64, usize)>,
}

/// The most characteristic words of the collection with the pages where
/// they matter most (a simple tf-idf over pages).
fn keyword_index(docs: &[Doc], max_terms: usize) -> Vec<Term> {
    let stop: HashSet<&str> = STOP.iter().copied().collect();
    // term → (doc idx, page) → count
    let mut occ: HashMap<String, HashMap<(usize, i64), usize>> = HashMap::new();
    let mut n_pages = 0usize;
    for (di, d) in docs.iter().enumerate() {
        for p in &d.pages {
            n_pages += 1;
            for t in tokens(&p.text) {
                if stop.contains(t.as_str()) {
                    continue;
                }
                *occ.entry(t).or_default().entry((di, p.no)).or_insert(0) += 1;
            }
        }
    }
    if n_pages == 0 {
        return vec![];
    }
    let n = n_pages as f64;
    let mut scored: Vec<(f64, String)> = occ
        .iter()
        .filter_map(|(t, places)| {
            let df = places.len() as f64;
            let tf: usize = places.values().sum();
            if tf < 3 || (n_pages >= 10 && df > n * 0.35) {
                return None;
            }
            Some((tf as f64 * (n / df).ln().max(0.1) * (1.0 + (df.min(8.0) / 8.0)), t.clone()))
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    // skip near-duplicates (plural / suffix forms sharing a long prefix)
    let mut picked: Vec<String> = vec![];
    for (_, t) in scored {
        if picked.len() >= max_terms {
            break;
        }
        let stem: String = t.chars().take(6).collect();
        if t.chars().count() > 6 && picked.iter().filter(|p| p.chars().count() > 6 && p.starts_with(&stem)).count() >= 2 {
            continue;
        }
        picked.push(t);
    }
    let mut out: Vec<Term> = picked
        .into_iter()
        .map(|t| {
            let mut places: Vec<_> = occ[&t].iter().map(|(&(di, pg), &c)| (di, pg, c)).collect();
            places.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)).then(a.1.cmp(&b.1)));
            places.truncate(6);
            places.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
            Term { term: t, places: places.into_iter().map(|(di, pg, c)| (docs[di].code.clone(), pg, c)).collect() }
        })
        .collect();
    out.sort_by(|a, b| a.term.cmp(&b.term));
    out
}

/// The top terms of one document.
fn doc_terms(d: &Doc, all: &[Term], k: usize) -> Vec<String> {
    let mut v: Vec<(usize, &str)> = all
        .iter()
        .filter_map(|t| {
            let c: usize = t.places.iter().filter(|p| p.0 == d.code).map(|p| p.2).sum();
            (c > 0).then_some((c, t.term.as_str()))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    v.into_iter().take(k).map(|x| x.1.to_string()).collect()
}

// ───────────────────────── text pieces ─────────────────────────

fn md_escape_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn yaml_str(s: &str) -> String {
    let one: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("\"{}\"", one.replace('\\', "\\\\").replace('"', "\\\""))
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
    if let Some(sp) = t.rfind(' ') {
        if sp > max / 2 {
            t.truncate(sp);
        }
    }
    t.trim_end_matches([',', ';', ' ']).to_string() + "…"
}

fn kind_label(l: &L, d: &Document) -> String {
    let src = match d.kind.as_str() {
        "pdf" if d.ocr_pages == 0 => l.t("PDF szövegréteg", "PDF text layer").to_string(),
        "pdf" if d.ocr_pages >= d.pages => l.t("PDF, OCR", "PDF, OCR").to_string(),
        "pdf" => format!("{} ({} {})", l.t("PDF szövegréteg + OCR", "PDF text layer + OCR"), d.ocr_pages, l.t("oldal OCR", "pages OCR")),
        "image" => l.t("kép, OCR", "image, OCR").to_string(),
        "docx" => "Word (DOCX)".to_string(),
        "pptx" => l.t("PowerPoint (diánként egy oldal)", "PowerPoint (one page per slide)").to_string(),
        "odt" => "OpenDocument (ODT)".to_string(),
        "html" => "HTML".to_string(),
        "md" => "Markdown".to_string(),
        _ => l.t("szöveg", "text").to_string(),
    };
    src
}

fn pages_meaning(l: &L, d: &Document) -> &'static str {
    match d.kind.as_str() {
        "pdf" => l.t("PDF-oldal", "PDF page"),
        "pptx" => l.t("dia", "slide"),
        "docx" | "odt" => l.t("oldal (Word oldaltörés szerint; ha nincs, kb. 3500 karakteres szakasz)", "page (Word page breaks; without them ~3500-character sections)"),
        "image" => l.t("kép", "image"),
        _ => l.t("kb. 3500 karakteres szakasz", "~3500-character section"),
    }
}

fn source_md(l: &L, d: &Doc) -> String {
    let mut s = String::new();
    s.push_str(&format!("# {}\n\n", d.title()));
    s.push_str(&format!(
        "> {}: `{}` · {} {} · {}: {}\n",
        l.t("Eredeti fájl", "Original file"),
        d.d.name,
        d.pages.len(),
        l.t("oldal", "pages"),
        l.t("Szöveg", "Text"),
        kind_label(l, &d.d)
    ));
    s.push_str(&format!("> `--- PAGE n ---` = {}\n", pages_meaning(l, &d.d)));
    if d.d.ocr_pages > 0 {
        s.push_str(&format!(
            "> {}\n",
            l.t(
                "Az OCR-rel olvasott oldalakon betű- és számhibák lehetnek (0/O, 1/l/I, 5/S, ékezetek); pontos értékhez nézd meg az eredeti dokumentumot.",
                "Pages read with OCR may contain character errors (0/O, 1/l/I, 5/S, accents); check the original document for exact values."
            )
        ));
    }
    if !d.d.summary.trim().is_empty() {
        s.push_str(&format!("\n{}\n", d.d.summary.trim()));
    }
    for p in &d.pages {
        s.push_str(&format!("\n--- PAGE {} ---\n", p.no));
        if p.ocr {
            s.push_str("<!-- OCR -->\n");
        }
        s.push_str(p.text.trim_end());
        s.push('\n');
    }
    s
}

fn chapters_line(d: &Doc, max: usize) -> String {
    let min_level = d.outline.iter().map(|o| o.level).min().unwrap_or(1);
    let mut items: Vec<String> = d
        .outline
        .iter()
        .filter(|o| o.level <= min_level + 1)
        .map(|o| if o.page > 0 { format!("{} {}", o.title, o.page) } else { o.title.clone() })
        .collect();
    if items.len() > max {
        items = d.outline.iter().filter(|o| o.level == min_level).map(|o| if o.page > 0 { format!("{} {}", o.title, o.page) } else { o.title.clone() }).collect();
        if items.len() > max {
            items.truncate(max);
            items.push("…".into());
        }
    }
    items.join(" · ")
}

fn documents_md(l: &L, sk: &Skill, docs: &[Doc], terms: &[Term]) -> String {
    let mut s = format!("# {} – {}\n\n", sk.title, l.t("dokumentumok", "documents"));
    s.push_str(l.t(
        "Minden dokumentum teljes szövege a `sources/` mappában van, `--- PAGE n ---` jelölőkkel. Az oldalszámok a dokumentum saját oldalai (PDF-oldal, dia, Word-oldal).\n",
        "The full text of every document is in `sources/`, with `--- PAGE n ---` markers. Page numbers are the document's own pages (PDF page, slide, Word page).\n",
    ));
    for (i, d) in docs.iter().enumerate() {
        s.push_str(&format!("\n---\n\n## {}. {} [{}]\n", i + 1, d.title(), d.code));
        s.push_str(&format!(
            "- **{}:** `sources/{}` ({}: `{}`, {} {})\n",
            l.t("Fájl", "File"),
            d.file,
            l.t("eredeti", "original"),
            d.d.name,
            d.pages.len(),
            l.t("oldal", "pages")
        ));
        s.push_str(&format!("- **{}:** {}\n", l.t("Szöveg", "Text"), kind_label(l, &d.d)));
        if !d.d.summary.trim().is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Összefoglaló", "Summary"), d.d.summary.trim().replace('\n', " ")));
        }
        if !d.outline.is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Fejezetek (oldal)", "Chapters (page)"), chapters_line(d, 40)));
        }
        let kw = doc_terms(d, terms, 15);
        if !kw.is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Kulcsszavak", "Keywords"), kw.join(", ")));
        }
        if !d.d.use_when.trim().is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Használd ha", "Use when"), d.d.use_when.trim().replace('\n', " ")));
        }
    }
    s
}

fn index_md(l: &L, sk: &Skill, docs: &[Doc], terms: &[Term]) -> String {
    let mut s = format!("# {} – {}\n\n", sk.title, l.t("tudásbázis index", "knowledge index"));
    s.push_str(l.t(
        "Gyökér: `knowledge/`. Dokumentumleírások: `DOCUMENTS.md`. Teljes szövegek: `sources/<fájl>.md` (`--- PAGE n ---`).\n\n",
        "Root: `knowledge/`. Document descriptions: `DOCUMENTS.md`. Full texts: `sources/<file>.md` (`--- PAGE n ---`).\n\n",
    ));
    s.push_str(&format!("| {} | {} | {} | {} |\n|---|---|---|---|\n", l.t("Kód", "Code"), l.t("Dokumentum", "Document"), l.t("Fájl", "File"), l.t("Oldalak", "Pages")));
    for d in docs {
        s.push_str(&format!("| {} | {} | `sources/{}` | {} |\n", d.code, md_escape_cell(d.title()), d.file, d.pages.len()));
    }
    let with_outline: Vec<&Doc> = docs.iter().filter(|d| !d.outline.is_empty()).collect();
    if !with_outline.is_empty() {
        s.push_str(&format!("\n## {}\n", l.t("Tartalomjegyzékek", "Tables of contents")));
        for d in with_outline {
            s.push_str(&format!("\n### {} – {}\n\n", d.code, d.title()));
            let min_level = d.outline.iter().map(|o| o.level).min().unwrap_or(1);
            for o in d.outline.iter().filter(|o| o.level <= min_level + 2).take(400) {
                let indent = "  ".repeat((o.level - min_level).clamp(0, 4) as usize);
                if o.page > 0 {
                    s.push_str(&format!("{indent}- {} — {} {}\n", o.title, d.code, o.page));
                } else {
                    s.push_str(&format!("{indent}- {}\n", o.title));
                }
            }
        }
    }
    if !terms.is_empty() {
        s.push_str(&format!(
            "\n## {}\n\n{}\n\n",
            l.t("Kulcsszó-mutató", "Keyword index"),
            l.t(
                "Automatikusan gyűjtött jellemző szavak; utánuk a dokumentum kódja és az oldalak, ahol a legtöbbször előfordulnak. Ragozott alakokra is keress (Grep).",
                "Characteristic words collected automatically, with the document code and the pages where they occur most. Search for inflected forms too (Grep)."
            )
        ));
        for t in terms {
            let mut by_doc: Vec<(String, Vec<i64>)> = vec![];
            for (code, page, _) in &t.places {
                match by_doc.iter_mut().find(|x| &x.0 == code) {
                    Some(x) => x.1.push(*page),
                    None => by_doc.push((code.clone(), vec![*page])),
                }
            }
            let places: Vec<String> = by_doc.into_iter().map(|(c, p)| format!("{c} {}", p.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(", "))).collect();
            s.push_str(&format!("- **{}** — {}\n", t.term, places.join(" · ")));
        }
    }
    s
}

/// The description in the frontmatter: what the skill knows and when to use it.
pub fn auto_description(sk: &Skill, titles: &[String], keywords: &[String]) -> String {
    let hu = sk.language != "en";
    let docs = titles.iter().take(6).cloned().collect::<Vec<_>>().join("; ");
    let more = if titles.len() > 6 { if hu { format!(" és még {}", titles.len() - 6) } else { format!(" and {} more", titles.len() - 6) } } else { String::new() };
    let kw = keywords.iter().take(25).cloned().collect::<Vec<_>>().join(", ");
    let s = if hu {
        format!(
            "{} – tudásbázis {} dokumentum teljes szövegéből ({docs}{more}). Használd, ha a kérdés ezeknek a dokumentumoknak a tartalmára vonatkozik, ha a felhasználó keres, idéz vagy hivatkozik bennük, vagy ezekről a témákról kérdez: {kw}.",
            sk.title,
            titles.len()
        )
    } else {
        format!(
            "{} – knowledge base built from the full text of {} documents ({docs}{more}). Use it when a question is about the content of these documents, when the user wants to search, quote or cite them, or asks about these topics: {kw}.",
            sk.title,
            titles.len()
        )
    };
    cut(&s.replace("(): ", ": ").replace(": .", "."), 1000)
}

fn skill_md(l: &L, sk: &Skill, docs: &[Doc], description: &str, root: &str, any_ocr: bool) -> String {
    let mut s = String::new();
    s.push_str("---\n");
    s.push_str(&format!("name: {}\n", sk.name));
    s.push_str(&format!("description: {}\n", yaml_str(description)));
    s.push_str("---\n\n");
    s.push_str(&format!("# {}\n\n", sk.title));
    s.push_str(&format!(
        "{} {} {}, {} {}.\n\n",
        l.t("Ez a skill", "This skill holds"),
        docs.len(),
        l.t("dokumentum teljes szövegét tartalmazza", "documents in full text"),
        docs.iter().map(|d| d.pages.len()).sum::<usize>(),
        l.t("oldal", "pages")
    ));
    s.push_str(&format!("## {}\n\n", l.t("Hol", "Where")));
    s.push_str(&format!("{} `{root}`\n\n", l.t("Tudásbázis gyökér (a skill mappájához képest):", "Knowledge root (relative to this skill's folder):")));
    s.push_str(&format!("| {} | {} |\n|---|---|\n", l.t("Útvonal", "Path"), l.t("Tartalom", "Content")));
    s.push_str(&format!("| `index.md` | {} |\n", l.t("Dokumentumkódok, tartalomjegyzékek oldalszámmal, kulcsszó-mutató → dokumentum + oldal", "Document codes, tables of contents with pages, keyword index → document + page")));
    s.push_str(&format!("| `DOCUMENTS.md` | {} |\n", l.t("Mi van az egyes dokumentumokban, fejezetek, mikor melyiket használd", "What each document covers, chapters, when to use which")));
    s.push_str(&format!("| `sources/<fájl>.md` | {} |\n\n", l.t("Teljes szöveg, `--- PAGE n ---` jelölők = a dokumentum oldalai", "Full text, `--- PAGE n ---` markers = the document's pages")));
    s.push_str(&format!("## {}\n\n", l.t("Hogyan keress", "How to search")));
    s.push_str(l.t(
        "1. Nézd meg az `index.md`-t (kulcsszó-mutató, tartalomjegyzék) és a `DOCUMENTS.md`-t: melyik dokumentum, melyik oldal.\n",
        "1. Check `index.md` (keyword index, tables of contents) and `DOCUMENTS.md`: which document, which page.\n",
    ));
    s.push_str(l.t("2. Keress a teljes szövegben, több változattal (szinonima, ragozott alak, angol és magyar kifejezés):\n", "2. Search the full texts with several variants (synonyms, inflected forms, other languages):\n"));
    s.push_str(&format!(
        "   ```\n   Grep pattern=\"(?i)kifejez\" path=\"{root}sources\" output_mode=\"files_with_matches\"\n   Grep pattern=\"(?i)kifejez\" path=\"{root}sources/<fájl>.md\" output_mode=\"content\" -C 3\n   ```\n"
    ));
    s.push_str(l.t(
        "3. A találat oldalszámát a fölötte lévő legközelebbi `--- PAGE n ---` sor adja. Olvasd el az egész oldalt (és ha kell, a szomszédosakat) a `Read` eszközzel.\n",
        "3. The page of a hit is the nearest `--- PAGE n ---` line above it. Read the whole page (and its neighbours if needed) with `Read`.\n",
    ));
    if any_ocr {
        s.push_str(l.t(
            "4. Az OCR-rel olvasott oldalak (`<!-- OCR -->`) betűhibásak lehetnek: keress ékezet nélküli szótöredékre, és engedj meg cseréket (`[0O]`, `[1lI]`, `[5S]`, `[8B]`).\n",
            "4. Pages read with OCR (`<!-- OCR -->`) may have character errors: search for short fragments without accents and allow swaps (`[0O]`, `[1lI]`, `[5S]`, `[8B]`).\n",
        ));
    }
    s.push_str(&format!("\n## {}\n\n", l.t("Válaszadás", "Answering")));
    s.push_str(l.t(
        "- Csak arra támaszkodj, ami a dokumentumokban áll; ha valami nincs benne, mondd ki egyértelműen, ne találgass.\n- Minden állításnál add meg a forrást: **[dokumentum címe, N. oldal]**.\n- Pontos idézetnél szó szerint idézz a szövegből.\n- A felhasználó nyelvén válaszolj.\n",
        "- Rely only on what the documents say; if something is not in them, say so clearly instead of guessing.\n- Cite every claim: **[document title, p. N]**.\n- Quote verbatim when an exact wording matters.\n- Answer in the user's language.\n",
    ));
    s.push_str(&format!("\n## {}\n\n", l.t("Dokumentumok", "Documents")));
    for d in docs {
        let hint = if !d.d.use_when.trim().is_empty() {
            d.d.use_when.trim().replace('\n', " ")
        } else if !d.d.summary.trim().is_empty() {
            cut(&d.d.summary.trim().replace('\n', " "), 220)
        } else {
            String::new()
        };
        s.push_str(&format!("- **{}** [{}] — `sources/{}`, {} {}", d.title(), d.code, d.file, d.pages.len(), l.t("oldal", "pages")));
        if !hint.is_empty() {
            s.push_str(&format!(". {hint}"));
        }
        s.push('\n');
    }
    if !sk.instructions.trim().is_empty() {
        s.push_str(&format!("\n## {}\n\n{}\n", l.t("További utasítások", "Further instructions"), sk.instructions.trim()));
    }
    s
}

fn readme_md(l: &L, sk: &Skill, docs: &[Doc], description: &str) -> String {
    let mut s = format!("# {}\n\n{}\n\n", sk.title, description);
    s.push_str(&format!("## {}\n\n", l.t("Telepítés", "Installation")));
    s.push_str(&format!(
        "{}\n\n```\nclaude --plugin-dir \"<{}>/{}\"\n```\n\n",
        l.t("**Claude Code** – a mappa pluginként:", "**Claude Code** – the folder as a plugin:"),
        l.t("ez a mappa szülője", "parent of this folder"),
        sk.name
    ));
    s.push_str(&format!(
        "{} `{}.plugin` {}\n\n",
        l.t("vagy a", "or install the"),
        sk.name,
        l.t("fájl telepítése (Claude Desktop → Plugins).", "file (Claude Desktop → Plugins).")
    ));
    s.push_str(&format!(
        "{} `{}-skill.zip` {}\n\n",
        l.t("**Claude.ai / Claude Desktop** – Settings → Capabilities → Skills → Upload skill:", "**Claude.ai / Claude Desktop** – Settings → Capabilities → Skills → Upload skill:"),
        sk.name,
        l.t("(a mappa melletti fájl).", "(next to this folder).")
    ));
    s.push_str(&format!("## {}\n\n", l.t("Tartalom", "Contents")));
    for d in docs {
        s.push_str(&format!("- {} — {} {} ({})\n", d.title(), d.pages.len(), l.t("oldal", "pages"), kind_label(l, &d.d)));
    }
    s.push_str(&format!(
        "\n{} SkillStar {}.\n",
        l.t("Készült:", "Made with"),
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    ));
    s
}

fn plugin_json(sk: &Skill, description: &str, author: &str, keywords: &[String]) -> String {
    let mut v = serde_json::json!({
        "name": sk.name,
        "version": if sk.version.trim().is_empty() { "0.1.0" } else { sk.version.trim() },
        "description": cut(description, 500),
        "keywords": keywords,
    });
    if !author.trim().is_empty() {
        v["author"] = serde_json::json!({ "name": author.trim() });
    }
    serde_json::to_string_pretty(&v).unwrap_or_default() + "\n"
}

// ───────────────────────── build ─────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct BuildResult {
    pub dir: String,
    pub plugin: String,
    pub skill_zip: String,
    pub documents: usize,
    pub pages: usize,
    pub bytes: u64,
    pub terms: usize,
    pub warnings: Vec<String>,
}

/// The files of the package (path relative to the plugin root, content).
pub struct Package {
    pub name: String,
    pub files: Vec<(String, String)>,
    /// SKILL.md of the single-skill zip (knowledge root `knowledge/`)
    pub skill_zip_md: String,
    pub documents: usize,
    pub pages: usize,
    pub terms: usize,
    pub warnings: Vec<String>,
}

pub fn package(c: &Connection, skill_id: i64, default_author: &str) -> Result<Package, String> {
    let sk = db::skill(c, skill_id)?;
    let l = L { hu: sk.language != "en" };
    let mut warnings = vec![];
    let mut docs = vec![];
    let mut used = HashSet::new();
    for d in db::documents(c, skill_id)? {
        if !d.include {
            continue;
        }
        if d.status != "done" {
            warnings.push(format!("{}: {}", d.name, l.t("nincs feldolgozva, kimarad", "not processed, left out")));
            continue;
        }
        if d.stale {
            warnings.push(format!("{}: {}", d.name, l.t("a fájl változott a feldolgozás óta", "the file changed since it was processed")));
        }
        let pages = db::pages(c, d.id)?;
        if pages.iter().all(|p| p.text.trim().is_empty()) {
            warnings.push(format!("{}: {}", d.name, l.t("nincs benne szöveg, kimarad", "has no text, left out")));
            continue;
        }
        let outline = db::outline(c, d.id)?;
        let stem = Path::new(&d.name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut file = slugify(&stem, "document");
        if used.contains(&file) {
            let mut i = 2;
            while used.contains(&format!("{file}-{i}")) {
                i += 1;
            }
            file = format!("{file}-{i}");
        }
        used.insert(file.clone());
        let code = format!("D{}", docs.len() + 1);
        docs.push(Doc { d, pages, outline, file: format!("{file}.md"), code });
    }
    if docs.is_empty() {
        return Err(l.t("Nincs feldolgozott dokumentum a skillben – adj hozzá fájlokat és dolgozd fel őket.", "The skill has no processed documents – add files and process them.").to_string());
    }
    let terms = keyword_index(&docs, 400);
    let titles: Vec<String> = docs.iter().map(|d| d.title().to_string()).collect();
    let mut keywords: Vec<String> = sk.keywords.split([',', ';']).map(|k| k.trim().to_string()).filter(|k| !k.is_empty()).collect();
    let mut top: Vec<&Term> = terms.iter().collect();
    top.sort_by(|a, b| b.places.iter().map(|p| p.2).sum::<usize>().cmp(&a.places.iter().map(|p| p.2).sum::<usize>()));
    for t in top.iter().take(20) {
        if !keywords.iter().any(|k| k.eq_ignore_ascii_case(&t.term)) {
            keywords.push(t.term.clone());
        }
    }
    let description = if sk.description.trim().is_empty() { auto_description(&sk, &titles, &keywords) } else { cut(sk.description.trim(), 1024) };
    let author = if sk.author.trim().is_empty() { default_author } else { sk.author.trim() };
    let any_ocr = docs.iter().any(|d| d.d.ocr_pages > 0);

    let mut files = vec![
        (".claude-plugin/plugin.json".to_string(), plugin_json(&sk, &description, author, &keywords.iter().take(12).cloned().collect::<Vec<_>>())),
        ("README.md".to_string(), readme_md(&l, &sk, &docs, &description)),
        (format!("skills/{}/SKILL.md", sk.name), skill_md(&l, &sk, &docs, &description, "../../knowledge/", any_ocr)),
        ("knowledge/index.md".to_string(), index_md(&l, &sk, &docs, &terms)),
        ("knowledge/DOCUMENTS.md".to_string(), documents_md(&l, &sk, &docs, &terms)),
    ];
    for d in &docs {
        files.push((format!("knowledge/sources/{}", d.file), source_md(&l, d)));
    }
    Ok(Package {
        name: sk.name.clone(),
        skill_zip_md: skill_md(&l, &sk, &docs, &description, "knowledge/", any_ocr),
        files,
        documents: docs.len(),
        pages: docs.iter().map(|d| d.pages.len()).sum(),
        terms: terms.len(),
        warnings,
    })
}

fn zip_files(path: &Path, files: &[(String, &str)]) -> Result<u64, String> {
    let tmp = path.with_extension("part");
    {
        let f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut z = zip::ZipWriter::new(f);
        let opt = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).unix_permissions(0o644);
        for (name, text) in files {
            z.start_file(name.as_str(), opt).map_err(|e| e.to_string())?;
            z.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(std::fs::metadata(path).map(|m| m.len()).unwrap_or(0))
}

/// Writes the plugin folder and the two zips into `out_root`.
pub fn write(pkg: &Package, out_root: &Path) -> Result<BuildResult, String> {
    let dir = out_root.join(&pkg.name);
    if dir.exists() {
        let ours = dir.join(MARKER).is_file();
        let empty = std::fs::read_dir(&dir).map(|mut r| r.next().is_none()).unwrap_or(false);
        if !ours && !empty {
            return Err(format!(
                "{} {}",
                dir.display(),
                crate::settings::tr(
                    "már létezik, és nem a SkillStar hozta létre – válassz másik kimeneti mappát vagy skill-nevet.",
                    "already exists and was not made by SkillStar – pick another output folder or skill name."
                )
            ));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut bytes = 0u64;
    for (rel, text) in &pkg.files {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, text).map_err(|e| format!("{}: {e}", p.display()))?;
        bytes += text.len() as u64;
    }
    std::fs::write(dir.join(MARKER), "Made by SkillStar. This folder is rebuilt on every build.\n").map_err(|e| e.to_string())?;

    let plugin_path = out_root.join(format!("{}.plugin", pkg.name));
    let entries: Vec<(String, &str)> = pkg.files.iter().map(|(n, t)| (n.clone(), t.as_str())).collect();
    zip_files(&plugin_path, &entries)?;

    let skill_path = out_root.join(format!("{}-skill.zip", pkg.name));
    let mut entries: Vec<(String, &str)> = vec![(format!("{}/SKILL.md", pkg.name), pkg.skill_zip_md.as_str())];
    for (n, t) in &pkg.files {
        if n.starts_with("knowledge/") {
            entries.push((format!("{}/{n}", pkg.name), t.as_str()));
        }
    }
    zip_files(&skill_path, &entries)?;

    Ok(BuildResult {
        dir: dir.to_string_lossy().into_owned(),
        plugin: plugin_path.to_string_lossy().into_owned(),
        skill_zip: skill_path.to_string_lossy().into_owned(),
        documents: pkg.documents,
        pages: pkg.pages,
        bytes,
        terms: pkg.terms,
        warnings: pkg.warnings.clone(),
    })
}

pub fn out_root(setting: &str, skill_override: &str, default_root: &Path) -> PathBuf {
    let o = skill_override.trim();
    if !o.is_empty() {
        return PathBuf::from(o);
    }
    let s = setting.trim();
    if s.is_empty() {
        default_root.to_path_buf()
    } else {
        PathBuf::from(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Extracted, OutlineEntry, PageText};
    use std::io::Read;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Árvíztűrő Tükörfúrógép 2.0!", "x"), "arvizturo-tukorfurogep-2-0");
        assert_eq!(slugify("Claude kézikönyv", "x"), "kezikonyv");
        assert_eq!(slugify("***", "fallback"), "fallback");
        assert!(slugify(&"hosszú szó ".repeat(20), "x").len() <= 64);
    }

    #[test]
    fn yaml_quoting() {
        assert_eq!(yaml_str("a \"b\"\n c\\d"), "\"a \\\"b\\\" c\\\\d\"");
    }

    #[test]
    fn builds_a_package() {
        let mut c = db::open_memory();
        let s = db::create_skill(&c, "Teszt tudás", "Teszt Elek", "hu").unwrap();
        let dir = std::env::temp_dir().join(format!("skillstar-build-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut ids = vec![];
        for name in ["Első kézikönyv.pdf", "elso-kezikonyv.docx", "jegyzet.txt"] {
            let f = dir.join(name);
            std::fs::write(&f, "x").unwrap();
            ids.push(db::add_document(&c, s, &f).unwrap().unwrap());
        }
        let page = |no, t: &str, ocr| PageText { no, text: t.to_string(), ocr };
        let x = Extracted {
            pages: vec![
                page(1, "A raszter megszakítás a képernyő adott sorában fut. raszter raszter", false),
                page(2, "Sprite multiplexer: sok sprite raszter alapján.", true),
            ],
            outline: vec![OutlineEntry { level: 1, title: "Raszter".into(), page: 1 }, OutlineEntry { level: 2, title: "Sprite".into(), page: 2 }],
            title: Some("Kézikönyv".into()),
        };
        db::store_result(&mut c, ids[0], &x, "a", 1, 1).unwrap();
        db::store_result(&mut c, ids[1], &Extracted { pages: vec![page(1, "Docx szöveg a raszterről.", false)], ..Default::default() }, "b", 1, 1).unwrap();
        // ids[2] stays unprocessed → warning
        let pkg = package(&c, s, "").unwrap();
        assert_eq!(pkg.documents, 2);
        // the unprocessed one is left out; the other two "changed" (fake mtime of the test)
        assert_eq!(pkg.warnings.iter().filter(|w| w.contains("nincs feldolgozva")).count(), 1);
        let names: Vec<&str> = pkg.files.iter().map(|f| f.0.as_str()).collect();
        assert!(names.contains(&"skills/teszt-tudas/SKILL.md"));
        assert!(names.contains(&"knowledge/sources/elso-kezikonyv.md"));
        assert!(names.contains(&"knowledge/sources/elso-kezikonyv-2.md"));
        let skill = &pkg.files.iter().find(|f| f.0.ends_with("SKILL.md")).unwrap().1;
        assert!(skill.starts_with("---\nname: teszt-tudas\ndescription: \""));
        assert!(skill.contains("../../knowledge/"));
        assert!(skill.contains("<!-- OCR -->") || skill.contains("OCR"));
        assert!(pkg.skill_zip_md.contains("`knowledge/`"));
        let src = &pkg.files.iter().find(|f| f.0 == "knowledge/sources/elso-kezikonyv.md").unwrap().1;
        assert!(src.contains("--- PAGE 2 ---\n<!-- OCR -->\nSprite multiplexer"));
        let plugin: serde_json::Value = serde_json::from_str(&pkg.files[0].1).unwrap();
        assert_eq!(plugin["name"], "teszt-tudas");
        assert_eq!(plugin["author"]["name"], "Teszt Elek");

        let out = dir.join("out");
        let r = write(&pkg, &out).unwrap();
        assert!(Path::new(&r.dir).join(".claude-plugin/plugin.json").is_file());
        // a rebuild replaces our own folder
        write(&pkg, &out).unwrap();
        let mut z = zip::ZipArchive::new(std::fs::File::open(&r.skill_zip).unwrap()).unwrap();
        let mut md = String::new();
        z.by_name("teszt-tudas/SKILL.md").unwrap().read_to_string(&mut md).unwrap();
        assert!(md.contains("name: teszt-tudas"));
        assert!(z.by_name("teszt-tudas/knowledge/index.md").is_ok());
        let z = zip::ZipArchive::new(std::fs::File::open(&r.plugin).unwrap()).unwrap();
        assert!(z.file_names().any(|n| n == ".claude-plugin/plugin.json"));
        // a foreign folder with the same name is never deleted
        let foreign = dir.join("out2").join("teszt-tudas");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("mine.txt"), "keep").unwrap();
        assert!(write(&pkg, &dir.join("out2")).is_err());
        assert!(foreign.join("mine.txt").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
