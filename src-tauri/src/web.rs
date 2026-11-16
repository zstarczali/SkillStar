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
    /// what a stopped download continues with: the links not visited yet…
    #[serde(default)]
    pub queue: Vec<Queued>,
    /// …and every address already seen (visited or queued)
    #[serde(default)]
    pub seen: Vec<String>,
    #[serde(default)]
    pub max_pages: usize,
    #[serde(default = "yes")]
    pub same_prefix: bool,
    #[serde(default)]
    pub docs: bool,
    /// every reachable page was visited (nothing to continue)
    #[serde(default)]
    pub finished: bool,
}

fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Queued {
    pub url: String,
    pub depth: u32,
}

/// What the "continue download" dialog shows.
#[derive(Serialize, Debug)]
pub struct ArchiveInfo {
    pub url: String,
    pub pages: usize,
    pub queued: usize,
    pub depth: u32,
    pub max_pages: usize,
    pub finished: bool,
}

pub fn archive_info(path: &Path) -> Result<ArchiveInfo, String> {
    let a = read_archive(path)?;
    Ok(ArchiveInfo { url: a.url, pages: a.pages.len(), queued: a.queue.len(), depth: a.depth, max_pages: a.max_pages, finished: a.finished })
}

/// Settings of a stopped download, to continue it with.
pub fn resume_opts(path: &Path, max_pages: usize) -> Result<CrawlOpts, String> {
    let a = read_archive(path)?;
    Ok(CrawlOpts { url: a.url, depth: a.depth, max_pages, same_prefix: a.same_prefix, docs: a.docs, delay_ms: 250, resume: Some(path.to_path_buf()) })
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
    /// continue this archive (its pages are kept, its queue is visited)
    pub resume: Option<PathBuf>,
}

#[derive(Serialize, Clone, Debug)]
pub struct WebProgress {
    /// the wait between two requests (the site's Crawl-delay, at least our own)
    pub delay_ms: u64,
    pub max: usize,
    pub done: usize,
    pub queued: usize,
    pub pages: usize,
    pub docs: usize,
    pub url: String,
}

pub struct CrawlResult {
    pub archive: Option<PathBuf>,
    /// pages in the archive in all
    pub pages: usize,
    /// pages downloaded now
    pub new_pages: usize,
    /// links not visited yet (a later "continue" goes on with them)
    pub remaining: usize,
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

/// Wiki / CMS links that are not content: edit, history, diff, backlinks, login,
/// export, sorting… (DokuWiki `?do=`, MediaWiki `action=`, `oldid=`, special pages).
fn is_action_link(u: &Url) -> bool {
    const KEYS: &[&str] = &["do", "action", "rev", "rev2[]", "oldid", "diff", "curid", "printable", "redlink", "returnto", "sectok", "idx", "tab_files", "tab_details", "mediado", "image", "ns", "media", "sort", "sortby", "order", "replytocom", "share", "print", "format", "veaction", "mobileaction", "useskin"];
    if u.query_pairs().any(|(k, _)| KEYS.contains(&k.to_lowercase().as_str())) {
        return true;
    }
    let path = u.path().to_lowercase();
    ["/special:", "/user:", "/talk:", "/user_talk:", "/wp-login", "/wp-admin", "/feed", "/_export/", "/lib/exe/", "/login", "/logout", "/signin", "/register"]
        .iter()
        .any(|p| path.contains(p))
}

// ───────────────────────── robots.txt ─────────────────────────

#[derive(Default)]
struct Robots {
    disallow: Vec<String>,
    allow: Vec<String>,
    /// seconds the site asks between two requests
    crawl_delay: Option<f64>,
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
                "crawl-delay" if ours => {
                    in_agents = false;
                    if let Ok(d) = v.parse::<f64>() {
                        r.crawl_delay = Some(d.clamp(0.0, 120.0));
                    }
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
    // "Disallow: folder/" is meant from the root
    let owned;
    let rule = if rule.starts_with('/') || rule.starts_with('*') {
        rule
    } else {
        owned = format!("/{rule}");
        &owned
    };
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
    // the site's Crawl-delay is kept (e.g. codebase.c64.org asks 15 s)
    let delay_ms = robots.crawl_delay.map(|d| (d * 1000.0) as u64).unwrap_or(0).max(o.delay_ms);
    fn in_scope(u: &Url, host: &str, prefix: &str, same_prefix: bool) -> bool {
        host_key(u) == host && (!same_prefix || u.path().starts_with(prefix) || u.path() == prefix.trim_end_matches('/'))
    }

    let text_hash = |text: &str| {
        use std::hash::{Hash, Hasher};
        let mut s = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut s);
        s.finish()
    };
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let mut queue: VecDeque<(Url, u32)> = VecDeque::from([(start.clone(), 0)]);
    let mut seen: HashSet<String> = HashSet::from([norm(&start)]);
    let mut texts: HashSet<u64> = HashSet::new();
    let mut pages: Vec<WebPage> = vec![];
    // the start page decides where the site is (it may redirect); not when continuing
    let mut first_is_start = true;
    let archive_path = match &o.resume {
        Some(p) => {
            let a = read_archive(p)?;
            for pg in &a.pages {
                texts.insert(text_hash(&pg.text));
                seen.insert(pg.url.clone());
            }
            seen.extend(a.seen.iter().cloned());
            pages = a.pages;
            let q: VecDeque<(Url, u32)> = a.queue.iter().filter_map(|x| Url::parse(&x.url).ok().map(|u| (u, x.depth))).collect();
            if !q.is_empty() || a.finished {
                // a finished download has nothing left to visit
                queue = q;
                first_is_start = false;
            }
            // an older archive without a saved queue: the site is walked again from the start
            // (the pages already there are not added twice)
            p.clone()
        }
        None => out_dir.join(format!("{host}-{stamp}.skweb")),
    };
    let stem = archive_path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{host}-{stamp}"));
    let files_dir = archive_path.parent().unwrap_or(out_dir).join(format!("{stem}-files"));
    let pages_before = pages.len();
    // the archive is written after every page: a stop, an error or a crash loses nothing
    let save = |pages: &[WebPage], queue: &VecDeque<(Url, u32)>, seen: &HashSet<String>, finished: bool| -> Result<(), String> {
        if pages.is_empty() {
            return Ok(());
        }
        std::fs::create_dir_all(archive_path.parent().unwrap_or(out_dir)).map_err(|e| e.to_string())?;
        let a = Archive {
            url: start.to_string(),
            title: pages[0].title.clone(),
            depth: o.depth,
            fetched: chrono::Utc::now().timestamp_millis(),
            pages: pages.to_vec(),
            queue: queue.iter().map(|(u, d)| Queued { url: u.to_string(), depth: *d }).collect(),
            seen: seen.iter().cloned().collect(),
            max_pages: o.max_pages,
            same_prefix: o.same_prefix,
            docs: o.docs,
            finished,
        };
        let tmp = archive_path.with_extension("skweb.tmp");
        std::fs::write(&tmp, serde_json::to_vec(&a).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &archive_path).map_err(|e| e.to_string())
    };
    let mut docs: Vec<PathBuf> = vec![];
    let mut errors: Vec<String> = vec![];
    let mut done = 0usize;
    let mut stopped = false;
    let max_docs = o.max_pages.max(1);
    // connection errors in a row: the server stopped answering (often rate limiting)
    let mut failures = 0usize;

