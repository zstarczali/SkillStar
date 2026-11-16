//! Website import: the pages of a site are downloaded by following its links
//! up to a given depth (breadth first, the same site only, robots.txt is
//! respected). The text of every page is kept in one `.skweb` file (JSON) that
//! is a document of its own: each web page starts a new page with its title in
//! the outline and its address on top. Documents linked from the pages (PDF,
//! EPUB, DOCX…) can be downloaded too; they become separate documents.

use crate::extract::{html_to_text, kind_of, Extracted};
use crate::settings::tr;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub static CANCEL: AtomicBool = AtomicBool::new(false);

const MAX_HTML: usize = 10 * 1024 * 1024;
const MAX_DOC: usize = 200 * 1024 * 1024;
/// Pages with less text than this are not kept (their links are still followed).
const MIN_TEXT: usize = 40;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WebPage {
    pub url: String,
    pub title: String,
    pub text: String,
    pub depth: u32,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Archive {
    pub url: String,
    pub title: String,
    pub depth: u32,
    /// ms since 1970
    pub fetched: i64,
    pub pages: Vec<WebPage>,
}

pub struct CrawlOpts {
    pub url: String,
    pub depth: u32,
    pub max_pages: usize,
    /// only addresses under the folder of the start page
    pub same_prefix: bool,
    /// download linked documents (PDF, EPUB, DOCX…) too
    pub docs: bool,
    /// pause between two requests
    pub delay_ms: u64,
}

#[derive(Serialize, Clone, Debug)]
pub struct WebProgress {
    pub done: usize,
    pub queued: usize,
    pub pages: usize,
    pub docs: usize,
    pub url: String,
}

pub struct CrawlResult {
    pub archive: Option<PathBuf>,
    pub pages: usize,
    pub docs: Vec<PathBuf>,
    pub errors: Vec<String>,
    pub stopped: bool,
}

// ───────────────────────── addresses ─────────────────────────

pub fn parse_start(url: &str) -> Result<Url, String> {
    let u = url.trim();
    let u = if u.contains("://") { u.to_string() } else { format!("https://{u}") };
    let url = Url::parse(&u).map_err(|e| format!("{}: {e}", tr("Érvénytelen webcím", "Invalid web address")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(tr("Csak http:// vagy https:// cím adható meg.", "Only an http:// or https:// address can be given."));
    }
    Ok(url)
}

fn host_key(u: &Url) -> String {
    u.host_str().unwrap_or("").trim_start_matches("www.").to_lowercase()
}

/// The address without its #fragment.
fn norm(u: &Url) -> String {
    let mut u = u.clone();
    u.set_fragment(None);
    u.to_string()
}

/// The folder of the start page: `/docs/guide/intro.html` → `/docs/guide/`.
fn prefix_of(u: &Url) -> String {
    let p = u.path();
    match p.rfind('/') {
        Some(i) => p[..=i].to_string(),
        None => "/".into(),
    }
}

const SKIP_EXT: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "ico", "bmp", "tif", "tiff", "avif", "css", "js", "mjs", "json", "xml", "rss", "atom", "woff", "woff2", "ttf",
    "otf", "eot", "mp3", "mp4", "m4a", "webm", "ogg", "wav", "avi", "mov", "mkv", "zip", "rar", "7z", "gz", "tgz", "bz2", "xz", "exe", "msi", "dmg", "pkg",
    "deb", "rpm", "apk", "iso", "bin", "csv", "xls", "xlsx", "ppt", "doc",
];

fn ext_of(u: &Url) -> String {
    let last = u.path().rsplit('/').next().unwrap_or("");
    match last.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => e.to_lowercase(),
        _ => String::new(),
    }
}

/// A linked document SkillStar can read (not a web page).
fn is_doc_link(u: &Url) -> bool {
    let e = ext_of(u);
    !e.is_empty() && !matches!(e.as_str(), "html" | "htm" | "xhtml" | "php" | "asp" | "aspx" | "jsp") && kind_of(Path::new(&format!("x.{e}"))).is_some_and(|k| k != "web" && k != "image")
}

// ───────────────────────── robots.txt ─────────────────────────

#[derive(Default)]
struct Robots {
    disallow: Vec<String>,
    allow: Vec<String>,
}

