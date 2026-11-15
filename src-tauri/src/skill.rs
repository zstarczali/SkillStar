//! Builds the skill packages from the processed documents. Every chosen
//! format goes into its own folder of `<out>/<name>/`:
//!
//! ```text
//! claude/<name>/          Claude Code plugin (.claude-plugin/plugin.json, skills/<name>/SKILL.md, knowledge/)
//! claude/<name>.plugin    the same folder as one zip
//! claude/<name>-skill.zip single skill for claude.ai / Claude Desktop
//! agents/<name>/          open Agent Skills format (Codex, Gemini CLI, Copilot, Cursor…) + zip
//! chatgpt/                Custom GPT / Project: instructions.md + ≤ 20 knowledge files
//! gemini/                 Gem: instructions.md + ≤ 10 knowledge files
//! deepseek/               system prompt + text cut into context-sized parts
//! rag/                    chunks.jsonl + documents.json + system prompt for any LLM / vector store
//! ```
//! knowledge/: index.md (codes, tables of contents, keyword index), DOCUMENTS.md,
//! sources/<doc>.md (full text, `--- PAGE n ---` markers).

use crate::db::{self, Document, OutlineItem, Page, Skill};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Marker file: only folders that SkillStar made are ever emptied.
const MARKER: &str = ".skillstar";

/// ASCII form of an accented letter (None: keep the character).
fn fold_char(c: char) -> Option<&'static str> {
    Some(match c {
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
        _ => return None,
    })
}

/// Lower case without accents (search matching).
pub fn fold(text: &str) -> String {
    let mut s = String::with_capacity(text.len());
    for c in text.chars().flat_map(|c| c.to_lowercase()) {
        match fold_char(c) {
            Some(f) => s.push_str(f),
            None => s.push(c),
        }
    }
    s
}

pub fn is_stop(word: &str) -> bool {
    STOP.contains(&word)
}

