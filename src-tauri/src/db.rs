//! Local database (SQLite + FTS5): skills, their documents, the extracted
//! page texts and the documents' outlines.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS skills (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  title TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  author TEXT NOT NULL DEFAULT '',
  version TEXT NOT NULL DEFAULT '0.1.0',
  language TEXT NOT NULL DEFAULT 'hu',
  instructions TEXT NOT NULL DEFAULT '',
  keywords TEXT NOT NULL DEFAULT '',
  out_dir TEXT NOT NULL DEFAULT '',
  created INTEGER NOT NULL,
  updated INTEGER NOT NULL,
  built INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS documents (
  id INTEGER PRIMARY KEY,
  skill_id INTEGER NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
  path TEXT NOT NULL,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  mtime INTEGER NOT NULL DEFAULT 0,
  sha TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL DEFAULT 'new',
  error TEXT NOT NULL DEFAULT '',
  pages INTEGER NOT NULL DEFAULT 0,
  ocr_pages INTEGER NOT NULL DEFAULT 0,
  chars INTEGER NOT NULL DEFAULT 0,
  title TEXT NOT NULL DEFAULT '',
  summary TEXT NOT NULL DEFAULT '',
  use_when TEXT NOT NULL DEFAULT '',
  ocr_mode TEXT NOT NULL DEFAULT 'auto',
  include INTEGER NOT NULL DEFAULT 1,
  added INTEGER NOT NULL,
  processed INTEGER NOT NULL DEFAULT 0,
  ord INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS documents_skill ON documents(skill_id);
CREATE TABLE IF NOT EXISTS pages (
  doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  no INTEGER NOT NULL,
  text TEXT NOT NULL,
  ocr INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (doc_id, no)
);
CREATE TABLE IF NOT EXISTS outline (
  doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  ord INTEGER NOT NULL,
  level INTEGER NOT NULL,
  title TEXT NOT NULL,
  page INTEGER NOT NULL,
  PRIMARY KEY (doc_id, ord)
);
-- pages finished by a run that was interrupted (cancel, error, app closed): resumed next time
CREATE TABLE IF NOT EXISTS partial_pages (
  doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
  sha TEXT NOT NULL,
  no INTEGER NOT NULL,
  text TEXT NOT NULL,
  ocr INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (doc_id, no)
);
CREATE VIRTUAL TABLE IF NOT EXISTS pages_fts USING fts5(
  text, doc_id UNINDEXED, no UNINDEXED,
  tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

pub fn open(path: &Path) -> Result<Connection, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let c = Connection::open(path).map_err(|e| e.to_string())?;
    c.busy_timeout(std::time::Duration::from_secs(10)).map_err(|e| e.to_string())?;
    c.execute_batch(SCHEMA).map_err(|e| e.to_string())?;
    Ok(c)
}

#[cfg(test)]
pub fn open_memory() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(SCHEMA).unwrap();
    c
}

fn e(err: rusqlite::Error) -> String {
    err.to_string()
}

// ───────────────────────── skills ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Skill {
    pub id: i64,
    /// kebab-case id of the plugin / skill
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub version: String,
    /// language of the generated texts: "hu" | "en"
    pub language: String,
    /// extra guidance written into SKILL.md
    pub instructions: String,
    /// comma separated trigger words
    pub keywords: String,
    /// output folder override (empty = settings.output_dir)
    pub out_dir: String,
    pub created: i64,
    pub updated: i64,
    pub built: i64,
    // computed
    pub docs: i64,
    pub done: i64,
    pub pages: i64,
}

const SKILL_COLS: &str = "s.id, s.name, s.title, s.description, s.author, s.version, s.language, s.instructions, s.keywords, s.out_dir, s.created, s.updated, s.built,
  (SELECT COUNT(*) FROM documents d WHERE d.skill_id = s.id),
  (SELECT COUNT(*) FROM documents d WHERE d.skill_id = s.id AND d.status = 'done'),
  (SELECT COALESCE(SUM(pages), 0) FROM documents d WHERE d.skill_id = s.id)";