impl Robots {
    fn parse(txt: &str) -> Robots {
        let mut r = Robots::default();
        let mut ours = false;
        let mut in_agents = false;
        for line in txt.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once(':') else { continue };
            let (k, v) = (k.trim().to_lowercase(), v.trim());
            match k.as_str() {
                "user-agent" => {
                    let me = v == "*" || v.to_lowercase().contains("skillstar");
                    ours = if in_agents { ours || me } else { me };
                    in_agents = true;
                }
                "disallow" if ours && !v.is_empty() => {
                    in_agents = false;
                    r.disallow.push(v.to_string());
                }
                "allow" if ours && !v.is_empty() => {
                    in_agents = false;
                    r.allow.push(v.to_string());
                }
                _ => in_agents = false,
            }
        }
        r
    }

    /// The longest matching rule wins (`*` and `$` are understood).
    fn allowed(&self, path: &str) -> bool {
        let m = |rules: &[String]| rules.iter().filter(|r| rule_match(r, path)).map(|r| r.len()).max().unwrap_or(0);
        let (a, d) = (m(&self.allow), m(&self.disallow));
        d == 0 || a >= d
    }
}

fn rule_match(rule: &str, path: &str) -> bool {
    let (rule, anchored) = match rule.strip_suffix('$') {
        Some(r) => (r, true),
        None => (rule, false),
    };
    let parts: Vec<&str> = rule.split('*').collect();
    let mut pos = 0;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            if !path.starts_with(part) {
                return false;
            }
            pos = part.len();
        } else if let Some(f) = path[pos..].find(part) {
            pos += f + part.len();
        } else {
            return false;
        }
    }
    !anchored || pos == path.len() || parts.last().is_some_and(|p| p.is_empty())
}

// ───────────────────────── HTML ─────────────────────────

/// Bytes → text by the declared charset (header or <meta>).
fn decode_html(b: &[u8], header_cs: Option<&str>) -> String {
    let head = String::from_utf8_lossy(&b[..b.len().min(4096)]).to_ascii_lowercase();
    let meta_cs = head.find("charset=").map(|i| head[i + 8..].trim_start_matches(['"', '\'']).chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect::<String>());
    let cs = header_cs.map(str::to_lowercase).or(meta_cs).unwrap_or_default();
    if cs.contains("1251") {
        return b.iter().map(|&c| crate::ebook::cp1251(c)).collect();
    }
    crate::extract::decode_text(b)
}

/// Removes `<tag …>…</tag>` blocks (nesting counted).
fn strip_blocks(html: &str, tags: &[&str]) -> String {
    let mut s = html.to_string();
    for tag in tags {
        loop {
            let low = s.to_ascii_lowercase();
            let open = format!("<{tag}");
            let Some(a) = find_tag(&low, &open, 0) else { break };
            let close = format!("</{tag}");
            let mut depth = 0usize;
            let mut i = a;
            let mut end = s.len();
            while i < low.len() {
                let no = find_tag(&low, &open, i);
                let nc = low[i..].find(&close).map(|x| x + i);
                match (no, nc) {
                    (Some(o), Some(c)) if o < c => {
                        depth += 1;
                        i = o + open.len();
                    }
                    (_, Some(c)) => {
                        depth = depth.saturating_sub(1);
                        i = c + close.len();
                        if depth == 0 {
                            end = low[i..].find('>').map(|x| x + i + 1).unwrap_or(low.len());
                            break;
                        }
                    }
                    _ => break,
                }
            }
            s.replace_range(a..end, " ");
        }
    }
    s
}

/// `<tag` followed by a space, `>` or `/` (so `<nav` does not match `<navbar-x`).
fn find_tag(low: &str, open: &str, from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(f) = low[i..].find(open) {
        let at = i + f;
        let next = low.as_bytes().get(at + open.len()).copied().unwrap_or(b'>');
        if next == b'>' || next == b'/' || next.is_ascii_whitespace() {
            return Some(at);
        }
        i = at + open.len();
    }
    None
}

/// The inside of the first `<tag>…</tag>`.
fn inner_of<'a>(html: &'a str, low: &str, tag: &str) -> Option<&'a str> {
    let a = find_tag(low, &format!("<{tag}"), 0)?;
    let start = low[a..].find('>')? + a + 1;
    let end = low.rfind(&format!("</{tag}")).filter(|&e| e > start)?;
    Some(&html[start..end])
}

/// The readable part of a page: <main>, else a single <article>, else the body
/// without its navigation, header, footer, sidebars and forms.
pub fn main_text(html: &str) -> (String, Option<String>) {
    let low = html.to_ascii_lowercase();
    let (_, title) = html_to_text(&html[..low.find("<body").unwrap_or(html.len().min(20000)).min(html.len())]);
    let articles = low.matches("<article").count();
    let part = inner_of(html, &low, "main").or_else(|| if articles == 1 { inner_of(html, &low, "article") } else { None });
    let body = match part {
        Some(p) => strip_blocks(p, &["nav", "aside", "form", "script", "style", "noscript"]),
        None => strip_blocks(html, &["nav", "header", "footer", "aside", "form", "script", "style", "noscript"]),
    };
    let (text, t2) = html_to_text(&body);
    (text, title.or(t2))
}