    while let Some((url, depth)) = queue.pop_front() {
        if CANCEL.load(Ordering::Relaxed) {
            stopped = true;
            break;
        }
        if pages.len() >= o.max_pages {
            // not visited: kept for a later "continue"
            queue.push_front((url, depth));
            break;
        }
        if done > 0 && !robots.allowed(url.path()) {
            continue;
        }
        if done > 0 && delay_ms > 0 && !sleep_cancellable(delay_ms).await {
            queue.push_front((url, depth));
            stopped = true;
            break;
        }
        done += 1;
        emit(WebProgress { delay_ms, max: o.max_pages, done, queued: queue.len(), pages: pages.len(), docs: docs.len(), url: url.to_string() });
        let is_doc = is_doc_link(&url);
        if is_doc && (!o.docs || docs.len() >= max_docs) {
            continue;
        }
        let resp = match fetch(&client, &url, delay_ms).await {
            Ok(r) => {
                failures = 0;
                r
            }
            Err(e) => {
                errors.push(format!("{url}: {e}"));
                failures += 1;
                if failures >= 4 || done == 1 || CANCEL.load(Ordering::Relaxed) {
                    // tried again next time
                    queue.push_front((url.clone(), depth));
                    seen.insert(norm(&url));
                    if CANCEL.load(Ordering::Relaxed) {
                        stopped = true;
                        break;
                    }
                    errors.insert(0, tr("A webhely nem válaszol (lehet, hogy túl sok kérés miatt átmenetileg letiltott; próbáld később).", "The website does not answer (it may have blocked you for a while because of too many requests; try later)."));
                    break;
                }
                continue;
            }
        };
        if !resp.status().is_success() {
            errors.push(format!("{url}: HTTP {}", resp.status().as_u16()));
            continue;
        }
        // a redirect may leave the site
        let final_url = resp.url().clone();
        if done == 1 && first_is_start {
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
                let ok = if doc { o.docs && host_key(&l) == host } else { depth < o.depth && in_scope(&l, &host, &prefix, o.same_prefix) && !SKIP_EXT.contains(&ext_of(&l).as_str()) && !is_action_link(&l) };
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
        if !texts.insert(text_hash(&text)) {
            continue;
        }
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| final_url.path().to_string());
        pages.push(WebPage { url: norm(&final_url), title, text, depth });
        if let Err(e) = save(&pages, &queue, &seen, false) {
            errors.push(e);
        }
    }
    emit(WebProgress { delay_ms, max: o.max_pages, done, queued: queue.len(), pages: pages.len(), docs: docs.len(), url: String::new() });