fn skill_row(r: &rusqlite::Row) -> rusqlite::Result<Skill> {
    Ok(Skill {
        id: r.get(0)?,
        name: r.get(1)?,
        title: r.get(2)?,
        description: r.get(3)?,
        author: r.get(4)?,
        version: r.get(5)?,
        language: r.get(6)?,
        instructions: r.get(7)?,
        keywords: r.get(8)?,
        out_dir: r.get(9)?,
        created: r.get(10)?,
        updated: r.get(11)?,
        built: r.get(12)?,
        docs: r.get(13)?,
        done: r.get(14)?,
        pages: r.get(15)?,
    })
}

pub fn skills(c: &Connection) -> Result<Vec<Skill>, String> {
    let mut st = c.prepare(&format!("SELECT {SKILL_COLS} FROM skills s ORDER BY s.title COLLATE NOCASE")).map_err(e)?;
    let rows = st.query_map([], skill_row).map_err(e)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(e)
}

pub fn skill(c: &Connection, id: i64) -> Result<Skill, String> {
    c.query_row(&format!("SELECT {SKILL_COLS} FROM skills s WHERE s.id = ?1"), [id], skill_row)
        .optional()
        .map_err(e)?
        .ok_or_else(|| crate::settings::tr("A skill nem található.", "Skill not found."))
}

pub fn create_skill(c: &Connection, title: &str, author: &str, language: &str) -> Result<i64, String> {
    let mut name = crate::skill::slugify(title, "skill");
    // names stay unique: they become folder names
    let taken: Vec<String> = skills(c)?.into_iter().map(|s| s.name).collect();
    if taken.contains(&name) {
        let base = name.clone();
        let mut i = 2;
        while taken.contains(&name) {
            name = format!("{base}-{i}");
            i += 1;
        }
    }
    let t = now();
    c.execute(
        "INSERT INTO skills (name, title, author, language, created, updated) VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        params![name, title.trim(), author, language, t],
    )
    .map_err(e)?;
    Ok(c.last_insert_rowid())
}

pub fn update_skill(c: &Connection, s: &Skill) -> Result<(), String> {
    let name = crate::skill::slugify(&s.name, "skill");
    c.execute(
        "UPDATE skills SET name=?2, title=?3, description=?4, author=?5, version=?6, language=?7, instructions=?8, keywords=?9, out_dir=?10, updated=?11 WHERE id=?1",
        params![s.id, name, s.title.trim(), s.description.trim(), s.author.trim(), s.version.trim(), s.language, s.instructions, s.keywords.trim(), s.out_dir.trim(), now()],
    )
    .map_err(e)?;
    Ok(())
}

pub fn set_built(c: &Connection, id: i64) -> Result<(), String> {
    c.execute("UPDATE skills SET built=?2 WHERE id=?1", params![id, now()]).map_err(e)?;
    Ok(())
}

pub fn delete_skill(c: &Connection, id: i64) -> Result<(), String> {
    let docs: Vec<i64> = documents(c, id)?.into_iter().map(|d| d.id).collect();
    for d in docs {
        delete_document(c, d)?;
    }
    c.execute("DELETE FROM skills WHERE id=?1", [id]).map_err(e)?;
    Ok(())
}

// ───────────────────────── documents ─────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Document {
    pub id: i64,
    pub skill_id: i64,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: i64,
    pub mtime: i64,
    pub sha: String,
    /// "new" | "queued" | "processing" | "done" | "error"
    pub status: String,
    pub error: String,
    pub pages: i64,
    pub ocr_pages: i64,
    pub chars: i64,
    pub title: String,
    pub summary: String,
    pub use_when: String,
    /// "auto" | "force" | "off"
    pub ocr_mode: String,
    pub include: bool,
    pub added: i64,
    pub processed: i64,
    pub ord: i64,
    /// the source file is missing or changed since processing
    pub stale: bool,
    pub missing: bool,
    /// pages already done by an interrupted run (resumable)
    pub partial: i64,
}