/// The targets of `<a href>` (and `<base href>`) in a page.
fn links(html: &str, page: &Url) -> Vec<Url> {
    let low = html.to_ascii_lowercase();
    let attr = |tag_end: usize, at: usize| -> Option<String> {
        let rest = &html[at..tag_end];
        let lrest = &low[at..tag_end];
        let h = find_attr(lrest, "href")?;
        let v = rest[h..].trim_start();
        let (q, v) = match v.chars().next()? {
            '"' => ('"', &v[1..]),
            '\'' => ('\'', &v[1..]),
            _ => (' ', v),
        };
        let end = v.find(|c: char| c == q || (q == ' ' && (c.is_whitespace() || c == '>'))).unwrap_or(v.len());
        Some(crate::extract::decode_entities(v[..end].trim()))
    };
    let mut base = page.clone();
    if let Some(b) = find_tag(&low, "<base", 0) {
        if let Some(e) = low[b..].find('>') {
            if let Some(href) = attr(b + e, b) {
                if let Ok(u) = page.join(&href) {
                    base = u;
                }
            }
        }
    }
    let mut out = vec![];
    let mut i = 0;
    while let Some(a) = find_tag(&low, "<a", i) {
        let Some(e) = low[a..].find('>') else { break };
        if let Some(href) = attr(a + e, a) {
            let lh = href.to_lowercase();
            if !(lh.is_empty() || lh.starts_with('#') || lh.starts_with("javascript:") || lh.starts_with("mailto:") || lh.starts_with("tel:") || lh.starts_with("data:")) {
                if let Ok(u) = base.join(&href) {
                    if matches!(u.scheme(), "http" | "https") {
                        out.push(u);
                    }
                }
            }
        }
        i = a + e;
    }
    out
}

/// Position just after `name=` in a lower-cased tag.
fn find_attr(ltag: &str, name: &str) -> Option<usize> {
    let mut i = 0;
    while let Some(f) = ltag[i..].find(name) {
        let at = i + f;
        let before = ltag.as_bytes().get(at.wrapping_sub(1)).copied().unwrap_or(b' ');
        let rest = ltag[at + name.len()..].trim_start();
        if (before.is_ascii_whitespace() || at == 0) && rest.starts_with('=') {
            let eq = ltag.len() - rest.len();
            return Some(eq + 1);
        }
        i = at + name.len();
    }
    None
}

fn file_name_for(u: &Url, dir: &Path) -> PathBuf {
    let last = u.path_segments().and_then(|mut s| s.next_back()).unwrap_or("").to_string();
    let last = percent_decode(&last);
    let clean: String = last.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '-' } else { c }).collect();
    let clean = if clean.trim().is_empty() { "document".to_string() } else { clean.chars().take(120).collect() };
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (clean.clone(), String::new()),
    };
    let mut p = dir.join(&clean);
    let mut n = 2;
    while p.exists() {
        p = dir.join(format!("{stem} {n}{ext}"));
        n += 1;
    }
    p
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

async fn read_capped(resp: reqwest::Response, max: usize) -> Result<Vec<u8>, String> {
    let mut resp = resp;
    let mut buf = vec![];
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        buf.extend_from_slice(&chunk);
        if buf.len() > max {
            return Err(tr("túl nagy", "too large"));
        }
    }
    Ok(buf)
}

// ───────────────────────── crawling ─────────────────────────

