//! RAG over the local full-text index: the question (and the extra search
//! queries the AI may write) → FTS5 prefix queries without stop words →
//! results fused with reciprocal rank fusion → the best passage of every page,
//! numbered, within a character budget. No embeddings, no server.

use crate::db;
use crate::skill::{chunk_page, fold, is_stop};
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;

#[derive(Serialize, Clone, Debug)]
pub struct Passage {
    /// 1-based number the answer cites as [n]
    pub n: usize,
    pub doc_id: i64,
    pub skill_id: i64,
    pub doc: String,
    pub page: i64,
    pub text: String,
}

/// Search terms of a query: no stop words, long words cut to a stem so that
/// inflected forms match too (Hungarian, German…), at most 16 terms.
pub fn terms(q: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in q.split(|c: char| !(c.is_alphanumeric() || c == '$' || c == '_')) {
        let w = w.trim_matches('$').to_lowercase();
        let n = w.chars().count();
        if n < 2 || is_stop(&w) || is_stop(&fold(&w)) {
            continue;
        }
        let stem: String = if n > 7 { w.chars().take((n * 2 / 3).max(6)).collect() } else { w };
        if !out.contains(&stem) {
            out.push(stem);
        }
        if out.len() == 16 {
            break;
        }
    }
    out
}

fn expr(terms: &[String], any: bool) -> String {
    terms.iter().map(|t| format!("\"{}\"*", t.replace('"', "\"\""))).collect::<Vec<_>>().join(if any { " OR " } else { " " })
}

/// The part of a long page that holds the most query terms (plus the next best part).
fn best_window(text: &str, terms: &[String], max: usize) -> String {
    if text.chars().count() <= max {
        return text.trim().to_string();
    }
    let folded: Vec<String> = terms.iter().map(|t| fold(t)).collect();
    let chunks = chunk_page(text, max / 2);
    let score = |c: &str| {
        let f = fold(c);
        folded.iter().map(|t| f.matches(t.as_str()).count()).sum::<usize>()
    };
    let mut ranked: Vec<(usize, usize)> = chunks.iter().enumerate().map(|(i, c)| (score(c), i)).collect();
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let mut pick: Vec<usize> = ranked.iter().take(2).filter(|(s, _)| *s > 0).map(|x| x.1).collect();
    if pick.is_empty() {
        pick.push(0);
    }
    pick.sort();
    pick.iter().map(|&i| chunks[i].as_str()).collect::<Vec<_>>().join("\n…\n")
}

/// Retrieves at most `k` passages for the queries (the first is the question itself).
/// `skill_id` 0 searches every skill.
pub fn retrieve(c: &Connection, skill_id: i64, queries: &[String], k: usize, budget: usize) -> Result<Vec<Passage>, String> {
    let mut score: HashMap<(i64, i64), f64> = HashMap::new();
    let mut hits: HashMap<(i64, i64), db::Hit> = HashMap::new();
    let mut all_terms: Vec<String> = vec![];
    for (qi, q) in queries.iter().enumerate() {
        let ts = terms(q);
        if ts.is_empty() {
            continue;
        }
        for t in &ts {
            if !all_terms.contains(t) {
                all_terms.push(t.clone());
            }
        }
        // every word first; when that finds little, any word
        let mut list = db::search_expr(c, skill_id, &expr(&ts, false), 25)?;
        if list.len() < 6 && ts.len() > 1 {
            for h in db::search_expr(c, skill_id, &expr(&ts, true), 40)? {
                if !list.iter().any(|x| x.doc_id == h.doc_id && x.page == h.page) {
                    list.push(h);
                }
            }
        }
        // the user's own words weigh a little more than the AI's rewrites
        let w = if qi == 0 { 1.2 } else { 1.0 };
        for (rank, h) in list.into_iter().enumerate() {
            let key = (h.doc_id, h.page);
            *score.entry(key).or_default() += w / (60.0 + rank as f64);
            hits.entry(key).or_insert(h);
        }
    }
    let mut ranked: Vec<((i64, i64), f64)> = score.into_iter().collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal).then(a.0.cmp(&b.0)));
    let mut out = vec![];
    let mut size = 0;
    for (key, _) in ranked.into_iter().take(k) {
        let Some(h) = hits.remove(&key) else { continue };
        let text = best_window(&h.text, &all_terms, 3000);
        if size + text.len() > budget && !out.is_empty() {
            break;
        }
        size += text.len();
        out.push(Passage { n: out.len() + 1, doc_id: h.doc_id, skill_id: h.skill_id, doc: h.doc, page: h.page, text });
    }
    Ok(out)
}

/// The passages as the context of the answering model.
pub fn context(passages: &[Passage]) -> String {
    let mut s = String::from("<passages>\n");
    for p in passages {
        s.push_str(&format!("<passage n=\"{}\" document=\"{}\" page=\"{}\">\n{}\n</passage>\n", p.n, p.doc.replace('"', "'"), p.page, p.text));
    }
    s.push_str("</passages>\n");
    s
}

/// The search queries written by the AI: one per line, list marks and quotes removed.
pub fn parse_queries(text: &str, max: usize) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_start_matches(|c: char| c == '-' || c == '*' || c == '•' || c.is_ascii_digit() || c == '.' || c == ')').trim().trim_matches(|c| c == '"' || c == '\'' || c == '`').trim().to_string())
        .filter(|l| !l.is_empty() && l.chars().count() <= 200 && !l.ends_with(':'))
        .take(max)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::{Extracted, PageText};

    #[test]
    fn query_terms() {
        assert_eq!(terms("Hogyan működik a raszter megszakítás?"), vec!["hogyan", "működik", "raszter", "megszak"]);
        assert_eq!(terms("$D012 IRQ"), vec!["d012", "irq"]);
        assert_eq!(parse_queries("1. raszter IRQ\n- \"sprite multiplexer\"\nKeresések:\n\n", 5), vec!["raszter IRQ", "sprite multiplexer"]);
    }

    #[test]
    fn retrieves_and_fuses() {
        let mut c = db::open_memory();
        let s = db::create_skill(&c, "Teszt", "", "hu").unwrap();
        let dir = std::env::temp_dir().join(format!("skillstar-rag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let d = db::add_document(&c, s, &f).unwrap().unwrap();
        let page = |no, t: &str| PageText { no, text: t.to_string(), ocr: false };
        let long = format!("{}A raszter megszakításokat a $D012 regiszter állítja be.\n\n{}", "töltelék szöveg. ".repeat(300), "más téma. ".repeat(300));
        let x = Extracted {
            pages: vec![page(1, "Sprite-ok és színek."), page(2, &long), page(3, "A megszakítás vektor a $0314 címen van.")],
            ..Default::default()
        };
        db::store_result(&mut c, d, &x, "h", 1, 1).unwrap();
        let p = retrieve(&c, s, &["raszter megszakítások".into()], 8, 50_000).unwrap();
        assert_eq!(p[0].page, 2);
        assert_eq!(p[0].n, 1);
        assert!(p[0].text.contains("$D012"));
        assert!(p[0].text.chars().count() < 3200);
        // OR fallback + a second query
        let p = retrieve(&c, 0, &["megszakítás vektor raszterfoo".into(), "IRQ vektor".into()], 8, 50_000).unwrap();
        assert!(p.iter().any(|x| x.page == 3));
        assert!(context(&p).contains("<passage n=\"1\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