const DOC_COLS: &str = "id, skill_id, path, name, kind, size, mtime, sha, status, error, pages, ocr_pages, chars, title, summary, use_when, ocr_mode, include, added, processed, ord,
  (SELECT COUNT(*) FROM partial_pages pp WHERE pp.doc_id = documents.id)";

fn doc_row(r: &rusqlite::Row) -> rusqlite::Result<Document> {
    Ok(Document {
        id: r.get(0)?,
        skill_id: r.get(1)?,
        path: r.get(2)?,
        name: r.get(3)?,
        kind: r.get(4)?,
        size: r.get(5)?,
        mtime: r.get(6)?,
        sha: r.get(7)?,
        status: r.get(8)?,
        error: r.get(9)?,
        pages: r.get(10)?,
        ocr_pages: r.get(11)?,
        chars: r.get(12)?,
        title: r.get(13)?,
        summary: r.get(14)?,
        use_when: r.get(15)?,
        ocr_mode: r.get(16)?,
        include: r.get::<_, i64>(17)? != 0,
        added: r.get(18)?,
        processed: r.get(19)?,
        ord: r.get(20)?,
        stale: false,
        missing: false,
        partial: r.get(21)?,
    })
}

pub fn documents(c: &Connection, skill_id: i64) -> Result<Vec<Document>, String> {
    let mut st = c.prepare(&format!("SELECT {DOC_COLS} FROM documents WHERE skill_id=?1 ORDER BY ord, name COLLATE NOCASE")).map_err(e)?;
    let rows = st.query_map([skill_id], doc_row).map_err(e)?;
    let mut list = rows.collect::<Result<Vec<_>, _>>().map_err(e)?;
    for d in &mut list {
        match std::fs::metadata(&d.path) {
            Ok(m) => d.stale = d.status == "done" && file_mtime(&m) != d.mtime,
            Err(_) => d.missing = true,
        }
    }
    Ok(list)
}

pub fn document(c: &Connection, id: i64) -> Result<Document, String> {
    c.query_row(&format!("SELECT {DOC_COLS} FROM documents WHERE id=?1"), [id], doc_row)
        .optional()
        .map_err(e)?
        .ok_or_else(|| crate::settings::tr("A dokumentum nem található.", "Document not found."))
}

pub fn file_mtime(m: &std::fs::Metadata) -> i64 {
    m.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Adds a file to a skill; a file already in the skill is skipped (returns None).
pub fn add_document(c: &Connection, skill_id: i64, path: &Path) -> Result<Option<i64>, String> {
    let p = path.to_string_lossy().to_string();
    let exists: Option<i64> = c
        .query_row("SELECT id FROM documents WHERE skill_id=?1 AND path=?2", params![skill_id, p], |r| r.get(0))
        .optional()
        .map_err(e)?;
    if exists.is_some() {
        return Ok(None);
    }
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| p.clone());
    let kind = crate::extract::kind_of(path).unwrap_or("unknown").to_string();
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let ord: i64 = c.query_row("SELECT COALESCE(MAX(ord), 0) + 1 FROM documents WHERE skill_id=?1", [skill_id], |r| r.get(0)).map_err(e)?;
    c.execute(
        "INSERT INTO documents (skill_id, path, name, kind, size, added, ord) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![skill_id, p, name, kind, size, now(), ord],
    )
    .map_err(e)?;
    c.execute("UPDATE skills SET updated=?2 WHERE id=?1", params![skill_id, now()]).map_err(e)?;
    Ok(Some(c.last_insert_rowid()))
}