pub async fn crawl(o: CrawlOpts, out_dir: &Path, emit: impl Fn(WebProgress)) -> Result<CrawlResult, String> {
    CANCEL.store(false, Ordering::Relaxed);
    let start = parse_start(&o.url)?;
    let mut host = host_key(&start);
    let mut prefix = prefix_of(&start);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(concat!("SkillStar/", env!("CARGO_PKG_VERSION"), " (website import)"))
        .build()
        .map_err(|e| e.to_string())?;
    let robots = match client.get(start.join("/robots.txt").map_err(|e| e.to_string())?).send().await {
        Ok(r) if r.status().is_success() => Robots::parse(&r.text().await.unwrap_or_default()),
        _ => Robots::default(),
    };
    fn in_scope(u: &Url, host: &str, prefix: &str, same_prefix: bool) -> bool {
        host_key(u) == host && (!same_prefix || u.path().starts_with(prefix) || u.path() == prefix.trim_end_matches('/'))
    }

    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let files_dir = out_dir.join(format!("{host}-{stamp}-files"));
    let mut queue: VecDeque<(Url, u32)> = VecDeque::from([(start.clone(), 0)]);
    let mut seen: HashSet<String> = HashSet::from([norm(&start)]);
    let mut texts: HashSet<u64> = HashSet::new();
    let mut pages: Vec<WebPage> = vec![];
    let mut docs: Vec<PathBuf> = vec![];
    let mut errors: Vec<String> = vec![];
    let mut done = 0usize;
    let mut stopped = false;
    let max_docs = o.max_pages.max(1);

    while let Some((url, depth)) = queue.pop_front() {
        if CANCEL.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        if pages.len() >= o.max_pages {
            break;
        }
        if done > 0 && !robots.allowed(url.path()) {
            continue;
        }
        if done > 0 && o.delay_ms > 0 {
            tokio_sleep(o.delay_ms).await;
        }
        done += 1;
        emit(WebProgress { done, queued: queue.len(), pages: pages.len(), docs: docs.len(), url: url.to_string() });
        let is_doc = is_doc_link(&url);
        if is_doc && (!o.docs || docs.len() >= max_docs) {
            continue;
        }
        let resp = match client.get(url.clone()).send().await {
            Ok(r) => r,
            Err(e) => {
                errors.push(format!("{url}: {e}"));
                continue;
            }
        };
        if !resp.status().is_success() {
            errors.push(format!("{url}: HTTP {}", resp.status().as_u16()));
            continue;
        }
        // a redirect may leave the site
        let final_url = resp.url().clone();
        if done == 1 {
            // the start page moved (http → https, another host): the site is where it landed
            host = host_key(&final_url);
            prefix = prefix_of(&final_url);
        } else if !is_doc && !in_scope(&final_url, &host, &prefix, o.same_prefix) {
            continue;
        }
        let ctype = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_lowercase();
        let html_like = ctype.contains("html") || (ctype.is_empty() && !is_doc);
        if !html_like {
            if !o.docs || docs.len() >= max_docs || !(is_doc || ctype.contains("pdf") || ctype.contains("epub")) {
                continue;
            }
            match read_capped(resp, MAX_DOC).await {
                Ok(bytes) => {
                    let _ = std::fs::create_dir_all(&files_dir);
                    let mut p = file_name_for(&final_url, &files_dir);
                    if p.extension().is_none() && ctype.contains("pdf") {
                        p.set_extension("pdf");
                    }
                    if kind_of(&p).is_none() {
                        continue;
                    }
                    match std::fs::write(&p, &bytes) {
                        Ok(()) => docs.push(p),
                        Err(e) => errors.push(format!("{url}: {e}")),
                    }
                }
                Err(e) => errors.push(format!("{url}: {e}")),
            }
            continue;
        }
        let cs = ctype.split("charset=").nth(1).map(|s| s.trim().trim_matches('"').to_string());
        let bytes = match read_capped(resp, MAX_HTML).await {
            Ok(b) => b,
            Err(e) => {
                errors.push(format!("{url}: {e}"));
                continue;
            }
        };
        let html = decode_html(&bytes, cs.as_deref());
        // links first: a page with little text may still lead further
        if depth < o.depth || o.docs {
            for l in links(&html, &final_url) {
                let key = norm(&l);
                if seen.contains(&key) {
                    continue;
                }
                let doc = is_doc_link(&l);
                let ok = if doc { o.docs && host_key(&l) == host } else { depth < o.depth && in_scope(&l, &host, &prefix, o.same_prefix) && !SKIP_EXT.contains(&ext_of(&l).as_str()) };
                if ok {
                    seen.insert(key);
                    let mut l = l;
                    l.set_fragment(None);
                    queue.push_back((l, depth + 1));
                }
            }
        }
        let (text, title) = main_text(&html);
        if text.chars().count() < MIN_TEXT {
            continue;
        }
        let h = {
            use std::hash::{Hash, Hasher};
            let mut s = std::collections::hash_map::DefaultHasher::new();
            text.hash(&mut s);
            s.finish()
        };
        if !texts.insert(h) {
            continue;
        }
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| final_url.path().to_string());
        pages.push(WebPage { url: norm(&final_url), title, text, depth });
    }
    emit(WebProgress { done, queued: queue.len(), pages: pages.len(), docs: docs.len(), url: String::new() });

    let n = pages.len();
    let archive = if pages.is_empty() {
        None
    } else {
        std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        let a = Archive {
            url: start.to_string(),
            title: pages[0].title.clone(),
            depth: o.depth,
            fetched: chrono::Utc::now().timestamp_millis(),
            pages,
        };
        let p = out_dir.join(format!("{host}-{stamp}.skweb"));
        std::fs::write(&p, serde_json::to_vec_pretty(&a).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        Some(p)
    };
    Ok(CrawlResult { archive, pages: n, docs, errors, stopped })
}