/// kebab-case ASCII name (accents folded), at most 64 characters. Words that
/// skill names may not contain ("claude", "anthropic") are dropped.
pub fn slugify(input: &str, fallback: &str) -> String {
    let mut s = String::new();
    for c in input.chars().flat_map(|c| c.to_lowercase()) {
        let f = match fold_char(c) {
            Some(f) => f,
            None if c.is_ascii_alphanumeric() => {
                s.push(c);
                continue;
            }
            None => "-",
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
    s.push_str(&toc_md(l, docs, 400));
    s.push_str(&keywords_md(l, terms, true));
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

fn skill_md(l: &L, sk: &Skill, docs: &[Doc], description: &str, root: &str, any_ocr: bool, generic: bool) -> String {
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
    if generic {
        s.push_str(&format!("   ```\n   rg -i -l \"kifejez\" {root}sources\n   rg -i -C 3 \"kifejez\" {root}sources/<fájl>.md\n   ```\n"));
    } else {
        s.push_str(&format!(
            "   ```\n   Grep pattern=\"(?i)kifejez\" path=\"{root}sources\" output_mode=\"files_with_matches\"\n   Grep pattern=\"(?i)kifejez\" path=\"{root}sources/<fájl>.md\" output_mode=\"content\" -C 3\n   ```\n"
        ));
    }
    s.push_str(if generic {
        l.t(
            "3. A találat oldalszámát a fölötte lévő legközelebbi `--- PAGE n ---` sor adja. Olvasd el az egész oldalt (és ha kell, a szomszédosakat).\n",
            "3. The page of a hit is the nearest `--- PAGE n ---` line above it. Read the whole page (and its neighbours if needed).\n",
        )
    } else {
        l.t(
            "3. A találat oldalszámát a fölötte lévő legközelebbi `--- PAGE n ---` sor adja. Olvasd el az egész oldalt (és ha kell, a szomszédosakat) a `Read` eszközzel.\n",
            "3. The page of a hit is the nearest `--- PAGE n ---` line above it. Read the whole page (and its neighbours if needed) with `Read`.\n",
        )
    });
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
        "{}\n\n```\nclaude --plugin-dir \"<{}>\"\n```\n\n",
        l.t("**Claude Code** – a mappa pluginként:", "**Claude Code** – the folder as a plugin:"),
        l.t("ennek a mappának az útvonala", "path of this folder"),
    ));
    s.push_str(&format!(
        "{} `{}.plugin` {}\n\n",
        l.t("vagy a mappa melletti", "or install the"),
        sk.name,
        l.t("fájl telepítése (Claude Desktop → Plugins).", "file next to this folder (Claude Desktop → Plugins).")
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
    s.push_str(&made_with(l));
    s
}

fn made_with(l: &L) -> String {
    format!("\n{} SkillStar {}.\n", l.t("Készült:", "Made with"), chrono::Local::now().format("%Y-%m-%d %H:%M"))
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

// ───────────────────────── chat assistants (ChatGPT, Gemini, DeepSeek, RAG) ─────────────────────────

/// Table of contents of every document (outline with pages).
fn toc_md(l: &L, docs: &[Doc], max_per_doc: usize) -> String {
    let mut s = String::new();
    let with_outline: Vec<&Doc> = docs.iter().filter(|d| !d.outline.is_empty()).collect();
    if with_outline.is_empty() {
        return s;
    }
    s.push_str(&format!("\n## {}\n", l.t("Tartalomjegyzékek", "Tables of contents")));
    for d in with_outline {
        s.push_str(&format!("\n### {} – {}\n\n", d.code, d.title()));
        let min_level = d.outline.iter().map(|o| o.level).min().unwrap_or(1);
        for o in d.outline.iter().filter(|o| o.level <= min_level + 2).take(max_per_doc) {
            let indent = "  ".repeat((o.level - min_level).clamp(0, 4) as usize);
            if o.page > 0 {
                s.push_str(&format!("{indent}- {} — {} {}\n", o.title, d.code, o.page));
            } else {
                s.push_str(&format!("{indent}- {}\n", o.title));
            }
        }
    }
    s
}

/// Keyword → document code + pages.
fn keywords_md(l: &L, terms: &[Term], hint_grep: bool) -> String {
    if terms.is_empty() {
        return String::new();
    }
    let mut s = format!(
        "\n## {}\n\n{}\n\n",
        l.t("Kulcsszó-mutató", "Keyword index"),
        if hint_grep {
            l.t(
                "Automatikusan gyűjtött jellemző szavak; utánuk a dokumentum kódja és az oldalak, ahol a legtöbbször előfordulnak. Ragozott alakokra is keress (Grep).",
                "Characteristic words collected automatically, with the document code and the pages where they occur most. Search for inflected forms too (Grep).",
            )
        } else {
            l.t(
                "Automatikusan gyűjtött jellemző szavak; utánuk a dokumentum kódja és az oldalak, ahol a legtöbbször előfordulnak.",
                "Characteristic words collected automatically, with the document code and the pages where they occur most.",
            )
        }
    );
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
    s
}

/// The catalog file of the chat formats: what is where (`loc[i]` = the knowledge file(s) of `docs[i]`).
fn catalog_md(l: &L, sk: &Skill, docs: &[Doc], terms: &[Term], loc: &[String]) -> String {
    let mut s = format!("# {} – {}\n\n", sk.title, l.t("katalógus", "catalog"));
    s.push_str(l.t(
        "A többi tudásfájl a dokumentumok teljes szövege, `--- PAGE n ---` jelölőkkel (egy szakasz oldalszáma a fölötte lévő legközelebbi jelölő). Itt látod, melyik dokumentum melyik fájlban van, miről szól, és a tartalomjegyzékeket oldalszámmal.\n",
        "The other knowledge files hold the full text of the documents, with `--- PAGE n ---` markers (the page of a passage is the nearest marker above it). This file tells which document is in which file, what it covers, and the tables of contents with pages.\n",
    ));
    for (i, d) in docs.iter().enumerate() {
        s.push_str(&format!("\n---\n\n## {} – {}\n", d.code, d.title()));
        s.push_str(&format!("- **{}:** {}\n", l.t("Fájl", "File"), loc[i]));
        s.push_str(&format!("- **{}:** `{}`, {} {}, {}\n", l.t("Eredeti", "Original"), d.d.name, d.pages.len(), l.t("oldal", "pages"), kind_label(l, &d.d)));
        if !d.d.summary.trim().is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Összefoglaló", "Summary"), d.d.summary.trim().replace('\n', " ")));
        }
        if !d.d.use_when.trim().is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Használd ha", "Use when"), d.d.use_when.trim().replace('\n', " ")));
        }
        let kw = doc_terms(d, terms, 15);
        if !kw.is_empty() {
            s.push_str(&format!("- **{}:** {}\n", l.t("Kulcsszavak", "Keywords"), kw.join(", ")));
        }
    }
    s.push_str(&toc_md(l, docs, 300));
    s.push_str(&keywords_md(l, terms, false));
    s
}

/// Instructions of a chat assistant (Custom GPT, Gem, DeepSeek system prompt), at most `max` characters.
fn chat_instructions(l: &L, sk: &Skill, docs: &[Doc], description: &str, loc: &[String], any_ocr: bool, max: usize) -> (String, bool) {
    let pages: usize = docs.iter().map(|d| d.pages.len()).sum();
    let build = |hints: bool, limit: usize| -> String {
        let mut s = format!("# {}\n\n", sk.title);
        s.push_str(&format!(
            "{} {} {} ({} {}). {}\n\n",
            l.t("Szakértő asszisztens vagy, aki a csatolt tudásfájlokból válaszol:", "You are an expert assistant who answers from the attached knowledge files:"),
            docs.len(),
            l.t("dokumentum teljes szövege", "documents in full text"),
            pages,
            l.t("oldal", "pages"),
            cut(description, 400)
        ));
        s.push_str(&format!("## {}\n\n", l.t("Tudásfájlok", "Knowledge files")));
        s.push_str(l.t(
            "- `00-catalog.md`: melyik dokumentum melyik fájlban van, mi van benne, tartalomjegyzékek oldalszámmal, kulcsszó-mutató. Ezzel kezdd.\n- A többi fájl: teljes szöveg, `--- PAGE n ---` jelölőkkel; egy szakasz oldalszáma a fölötte lévő legközelebbi jelölő.\n",
            "- `00-catalog.md`: which document is in which file, what it covers, tables of contents with pages, keyword index. Start there.\n- The other files: full text with `--- PAGE n ---` markers; the page of a passage is the nearest marker above it.\n",
        ));
        s.push_str(&format!("\n## {}\n\n", l.t("Válaszadás", "Answering")));
        s.push_str(l.t(
            "- Válasz előtt mindig keress a tudásfájlokban, több változattal (szinonima, ragozott alak, más nyelvű kifejezés).\n- Csak arra támaszkodj, ami a dokumentumokban áll; ha nincs benne, mondd ki, ne találgass.\n- Minden állításnál add meg a forrást: **[dokumentum címe, N. oldal]**.\n- Pontos megfogalmazásnál idézz szó szerint.\n- A felhasználó nyelvén válaszolj.\n",
            "- Always search the knowledge files before answering, with several variants (synonyms, inflected forms, other languages).\n- Rely only on what the documents say; if something is not in them, say so instead of guessing.\n- Cite every claim: **[document title, p. N]**.\n- Quote verbatim when the exact wording matters.\n- Answer in the user's language.\n",
        ));
        if any_ocr {
            s.push_str(l.t(
                "- Egyes oldalak OCR-rel készültek (`<!-- OCR -->`): lehetnek bennük betűhibák (0/O, 1/l, ékezetek); számoknál jelezd, ha bizonytalan.\n",
                "- Some pages were read with OCR (`<!-- OCR -->`) and may contain character errors (0/O, 1/l, accents); flag uncertain numbers.\n",
            ));
        }
        let mut tail = String::new();
        if !sk.instructions.trim().is_empty() {
            tail = format!("\n## {}\n\n{}\n", l.t("További utasítások", "Further instructions"), sk.instructions.trim());
        }
        s.push_str(&format!("\n## {}\n\n", l.t("Dokumentumok", "Documents")));
        for (i, d) in docs.iter().enumerate() {
            let mut line = format!("- **{}** [{}] — {}, {} {}", d.title(), d.code, loc[i], d.pages.len(), l.t("oldal", "pages"));
            if hints {
                let hint = if !d.d.use_when.trim().is_empty() { d.d.use_when.trim().replace('\n', " ") } else { cut(&d.d.summary.trim().replace('\n', " "), 160) };
                if !hint.is_empty() {
                    line.push_str(&format!(". {hint}"));
                }
            }
            line.push('\n');
            if s.len() + line.len() + tail.len() + 80 > limit {
                s.push_str(&format!("- … {} {} (`00-catalog.md`)\n", docs.len() - i, l.t("további dokumentum", "more documents")));
                break;
            }
            s.push_str(&line);
        }
        s.push_str(&tail);
        s
    };
    let full = build(true, usize::MAX);
    if full.chars().count() <= max {
        return (full, false);
    }
    let short = build(false, usize::MAX);
    if short.chars().count() <= max {
        return (short, false);
    }
    // byte budget is a safe lower bound of the character count
    let s = build(false, max);
    let s = if s.chars().count() > max { s.chars().take(max).collect() } else { s };
    (s, true)
}

/// Splits the documents into at most `max_files` groups in order, of roughly equal size.
fn group_docs(sizes: &[usize], max_files: usize) -> Vec<Vec<usize>> {
    if sizes.len() <= max_files {
        return (0..sizes.len()).map(|i| vec![i]).collect();
    }
    let total: usize = sizes.iter().sum();
    let cap = total.div_ceil(max_files.max(1));
    let mut out: Vec<Vec<usize>> = vec![];
    let mut cur: Vec<usize> = vec![];
    let mut cur_size = 0;
    for (i, &n) in sizes.iter().enumerate() {
        if !cur.is_empty() && cur_size + n > cap && out.len() + 1 < max_files {
            out.push(std::mem::take(&mut cur));
            cur_size = 0;
        }
        cur.push(i);
        cur_size += n;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// One part of a document (pages `pages`), for chats with a small context.
fn part_md(l: &L, d: &Doc, pages: &[Page], k: usize, n: usize) -> String {
    let first = pages.first().map(|p| p.no).unwrap_or(1);
    let last = pages.last().map(|p| p.no).unwrap_or(first);
    let mut s = if n > 1 {
        format!("# {} [{}] — {} {first}–{last} ({k}/{n})\n\n", d.title(), d.code, l.t("oldal", "pages"))
    } else {
        format!("# {} [{}]\n\n", d.title(), d.code)
    };
    s.push_str(&format!("> {}: `{}` · {}: {}\n", l.t("Eredeti fájl", "Original file"), d.d.name, l.t("Szöveg", "Text"), kind_label(l, &d.d)));
    s.push_str(&format!("> `--- PAGE n ---` = {}\n", pages_meaning(l, &d.d)));
    for p in pages {
        s.push_str(&format!("\n--- PAGE {} ---\n", p.no));
        if p.ocr {
            s.push_str("<!-- OCR -->\n");
        }
        s.push_str(p.text.trim_end());
        s.push('\n');
    }
    s
}

/// Pages of a document cut into runs of at most `max` characters.
fn split_pages(pages: &[Page], max: usize) -> Vec<&[Page]> {
    let mut out = vec![];
    let mut start = 0;
    let mut size = 0;
    for (i, p) in pages.iter().enumerate() {
        let n = p.text.len() + 20;
        if i > start && size + n > max {
            out.push(&pages[start..i]);
            start = i;
            size = 0;
        }
        size += n;
    }
    if start < pages.len() {
        out.push(&pages[start..]);
    }
    out
}

/// RAG chunks of a page: paragraphs packed up to `max` characters; longer paragraphs cut at spaces.
pub fn chunk_page(text: &str, max: usize) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let push = |cur: &mut String, out: &mut Vec<String>| {
        let t = cur.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
        cur.clear();
    };
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if cur.chars().count() + para.chars().count() + 2 > max {
            push(&mut cur, &mut out);
        }
        if para.chars().count() > max {
            let mut piece = String::new();
            for w in para.split_inclusive(char::is_whitespace) {
                if piece.chars().count() + w.chars().count() > max {
                    push(&mut piece, &mut out);
                }
                piece.push_str(w);
            }
            cur = piece;
            continue;
        }
        if !cur.is_empty() {
            cur.push_str("\n\n");
        }
        cur.push_str(para);
    }
    push(&mut cur, &mut out);
    out
}

fn rag_prompt(l: &L, sk: &Skill, description: &str) -> String {
    let mut s = format!("# {} – {}\n\n", sk.title, l.t("RAG rendszerprompt", "RAG system prompt"));
    s.push_str(l.t(
        "Ezt a szöveget add meg rendszerpromptként (system message); a felhasználói üzenet a lent látható sablon, a `chunks.jsonl`-ből keresett részletekkel.\n\n```text\n",
        "Use this text as the system prompt (system message); the user message is the template below, filled with the passages retrieved from `chunks.jsonl`.\n\n```text\n",
    ));
    s.push_str(&format!(
        "{} {}\n\n{}\n",
        l.t("Kérdésekre válaszolsz ebből a tudásbázisból:", "You answer questions from this knowledge base:"),
        cut(description, 500),
        l.t(
            "A felhasználói üzenetben számozott részleteket kapsz (<passage n=\"…\" document=\"…\" page=\"…\">). Csak ezekre támaszkodj. Minden állítás után jelöld a forrást a részlet számával: [1], [2]. Ha a részletekben nincs benne a válasz, mondd ki egyértelműen. Az OCR-rel olvasott szövegben betűhibák lehetnek. A felhasználó nyelvén válaszolj.",
            "The user message holds numbered passages (<passage n=\"…\" document=\"…\" page=\"…\">). Rely only on them. Cite every claim with the passage number: [1], [2]. If the passages do not contain the answer, say so plainly. Text read with OCR may contain character errors. Answer in the user's language.",
        )
    ));
    if !sk.instructions.trim().is_empty() {
        s.push_str(&format!("\n{}\n", sk.instructions.trim()));
    }
    s.push_str("```\n\n");
    s.push_str(&format!("## {}\n\n```text\n<passages>\n<passage n=\"1\" document=\"{{title}}\" page=\"{{page}}\">\n{{text}}\n</passage>\n…\n</passages>\n\nQuestion: {{question}}\n```\n", l.t("Felhasználói üzenet sablon", "User message template")));
    s
}

fn rag_readme(l: &L, sk: &Skill, chunks: usize) -> String {
    let mut s = format!("# {} – RAG\n\n", sk.title);
    s.push_str(&format!(
        "{}\n\n",
        l.t(
            "Bármely LLM-hez (DeepSeek API, OpenAI, Mistral, Ollama, LM Studio…) és bármely vektor- vagy szöveges keresőhöz (OpenAI vector store, Chroma, Qdrant, pgvector, Elasticsearch, LangChain, LlamaIndex…).",
            "For any LLM (DeepSeek API, OpenAI, Mistral, Ollama, LM Studio…) and any vector or text search (OpenAI vector stores, Chroma, Qdrant, pgvector, Elasticsearch, LangChain, LlamaIndex…).",
        )
    ));
    s.push_str(&format!("| {} | {} |\n|---|---|\n", l.t("Fájl", "File"), l.t("Tartalom", "Content")));
    s.push_str(&format!(
        "| `chunks.jsonl` | {} {} — `{{\"id\", \"doc\", \"title\", \"file\", \"page\", \"ocr\", \"text\"}}` |\n",
        chunks,
        l.t("részlet, soronként egy JSON (oldalhatáron vágva, ≤ ~2000 karakter)", "chunks, one JSON per line (cut at page boundaries, ≤ ~2000 characters)")
    ));
    s.push_str(&format!("| `documents.json` | {} |\n", l.t("a dokumentumok adatai (kód, cím, eredeti fájl, oldalak, összefoglaló)", "the documents (code, title, original file, pages, summary)")));
    s.push_str(&format!("| `system-prompt.md` | {} |\n\n", l.t("rendszerprompt és üzenetsablon forrásmegjelöléssel", "system prompt and message template with citations")));
    s.push_str(&format!("## {}\n\n", l.t("Lépések", "Steps")));
    s.push_str(l.t(
        "1. Indexeld a `chunks.jsonl` `text` mezőjét (embedding vagy BM25), a többi mező legyen metaadat.\n2. Kérdésnél keresd ki a 6–12 legjobb részletet.\n3. Küldd el a modellnek a `system-prompt.md` szerint; a `[n]` hivatkozások a részletek `title` és `page` mezőjére mutatnak.\n\nDeepSeek API: OpenAI-kompatibilis (`https://api.deepseek.com`), a rendszerprompt a `system` üzenet.\n",
        "1. Index the `text` field of `chunks.jsonl` (embeddings or BM25); keep the other fields as metadata.\n2. For a question, retrieve the 6–12 best chunks.\n3. Send them to the model as in `system-prompt.md`; `[n]` citations point to the `title` and `page` of the chunks.\n\nDeepSeek API: OpenAI-compatible (`https://api.deepseek.com`); the system prompt goes into the `system` message.\n",
    ));
    s.push_str(&made_with(l));
    s
}

fn chat_readme(l: &L, sk: &Skill, kind: &str, description: &str, files: usize) -> String {
    let mut s = format!("# {} – {}\n\n", sk.title, match kind {
        "chatgpt" => "ChatGPT (Custom GPT / Project)",
        "gemini" => "Gemini Gem",
        _ => "DeepSeek",
    });
    match kind {
        "chatgpt" => {
            s.push_str(l.t(
                "### Custom GPT\n\n1. ChatGPT → **GPTs → Create** → **Configure**.\n2. **Name**: a skill neve, **Description**: lent.\n3. **Instructions**: másold be az `instructions.md` tartalmát (legfeljebb 8000 karakter).\n4. **Knowledge → Upload files**: a `knowledge/` mappa összes fájlja (legfeljebb 20).\n5. **Create / Update**.\n\n### ChatGPT Project\n\nÚj projekt → **Instructions**: `instructions.md`, **Files**: a `knowledge/` fájljai.\n",
                "### Custom GPT\n\n1. ChatGPT → **GPTs → Create** → **Configure**.\n2. **Name**: the skill name, **Description**: below.\n3. **Instructions**: paste `instructions.md` (at most 8000 characters).\n4. **Knowledge → Upload files**: every file of `knowledge/` (at most 20).\n5. **Create / Update**.\n\n### ChatGPT Project\n\nNew project → **Instructions**: `instructions.md`, **Files**: the files of `knowledge/`.\n",
            ));
        }
        "gemini" => {
            s.push_str(l.t(
                "1. gemini.google.com → **Gems** → **New Gem**.\n2. **Name**: a skill neve; **Instructions**: másold be az `instructions.md` tartalmát.\n3. **Knowledge**: töltsd fel a `knowledge/` mappa fájljait (legfeljebb 10).\n4. **Save**.\n",
                "1. gemini.google.com → **Gems** → **New Gem**.\n2. **Name**: the skill name; **Instructions**: paste `instructions.md`.\n3. **Knowledge**: upload the files of `knowledge/` (at most 10).\n4. **Save**.\n",
            ));
        }
        _ => {
            s.push_str(l.t(
                "A DeepSeek csevegőben nincs tartós asszisztens és a kontextus korlátos (~128K token), ezért a szöveg kisebb részekre van vágva.\n\n### chat.deepseek.com\n\n1. Új csevegés: illeszd be a `system-prompt.md` tartalmát első üzenetként.\n2. Csatold a `knowledge/00-catalog.md`-t és csak azokat a részeket, amelyek a kérdéshez kellenek (a katalógus megmondja, melyik dokumentum és oldal melyik fájlban van).\n3. Kérdezz.\n\n### DeepSeek API\n\nA `system-prompt.md` a `system` üzenet. Nagy tudásbázishoz használd a **RAG** formátumot (`../rag/chunks.jsonl`), és csak a keresett részleteket küldd el.\n",
                "The DeepSeek chat has no persistent assistants and a limited context (~128K tokens), so the text is cut into smaller parts.\n\n### chat.deepseek.com\n\n1. New chat: paste `system-prompt.md` as the first message.\n2. Attach `knowledge/00-catalog.md` and only the parts the question needs (the catalog tells which document and pages are in which file).\n3. Ask.\n\n### DeepSeek API\n\n`system-prompt.md` is the `system` message. For a large knowledge base use the **RAG** format (`../rag/chunks.jsonl`) and send only the retrieved passages.\n",
            ));
        }
    }
    s.push_str(&format!("\n**Description:** {}\n\n{}: {}\n", cut(description, 300), l.t("Tudásfájlok", "Knowledge files"), files));
    s.push_str(&made_with(l));
    s
}

fn agents_readme(l: &L, name: &str) -> String {
    let mut s = String::from("# Agent Skills\n\n");
    s.push_str(l.t(
        "A `SKILL.md` + `knowledge/` mappa a nyílt Agent Skills formátum: Claude-on kívül más kódoló ágensek is betöltik (pl. OpenAI Codex, Gemini CLI, GitHub Copilot / VS Code, Cursor). A pontos skill-mappát az eszköz leírásában nézd meg; jellemzően:\n\n",
        "`SKILL.md` + `knowledge/` is the open Agent Skills format: besides Claude, other coding agents load it too (e.g. OpenAI Codex, Gemini CLI, GitHub Copilot / VS Code, Cursor). Check your tool's documentation for its skills folder; typically:\n\n",
    ));
    s.push_str(&format!(
        "| {} | {} |\n|---|---|\n| OpenAI Codex | `~/.codex/skills/{name}/` |\n| Gemini CLI | `~/.gemini/skills/{name}/` |\n| GitHub Copilot / VS Code | `.github/skills/{name}/` {} |\n| Cursor | `.cursor/skills/{name}/` |\n\n",
        l.t("Eszköz", "Tool"),
        l.t("Mappa", "Folder"),
        l.t("(a projektben)", "(in the project)")
    ));
    s.push_str(&format!("{} `{name}.zip`.\n", l.t("Másold oda a mappát, vagy csomagold ki oda a", "Copy the folder there, or extract")));
    s.push_str(&made_with(l));
    s
}

fn root_readme(l: &L, sk: &Skill, formats: &[&str]) -> String {
    let mut s = format!("# {}\n\n{}\n\n", sk.title, l.t("A SkillStar által készített csomagok:", "Packages made by SkillStar:"));
    for f in formats {
        let (dir, what) = match *f {
            "claude" => ("claude/", l.t("Claude Code plugin, .plugin fájl és claude.ai skill ZIP", "Claude Code plugin, .plugin file and claude.ai skill ZIP")),
            "agents" => ("agents/", l.t("Agent Skills (Codex, Gemini CLI, Copilot, Cursor…)", "Agent Skills (Codex, Gemini CLI, Copilot, Cursor…)")),
            "chatgpt" => ("chatgpt/", l.t("ChatGPT Custom GPT / Project: utasítás + tudásfájlok", "ChatGPT Custom GPT / Project: instructions + knowledge files")),
            "gemini" => ("gemini/", l.t("Gemini Gem: utasítás + tudásfájlok", "Gemini Gem: instructions + knowledge files")),
            "deepseek" => ("deepseek/", l.t("DeepSeek: rendszerprompt + kisebb részekre vágott szöveg", "DeepSeek: system prompt + text cut into smaller parts")),
            _ => ("rag/", l.t("RAG: chunks.jsonl bármely LLM-hez és vektoradatbázishoz", "RAG: chunks.jsonl for any LLM and vector database")),
        };
        s.push_str(&format!("- `{dir}` — {what}\n"));
    }
    s.push_str(&format!("\n{}\n", l.t("Minden mappában README.md írja le a használatát.", "Each folder has a README.md on how to use it.")));
    s.push_str(&made_with(l));
    s
}

// ───────────────────────── build ─────────────────────────

/// Package formats, in the order they are built.
pub const FORMATS: &[&str] = &["claude", "agents", "chatgpt", "gemini", "deepseek", "rag"];

pub fn format_list(s: &str) -> Vec<&'static str> {
    let want: Vec<String> = s.split([',', ';', ' ']).map(|x| x.trim().to_ascii_lowercase()).collect();
    let v: Vec<&'static str> = FORMATS.iter().copied().filter(|f| want.iter().any(|w| w == f)).collect();
    if v.is_empty() {
        vec!["claude"]
    } else {
        v
    }
}

pub fn clean_formats(s: &str) -> String {
    format_list(s).join(",")
}

/// ChatGPT: 20 knowledge files, 8000-character instructions; Gemini Gem: 10 files.
const GPT_FILES: usize = 20;
const GPT_INSTR: usize = 8000;
const GEM_FILES: usize = 10;
const GEM_INSTR: usize = 12000;
/// knowledge file size above which the chat products may refuse or cut it
const BIG_FILE: usize = 6_000_000;
/// DeepSeek part size (~40–60K tokens)
const DS_PART: usize = 120_000;
const RAG_CHUNK: usize = 2000;

type Text = Arc<str>;

#[derive(Serialize, Clone, Debug)]
pub struct Output {
    pub format: String,
    pub label: String,
    pub path: String,
    pub dir: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct BuildResult {
    pub dir: String,
    pub formats: Vec<String>,
    pub outputs: Vec<Output>,
    pub documents: usize,
    pub pages: usize,
    pub bytes: u64,
    pub terms: usize,
    pub warnings: Vec<String>,
}

/// The files of the package (path relative to `<out>/<name>/`, content) and the zips to make.
pub struct Package {
    pub name: String,
    pub formats: Vec<&'static str>,
    pub files: Vec<(String, Text)>,
    pub zips: Vec<(String, Vec<(String, Text)>)>,
    /// relative paths
    pub outputs: Vec<Output>,
    pub documents: usize,
    pub pages: usize,
    pub terms: usize,
    pub warnings: Vec<String>,
}

fn out(format: &str, label: &str, path: String, dir: bool) -> Output {
    Output { format: format.into(), label: label.into(), path, dir }
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
    let formats = format_list(&sk.formats);
    let n = sk.name.clone();

    // shared texts (Arc: the same source text goes into several formats without copies)
    let sources: Vec<Text> = docs.iter().map(|d| Text::from(source_md(&l, d))).collect();
    let need_k = formats.iter().any(|f| *f == "claude" || *f == "agents");
    let index: Text = if need_k { index_md(&l, &sk, &docs, &terms).into() } else { "".into() };
    let documents_t: Text = if need_k { documents_md(&l, &sk, &docs, &terms).into() } else { "".into() };
    let knowledge = |prefix: &str| -> Vec<(String, Text)> {
        let mut v = vec![(format!("{prefix}knowledge/index.md"), index.clone()), (format!("{prefix}knowledge/DOCUMENTS.md"), documents_t.clone())];
        for (d, t) in docs.iter().zip(&sources) {
            v.push((format!("{prefix}knowledge/sources/{}", d.file), t.clone()));
        }
        v
    };

    let mut files: Vec<(String, Text)> = vec![];
    let mut zips = vec![];
    let mut outputs = vec![];
    for f in &formats {
        match *f {
            "claude" => {
                let mut plugin: Vec<(String, Text)> = vec![
                    (".claude-plugin/plugin.json".into(), plugin_json(&sk, &description, author, &keywords.iter().take(12).cloned().collect::<Vec<_>>()).into()),
                    ("README.md".into(), readme_md(&l, &sk, &docs, &description).into()),
                    (format!("skills/{n}/SKILL.md"), skill_md(&l, &sk, &docs, &description, "../../knowledge/", any_ocr, false).into()),
                ];
                plugin.extend(knowledge(""));
                files.extend(plugin.iter().map(|(p, t)| (format!("claude/{n}/{p}"), t.clone())));
                zips.push((format!("claude/{n}.plugin"), plugin));
                let mut single: Vec<(String, Text)> = vec![(format!("{n}/SKILL.md"), skill_md(&l, &sk, &docs, &description, "knowledge/", any_ocr, false).into())];
                single.extend(knowledge(&format!("{n}/")));
                zips.push((format!("claude/{n}-skill.zip"), single));
                outputs.push(out(f, l.t("Claude Code plugin mappa", "Claude Code plugin folder"), format!("claude/{n}"), true));
                outputs.push(out(f, l.t("Claude plugin (.plugin)", "Claude plugin (.plugin)"), format!("claude/{n}.plugin"), false));
                outputs.push(out(f, l.t("Claude skill ZIP (claude.ai / Desktop)", "Claude skill ZIP (claude.ai / Desktop)"), format!("claude/{n}-skill.zip"), false));
            }
            "agents" => {
                let mut v: Vec<(String, Text)> = vec![(format!("{n}/SKILL.md"), skill_md(&l, &sk, &docs, &description, "knowledge/", any_ocr, true).into())];
                v.extend(knowledge(&format!("{n}/")));
                files.extend(v.iter().map(|(p, t)| (format!("agents/{p}"), t.clone())));
                files.push(("agents/README.md".into(), agents_readme(&l, &n).into()));
                zips.push((format!("agents/{n}.zip"), v));
                outputs.push(out(f, l.t("Agent Skill mappa (Codex, Gemini CLI, Copilot…)", "Agent Skill folder (Codex, Gemini CLI, Copilot…)"), format!("agents/{n}"), true));
                outputs.push(out(f, l.t("Agent Skill ZIP", "Agent Skill ZIP"), format!("agents/{n}.zip"), false));
            }
            "chatgpt" | "gemini" => {
                let (max_files, max_instr) = if *f == "chatgpt" { (GPT_FILES, GPT_INSTR) } else { (GEM_FILES, GEM_INSTR) };
                let groups = group_docs(&sources.iter().map(|t| t.len()).collect::<Vec<_>>(), max_files - 1);
                let mut loc = vec![String::new(); docs.len()];
                let mut kfiles: Vec<(String, Text)> = vec![];
                for (gi, g) in groups.iter().enumerate() {
                    let (name, text): (String, Text) = if g.len() == 1 {
                        (format!("{:02}-{}", gi + 1, docs[g[0]].file), sources[g[0]].clone())
                    } else {
                        let codes = format!("{}-{}", docs[g[0]].code, docs[*g.last().unwrap()].code);
                        let mut t = String::new();
                        for &i in g {
                            if !t.is_empty() {
                                t.push_str("\n\n");
                            }
                            t.push_str(&sources[i]);
                        }
                        (format!("{:02}-{codes}.md", gi + 1), t.into())
                    };
                    if text.len() > BIG_FILE {
                        warnings.push(format!("{}/knowledge/{name}: {} MB – {}", f, text.len() / 1_000_000, l.t("nagy fájl, a szolgáltatás elutasíthatja", "large file, the service may refuse it")));
                    }
                    for &i in g {
                        loc[i] = format!("`{name}`");
                    }
                    kfiles.push((name, text));
                }
                let (instr, cut_list) = chat_instructions(&l, &sk, &docs, &description, &loc, any_ocr, max_instr);
                if cut_list {
                    warnings.push(format!("{f}: {}", l.t("az utasítás túl hosszú lett, a dokumentumlista rövidítve (a katalógusban teljes)", "the instructions got too long; the document list is shortened (complete in the catalog)")));
                }
                files.push((format!("{f}/instructions.md"), instr.into()));
                files.push((format!("{f}/knowledge/00-catalog.md"), catalog_md(&l, &sk, &docs, &terms, &loc).into()));
                let count = kfiles.len() + 1;
                files.extend(kfiles.into_iter().map(|(p, t)| (format!("{f}/knowledge/{p}"), t)));
                files.push((format!("{f}/README.md"), chat_readme(&l, &sk, f, &description, count).into()));
                let label = if *f == "chatgpt" { l.t("ChatGPT: Custom GPT / Project", "ChatGPT: Custom GPT / Project") } else { l.t("Gemini Gem", "Gemini Gem") };
                outputs.push(out(f, label, f.to_string(), true));
            }
            "deepseek" => {
                let mut loc = vec![String::new(); docs.len()];
                let mut parts: Vec<(String, Text)> = vec![];
                for (i, d) in docs.iter().enumerate() {
                    let runs = split_pages(&d.pages, DS_PART);
                    let stem = d.file.trim_end_matches(".md");
                    let mut names = vec![];
                    for (k, run) in runs.iter().enumerate() {
                        let name = if runs.len() == 1 {
                            format!("{}-{stem}.md", d.code)
                        } else {
                            format!("{}-{stem}-p{}-{}.md", d.code, run[0].no, run[run.len() - 1].no)
                        };
                        names.push(if runs.len() == 1 { format!("`{name}`") } else { format!("`{name}` ({} {}–{})", l.t("oldal", "pages"), run[0].no, run[run.len() - 1].no) });
                        parts.push((name, part_md(&l, d, run, k + 1, runs.len()).into()));
                    }
                    loc[i] = names.join(", ");
                }
                let (instr, _) = chat_instructions(&l, &sk, &docs, &description, &loc, any_ocr, GEM_INSTR);
                files.push(("deepseek/system-prompt.md".into(), instr.into()));
                files.push(("deepseek/knowledge/00-catalog.md".into(), catalog_md(&l, &sk, &docs, &terms, &loc).into()));
                let count = parts.len() + 1;
                files.extend(parts.into_iter().map(|(p, t)| (format!("deepseek/knowledge/{p}"), t)));
                files.push(("deepseek/README.md".into(), chat_readme(&l, &sk, "deepseek", &description, count).into()));
                outputs.push(out(f, l.t("DeepSeek: rendszerprompt + részek", "DeepSeek: system prompt + parts"), "deepseek".into(), true));
            }
            _ => {
                // rag
                let mut jsonl = String::new();
                let mut chunks = 0;
                for d in &docs {
                    for p in &d.pages {
                        for (k, text) in chunk_page(&p.text, RAG_CHUNK).into_iter().enumerate() {
                            let v = serde_json::json!({
                                "id": format!("{}-p{}-{}", d.code, p.no, k + 1),
                                "doc": d.code,
                                "title": d.title(),
                                "file": d.d.name,
                                "page": p.no,
                                "ocr": p.ocr,
                                "text": text,
                            });
                            jsonl.push_str(&v.to_string());
                            jsonl.push('\n');
                            chunks += 1;
                        }
                    }
                }
                let meta: Vec<serde_json::Value> = docs
                    .iter()
                    .map(|d| {
                        serde_json::json!({
                            "doc": d.code, "title": d.title(), "file": d.d.name, "kind": d.d.kind,
                            "pages": d.pages.len(), "ocr_pages": d.d.ocr_pages,
                            "summary": d.d.summary.trim(), "use_when": d.d.use_when.trim(),
                            "outline": d.outline.iter().map(|o| serde_json::json!({ "level": o.level, "title": o.title, "page": o.page })).collect::<Vec<_>>(),
                        })
                    })
                    .collect();
                files.push(("rag/chunks.jsonl".into(), jsonl.into()));
                files.push(("rag/documents.json".into(), (serde_json::to_string_pretty(&meta).unwrap_or_default() + "\n").into()));
                files.push(("rag/system-prompt.md".into(), rag_prompt(&l, &sk, &description).into()));
                files.push(("rag/README.md".into(), rag_readme(&l, &sk, chunks).into()));
                outputs.push(out(f, l.t("RAG: chunks.jsonl + prompt", "RAG: chunks.jsonl + prompt"), "rag".into(), true));
            }
        }
    }
    files.insert(0, ("README.md".into(), root_readme(&l, &sk, &formats).into()));
    Ok(Package {
        name: n,
        formats,
        files,
        zips,
        outputs,
        documents: docs.len(),
        pages: docs.iter().map(|d| d.pages.len()).sum(),
        terms: terms.len(),
        warnings,
    })
}

fn zip_files(path: &Path, files: &[(String, Text)]) -> Result<u64, String> {
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

/// Writes `<out_root>/<name>/` (every chosen format) and the zips in it.
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
        std::fs::write(&p, text.as_bytes()).map_err(|e| format!("{}: {e}", p.display()))?;
        bytes += text.len() as u64;
    }
    std::fs::write(dir.join(MARKER), "Made by SkillStar. This folder is rebuilt on every build.\n").map_err(|e| e.to_string())?;
    for (rel, entries) in &pkg.zips {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        zip_files(&p, entries)?;
    }
    let outputs = pkg
        .outputs
        .iter()
        .map(|o| Output { path: dir.join(&o.path).to_string_lossy().into_owned(), ..o.clone() })
        .collect();
    Ok(BuildResult {
        dir: dir.to_string_lossy().into_owned(),
        formats: pkg.formats.iter().map(|f| f.to_string()).collect(),
        outputs,
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
    fn groups_and_chunks() {
        assert_eq!(group_docs(&[5, 5, 5], 19), vec![vec![0], vec![1], vec![2]]);
        let g = group_docs(&[10; 25], 9);
        assert!(g.len() <= 9);
        assert_eq!(g.iter().map(|x| x.len()).sum::<usize>(), 25);
        // order kept
        assert_eq!(g.concat(), (0..25).collect::<Vec<_>>());
        let long = "szó ".repeat(1500);
        let c = chunk_page(&format!("Első bekezdés.\n\n{long}\n\nUtolsó."), 2000);
        assert!(c.len() >= 3);
        assert!(c.iter().all(|x| x.chars().count() <= 2000));
        assert_eq!(c[0], "Első bekezdés.");
        assert_eq!(clean_formats("rag, Claude ,xyz"), "claude,rag");
        assert_eq!(clean_formats(""), "claude");
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
        assert_eq!(pkg.formats, vec!["claude"]);
        assert_eq!(pkg.documents, 2);
        // the unprocessed one is left out; the other two "changed" (fake mtime of the test)
        assert_eq!(pkg.warnings.iter().filter(|w| w.contains("nincs feldolgozva")).count(), 1);
        let names: Vec<&str> = pkg.files.iter().map(|f| f.0.as_str()).collect();
        assert!(names.contains(&"claude/teszt-tudas/skills/teszt-tudas/SKILL.md"));
        assert!(names.contains(&"claude/teszt-tudas/knowledge/sources/elso-kezikonyv.md"));
        assert!(names.contains(&"claude/teszt-tudas/knowledge/sources/elso-kezikonyv-2.md"));
        assert!(!names.iter().any(|n| n.starts_with("rag/")));
        let file = |p: &str| pkg.files.iter().find(|f| f.0 == p).unwrap().1.clone();
        let skill = file("claude/teszt-tudas/skills/teszt-tudas/SKILL.md");
        assert!(skill.starts_with("---\nname: teszt-tudas\ndescription: \""));
        assert!(skill.contains("../../knowledge/"));
        assert!(skill.contains("Grep pattern"));
        let src = file("claude/teszt-tudas/knowledge/sources/elso-kezikonyv.md");
        assert!(src.contains("--- PAGE 2 ---\n<!-- OCR -->\nSprite multiplexer"));
        let plugin: serde_json::Value = serde_json::from_str(&file("claude/teszt-tudas/.claude-plugin/plugin.json")).unwrap();
        assert_eq!(plugin["name"], "teszt-tudas");
        assert_eq!(plugin["author"]["name"], "Teszt Elek");

        let out = dir.join("out");
        let r = write(&pkg, &out).unwrap();
        assert!(Path::new(&r.dir).join("claude/teszt-tudas/.claude-plugin/plugin.json").is_file());
        assert_eq!(r.outputs.len(), 3);
        // a rebuild replaces our own folder
        write(&pkg, &out).unwrap();
        let skill_zip = &r.outputs.iter().find(|o| o.path.ends_with("-skill.zip")).unwrap().path;
        let mut z = zip::ZipArchive::new(std::fs::File::open(skill_zip).unwrap()).unwrap();
        let mut md = String::new();
        z.by_name("teszt-tudas/SKILL.md").unwrap().read_to_string(&mut md).unwrap();
        assert!(md.contains("name: teszt-tudas"));
        assert!(md.contains("`knowledge/`"));
        assert!(z.by_name("teszt-tudas/knowledge/index.md").is_ok());
        let plugin_zip = &r.outputs.iter().find(|o| o.path.ends_with(".plugin")).unwrap().path;
        let z = zip::ZipArchive::new(std::fs::File::open(plugin_zip).unwrap()).unwrap();
        assert!(z.file_names().any(|n| n == ".claude-plugin/plugin.json"));

        // every format
        let mut sk = db::skill(&c, s).unwrap();
        sk.formats = FORMATS.join(",");
        db::update_skill(&c, &sk).unwrap();
        let pkg = package(&c, s, "").unwrap();
        assert_eq!(pkg.formats.len(), FORMATS.len());
        let names: Vec<&str> = pkg.files.iter().map(|f| f.0.as_str()).collect();
        for want in [
            "README.md",
            "agents/teszt-tudas/SKILL.md",
            "agents/teszt-tudas/knowledge/index.md",
            "chatgpt/instructions.md",
            "chatgpt/knowledge/00-catalog.md",
            "chatgpt/knowledge/01-elso-kezikonyv.md",
            "gemini/instructions.md",
            "deepseek/system-prompt.md",
            "deepseek/knowledge/D1-elso-kezikonyv.md",
            "rag/chunks.jsonl",
            "rag/documents.json",
        ] {
            assert!(names.contains(&want), "{want} missing: {names:?}");
        }
        let file = |p: &str| pkg.files.iter().find(|f| f.0 == p).unwrap().1.clone();
        assert!(file("agents/teszt-tudas/SKILL.md").contains("rg -i"));
        let instr = file("chatgpt/instructions.md");
        assert!(instr.chars().count() <= GPT_INSTR);
        assert!(instr.contains("`01-elso-kezikonyv.md`"));
        let first: serde_json::Value = serde_json::from_str(file("rag/chunks.jsonl").lines().next().unwrap()).unwrap();
        assert_eq!(first["doc"], "D1");
        assert_eq!(first["page"], 1);
        let r = write(&pkg, &dir.join("all")).unwrap();
        assert!(Path::new(&r.dir).join("agents/teszt-tudas.zip").is_file());
        assert!(Path::new(&r.dir).join("gemini/knowledge/00-catalog.md").is_file());

        // a foreign folder with the same name is never deleted
        let foreign = dir.join("out2").join("teszt-tudas");
        std::fs::create_dir_all(&foreign).unwrap();
        std::fs::write(foreign.join("mine.txt"), "keep").unwrap();
        assert!(write(&pkg, &dir.join("out2")).is_err());
        assert!(foreign.join("mine.txt").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