pub fn delete_document(c: &Connection, id: i64) -> Result<(), String> {
    c.execute("DELETE FROM partial_pages WHERE doc_id=?1", [id]).map_err(e)?;
    c.execute("DELETE FROM pages_fts WHERE doc_id=?1", [id]).map_err(e)?;
    c.execute("DELETE FROM pages WHERE doc_id=?1", [id]).map_err(e)?;
    c.execute("DELETE FROM outline WHERE doc_id=?1", [id]).map_err(e)?;
    c.execute("DELETE FROM documents WHERE id=?1", [id]).map_err(e)?;
    Ok(())
}

pub fn set_status(c: &Connection, id: i64, status: &str, error: &str) -> Result<(), String> {
    c.execute("UPDATE documents SET status=?2, error=?3 WHERE id=?1", params![id, status, error]).map_err(e)?;
    Ok(())
}

/// Fields the user edits in the UI.
pub fn update_document_meta(c: &Connection, id: i64, title: &str, summary: &str, use_when: &str, ocr_mode: &str, include: bool) -> Result<(), String> {
    c.execute(
        "UPDATE documents SET title=?2, summary=?3, use_when=?4, ocr_mode=?5, include=?6 WHERE id=?1",
        params![id, title.trim(), summary.trim(), use_when.trim(), ocr_mode, include as i64],
    )
    .map_err(e)?;
    Ok(())
}

pub fn reorder(c: &Connection, ids: &[i64]) -> Result<(), String> {
    for (i, id) in ids.iter().enumerate() {
        c.execute("UPDATE documents SET ord=?2 WHERE id=?1", params![id, i as i64 + 1]).map_err(e)?;
    }
    Ok(())
}

/// Replaces the text of a processed document.
pub fn store_result(c: &mut Connection, id: i64, x: &crate::extract::Extracted, sha: &str, size: i64, mtime: i64) -> Result<(), String> {
    let tx = c.transaction().map_err(e)?;
    tx.execute("DELETE FROM partial_pages WHERE doc_id=?1", [id]).map_err(e)?;
    tx.execute("DELETE FROM pages_fts WHERE doc_id=?1", [id]).map_err(e)?;
    tx.execute("DELETE FROM pages WHERE doc_id=?1", [id]).map_err(e)?;
    tx.execute("DELETE FROM outline WHERE doc_id=?1", [id]).map_err(e)?;
    let mut chars = 0i64;
    let mut ocr = 0i64;
    for p in &x.pages {
        chars += p.text.chars().filter(|c| !c.is_whitespace()).count() as i64;
        if p.ocr {
            ocr += 1;
        }
        tx.execute("INSERT INTO pages (doc_id, no, text, ocr) VALUES (?1, ?2, ?3, ?4)", params![id, p.no, p.text, p.ocr as i64]).map_err(e)?;
        tx.execute("INSERT INTO pages_fts (text, doc_id, no) VALUES (?1, ?2, ?3)", params![p.text, id, p.no]).map_err(e)?;
    }
    for (i, o) in x.outline.iter().enumerate() {
        tx.execute("INSERT INTO outline (doc_id, ord, level, title, page) VALUES (?1, ?2, ?3, ?4, ?5)", params![id, i as i64, o.level, o.title, o.page]).map_err(e)?;
    }
    // a title from the document fills an empty one, never overwrites the user's
    tx.execute(
        "UPDATE documents SET status='done', error='', pages=?2, ocr_pages=?3, chars=?4, sha=?5, size=?6, mtime=?7, processed=?8,
           title = CASE WHEN title = '' THEN ?9 ELSE title END WHERE id=?1",
        params![id, x.pages.len() as i64, ocr, chars, sha, size, mtime, now(), x.title.clone().unwrap_or_default()],
    )
    .map_err(e)?;
    tx.commit().map_err(e)?;
    Ok(())
}

#[derive(Serialize, Clone, Debug)]
pub struct Page {
    pub no: i64,
    pub text: String,
    pub ocr: bool,
}