async fn tokio_sleep(ms: u64) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(std::time::Duration::from_millis(ms))).await;
}

fn read_archive(path: &Path) -> Result<Archive, String> {
    let b = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&b).map_err(|e| format!("{}: {e}", tr("Hibás webhely-fájl", "Damaged website file")))
}

/// A `.skweb` file → pages: every web page starts a new page, with its address on top.
pub fn extract(path: &Path) -> Result<Extracted, String> {
    let a = read_archive(path)?;
    let src = tr("Forrás", "Source");
    let chapters = a
        .pages
        .iter()
        .map(|p| {
            // the page's own first heading stays the title line; the address goes under it
            let text = match p.text.strip_prefix("# ").and_then(|t| t.split_once('\n')) {
                Some((h1, rest)) => format!("# {h1}\n\n{src}: <{}>\n\n{}", p.url, rest.trim_start()),
                None => format!("# {}\n\n{src}: <{}>\n\n{}", p.title, p.url, p.text),
            };
            (Some(p.title.clone()), text)
        })
        .collect();
    let site = Url::parse(&a.url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_default();
    let title = if site.is_empty() { a.title.clone() } else { format!("{} ({site})", a.title) };
    Ok(crate::ebook::from_chapters(chapters, &[], Some(title)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn robots_rules() {
        let r = Robots::parse("User-agent: Googlebot\nDisallow: /\n\nUser-agent: *\nDisallow: /private/\nAllow: /private/ok\nDisallow: /*.pdf$\n");
        assert!(r.allowed("/docs/a.html"));
        assert!(!r.allowed("/private/x"));
        assert!(r.allowed("/private/ok/1"));
        assert!(!r.allowed("/files/a.pdf"));
        assert!(r.allowed("/files/a.pdf?x=1"));
    }

    #[test]
    fn link_extraction() {
        let page = Url::parse("https://ex.com/docs/a/index.html").unwrap();
        let html = r#"<a href="b.html">b</a> <A HREF='../c.html#top'>c</A> <a class=x href=/d.pdf>d</a> <a href="mailto:x@y">m</a> <a name="n">n</a>"#;
        let l: Vec<String> = links(html, &page).iter().map(|u| u.to_string()).collect();
        assert_eq!(l, vec!["https://ex.com/docs/a/b.html", "https://ex.com/docs/c.html#top", "https://ex.com/d.pdf"]);
        assert!(is_doc_link(&Url::parse("https://ex.com/d.pdf").unwrap()));
        assert!(!is_doc_link(&Url::parse("https://ex.com/d.html").unwrap()));
        assert_eq!(prefix_of(&page), "/docs/a/");
    }

    #[test]
    fn main_content() {
        let html = "<html><head><title>T</title></head><body><nav><a href=x>Menu</a></nav><header>Site</header><main><h1>Doc</h1><p>Body text</p><aside>ad</aside></main><footer>(c)</footer></body></html>";
        let (t, title) = main_text(html);
        assert_eq!(title.as_deref(), Some("T"));
        assert_eq!(t, "# Doc\n\nBody text");
        let html = "<body><nav>Menu<nav>inner</nav>more</nav><div><p>Only this</p></div><footer>f</footer></body>";
        assert_eq!(main_text(html).0, "Only this");
    }
}

#[cfg(test)]
mod live {
    /// SKILLSTAR_CRAWL="https://example.org/docs/" cargo test --lib web::live -- --ignored --nocapture
    #[test]
    #[ignore]
    fn crawl_site() {
        let url = std::env::var("SKILLSTAR_CRAWL").unwrap_or_default();
        let dir = std::env::temp_dir().join("skillstar-crawl-test");
        let o = super::CrawlOpts { url, depth: 1, max_pages: 6, same_prefix: true, docs: false, delay_ms: 300 };
        let r = tauri::async_runtime::block_on(super::crawl(o, &dir, |p| println!("  {} / queued {} : {}", p.done, p.queued, p.url))).unwrap();
        println!("pages {} docs {} errors {:?} stopped {}", r.pages, r.docs.len(), r.errors, r.stopped);
        let x = super::extract(r.archive.as_ref().unwrap()).unwrap();
        println!("title {:?}, {} pages, outline {:?}", x.title, x.pages.len(), x.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>());
        println!("first page:\n{}", x.pages[0].text.chars().take(500).collect::<String>());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