    let n = pages.len();
    let finished = queue.is_empty() && !stopped;
    let archive = if pages.is_empty() {
        None
    } else {
        save(&pages, &queue, &seen, finished)?;
        Some(archive_path.clone())
    };
    Ok(CrawlResult { archive, pages: n, new_pages: n - pages_before, remaining: queue.len(), docs, errors, stopped })
}

async fn tokio_sleep(ms: u64) {
    let _ = tauri::async_runtime::spawn_blocking(move || std::thread::sleep(std::time::Duration::from_millis(ms))).await;
}

/// Waits, but stops early on web_cancel; false when cancelled.
async fn sleep_cancellable(ms: u64) -> bool {
    let mut left = ms;
    while left > 0 {
        if CANCEL.load(Ordering::Relaxed) {
            return false;
        }
        let step = left.min(200);
        tokio_sleep(step).await;
        left -= step;
    }
    !CANCEL.load(Ordering::Relaxed)
}

/// GET with retries: connection errors, 429 and 503 are tried again after a wait
/// (Retry-After when the server gives it).
async fn fetch(client: &reqwest::Client, url: &Url, delay_ms: u64) -> Result<reqwest::Response, String> {
    let mut wait = delay_ms.max(2000);
    let mut last = String::new();
    for attempt in 0..3 {
        if attempt > 0 && !sleep_cancellable(wait).await {
            return Err(tr("leállítva", "stopped"));
        }
        match client.get(url.clone()).send().await {
            Ok(r) if matches!(r.status().as_u16(), 429 | 503) => {
                let ra = r.headers().get(reqwest::header::RETRY_AFTER).and_then(|v| v.to_str().ok()).and_then(|v| v.trim().parse::<u64>().ok());
                last = format!("HTTP {}", r.status().as_u16());
                wait = ra.map(|s| s.min(120) * 1000).unwrap_or(wait * 3);
            }
            Ok(r) => return Ok(r),
            Err(e) => {
                last = e.to_string();
                wait *= 3;
            }
        }
    }
    Err(last)
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
        let r = Robots::parse("User-agent: ClaudeBot\nDisallow: /\n\nUser-Agent: *\nCrawl-delay: 15\nDisallow: Ultimate_Tape_Archive/\n");
        assert_eq!(r.crawl_delay, Some(15.0));
        assert!(r.allowed("/doku.php"));
        assert!(!r.allowed("/Ultimate_Tape_Archive/x"));
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
        let act = |s: &str| is_action_link(&Url::parse(s).unwrap());
        assert!(act("https://codebase.c64.org/doku.php?id=base:x&do=edit"));
        assert!(act("https://codebase.c64.org/doku.php?id=base:x&rev=1234"));
        assert!(act("https://en.wikipedia.org/w/index.php?title=X&action=history"));
        assert!(act("https://en.wikipedia.org/wiki/Special:Random"));
        assert!(!act("https://codebase.c64.org/doku.php?id=base:clearing_a_section_of_memory"));
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
        let o = super::CrawlOpts { url, depth: 1, max_pages: 6, same_prefix: true, docs: false, delay_ms: 300, resume: None };
        let r = tauri::async_runtime::block_on(super::crawl(o, &dir, |p| println!("  {} / queued {} : {}", p.done, p.queued, p.url))).unwrap();
        println!("pages {} docs {} errors {:?} stopped {}", r.pages, r.docs.len(), r.errors, r.stopped);
        let x = super::extract(r.archive.as_ref().unwrap()).unwrap();
        println!("title {:?}, {} pages, outline {:?}", x.title, x.pages.len(), x.outline.iter().map(|o| o.title.as_str()).collect::<Vec<_>>());
        println!("first page:\n{}", x.pages[0].text.chars().take(500).collect::<String>());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod live_resume {
    /// SKILLSTAR_CRAWL=https://doc.rust-lang.org/book/ cargo test --lib web::live_resume -- --ignored --nocapture
    #[test]
    #[ignore]
    fn crawl_then_continue() {
        let url = std::env::var("SKILLSTAR_CRAWL").unwrap_or_default();
        let dir = std::env::temp_dir().join("skillstar-resume-test");
        let _ = std::fs::remove_dir_all(&dir);
        let o = super::CrawlOpts { url, depth: 2, max_pages: 3, same_prefix: true, docs: false, delay_ms: 200, resume: None };
        let r = tauri::async_runtime::block_on(super::crawl(o, &dir, |_| {})).unwrap();
        let p = r.archive.unwrap();
        println!("first: pages {} new {} remaining {}", r.pages, r.new_pages, r.remaining);
        let info = super::archive_info(&p).unwrap();
        println!("info: {info:?}");
        let o = super::resume_opts(&p, 6).unwrap();
        let r2 = tauri::async_runtime::block_on(super::crawl(o, &dir, |w| println!("  {} {}", w.done, w.url))).unwrap();
        println!("second: pages {} new {} remaining {} same file {}", r2.pages, r2.new_pages, r2.remaining, r2.archive.as_ref() == Some(&p));
        let a = super::read_archive(&p).unwrap();
        let mut urls: Vec<_> = a.pages.iter().map(|x| x.url.clone()).collect();
        let n = urls.len();
        urls.sort();
        urls.dedup();
        println!("{} pages, {} unique: {:?}", n, urls.len(), urls);
        assert_eq!(n, urls.len());
        assert_eq!(n, 6);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