pub fn pages(c: &Connection, doc_id: i64) -> Result<Vec<Page>, String> {
    let mut st = c.prepare("SELECT no, text, ocr FROM pages WHERE doc_id=?1 ORDER BY no").map_err(e)?;
    let rows = st.query_map([doc_id], |r| Ok(Page { no: r.get(0)?, text: r.get(1)?, ocr: r.get::<_, i64>(2)? != 0 })).map_err(e)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(e)
}

/// The user corrected the (OCR) text of a page.
pub fn set_page_text(c: &Connection, doc_id: i64, no: i64, text: &str) -> Result<(), String> {
    c.execute("UPDATE pages SET text=?3 WHERE doc_id=?1 AND no=?2", params![doc_id, no, text]).map_err(e)?;
    c.execute("DELETE FROM pages_fts WHERE doc_id=?1 AND no=?2", params![doc_id, no]).map_err(e)?;
    c.execute("INSERT INTO pages_fts (text, doc_id, no) VALUES (?1, ?2, ?3)", params![text, doc_id, no]).map_err(e)?;
    let chars: i64 = c
        .query_row("SELECT COALESCE(SUM(LENGTH(REPLACE(REPLACE(REPLACE(text, ' ', ''), char(10), ''), char(13), ''))), 0) FROM pages WHERE doc_id=?1", [doc_id], |r| r.get(0))
        .map_err(e)?;
    c.execute("UPDATE documents SET chars=?2 WHERE id=?1", params![doc_id, chars]).map_err(e)?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct OutlineItem {
    pub level: i64,
    pub title: String,
    pub page: i64,
}

pub fn outline(c: &Connection, doc_id: i64) -> Result<Vec<OutlineItem>, String> {
    let mut st = c.prepare("SELECT level, title, page FROM outline WHERE doc_id=?1 ORDER BY ord").map_err(e)?;
    let rows = st.query_map([doc_id], |r| Ok(OutlineItem { level: r.get(0)?, title: r.get(1)?, page: r.get(2)? })).map_err(e)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(e)
}

// ───────────────────────── resumable processing ─────────────────────────

/// Pages of an interrupted run of this exact file (same hash); others are dropped.
pub fn partial_pages(c: &Connection, doc_id: i64, sha: &str) -> Result<std::collections::HashMap<i64, crate::extract::PageText>, String> {
    c.execute("DELETE FROM partial_pages WHERE doc_id=?1 AND sha<>?2", params![doc_id, sha]).map_err(e)?;
    let mut st = c.prepare("SELECT no, text, ocr FROM partial_pages WHERE doc_id=?1").map_err(e)?;
    let rows = st
        .query_map([doc_id], |r| Ok(crate::extract::PageText { no: r.get(0)?, text: r.get(1)?, ocr: r.get::<_, i64>(2)? != 0 }))
        .map_err(e)?;
    let mut map = std::collections::HashMap::new();
    for p in rows {
        let p = p.map_err(e)?;
        map.insert(p.no, p);
    }
    Ok(map)
}

pub fn save_partial(c: &Connection, doc_id: i64, sha: &str, p: &crate::extract::PageText) -> Result<(), String> {
    c.execute(
        "INSERT OR REPLACE INTO partial_pages (doc_id, sha, no, text, ocr) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![doc_id, sha, p.no, p.text, p.ocr as i64],
    )
    .map_err(e)?;
    Ok(())
}

/// "Process again from scratch": forget the pages of an interrupted run.
pub fn clear_partial(c: &Connection, doc_id: i64) -> Result<(), String> {
    c.execute("DELETE FROM partial_pages WHERE doc_id=?1", [doc_id]).map_err(e)?;
    Ok(())
}

// ───────────────────────── search ─────────────────────────

#[derive(Serialize, Clone, Debug)]
pub struct Hit {
    pub doc_id: i64,
    pub doc: String,
    pub page: i64,
    pub snippet: String,
    pub text: String,
}

/// Words of a free-text query as an FTS5 expression (every word, prefix match).
pub fn fts_query(q: &str, any: bool) -> String {
    let words: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '$')
        .map(|w| w.trim_matches('$'))
        .filter(|w| !w.is_empty())
        .take(24)
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    words.join(if any { " OR " } else { " " })
}

pub fn search(c: &Connection, skill_id: i64, q: &str, limit: i64) -> Result<Vec<Hit>, String> {
    let mut hits = run_search(c, skill_id, &fts_query(q, false), limit)?;
    if hits.is_empty() {
        hits = run_search(c, skill_id, &fts_query(q, true), limit)?;
    }
    Ok(hits)
}

fn run_search(c: &Connection, skill_id: i64, m: &str, limit: i64) -> Result<Vec<Hit>, String> {
    if m.is_empty() {
        return Ok(vec![]);
    }
    let mut st = c
        .prepare(
            "SELECT f.doc_id, CASE WHEN d.title <> '' THEN d.title ELSE d.name END, f.no,
                    snippet(pages_fts, 0, '\u{1}', '\u{2}', ' … ', 24), f.text
             FROM pages_fts f JOIN documents d ON d.id = f.doc_id
             WHERE pages_fts MATCH ?1 AND d.skill_id = ?2
             ORDER BY bm25(pages_fts) LIMIT ?3",
        )
        .map_err(e)?;
    let rows = st
        .query_map(params![m, skill_id, limit], |r| {
            Ok(Hit { doc_id: r.get(0)?, doc: r.get(1)?, page: r.get(2)?, snippet: r.get(3)?, text: r.get(4)? })
        })
        .map_err(e)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Extracted, PageText};

    #[test]
    fn store_and_search_pages() {
        let mut c = open_memory();
        let s = create_skill(&c, "Árvíztűrő Tükörfúrógép", "", "hu").unwrap();
        assert_eq!(skill(&c, s).unwrap().name, "arvizturo-tukorfurogep");
        // a second skill with the same title gets a new name
        let s2 = create_skill(&c, "Árvíztűrő tükörfúrógép", "", "hu").unwrap();
        assert_eq!(skill(&c, s2).unwrap().name, "arvizturo-tukorfurogep-2");
        let dir = std::env::temp_dir().join(format!("skillstar-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let d = add_document(&c, s, &f).unwrap().unwrap();
        assert!(add_document(&c, s, &f).unwrap().is_none());
        let x = Extracted {
            pages: vec![
                PageText { no: 1, text: "A raszter megszakítás beállítása".into(), ocr: false },
                PageText { no: 2, text: "Sprite multiplexer".into(), ocr: true },
            ],
            outline: vec![],
            title: Some("Kézikönyv".into()),
        };
        store_result(&mut c, d, &x, "abc", 1, 2).unwrap();
        let doc = document(&c, d).unwrap();
        assert_eq!((doc.pages, doc.ocr_pages, doc.title.as_str(), doc.status.as_str()), (2, 1, "Kézikönyv", "done"));
        // accent-insensitive prefix search
        let h = search(&c, s, "megszakitas", 10).unwrap();
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].page, 1);
        assert!(search(&c, s2, "megszakitas", 10).unwrap().is_empty());
        // an interrupted run is resumed only for the same file content
        save_partial(&c, d, "h1", &PageText { no: 1, text: "p1".into(), ocr: true }).unwrap();
        assert_eq!(document(&c, d).unwrap().partial, 1);
        assert_eq!(partial_pages(&c, d, "h1").unwrap().len(), 1);
        assert!(partial_pages(&c, d, "h2").unwrap().is_empty());
        assert_eq!(document(&c, d).unwrap().partial, 0);
        set_page_text(&c, d, 2, "Sprite multiplexer javítva").unwrap();
        assert_eq!(search(&c, s, "javitva", 10).unwrap().len(), 1);
        delete_skill(&c, s).unwrap();
        assert!(search(&c, s, "sprite", 10).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
