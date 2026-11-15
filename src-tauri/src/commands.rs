//! Tauri commands: everything the UI can ask from the backend.

use crate::db;
use crate::settings::{tr, Settings, SettingsStore};
use crate::{ai, ocr, pdf, process, rag, skill, vector};
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use tauri::{Emitter, State};

pub struct AppState {
    pub settings: SettingsStore,
    /// skillstar.db in the app data folder
    pub db_path: PathBuf,
    pub db: Mutex<Connection>,
    /// app data folder (downloaded pdfium lives here)
    pub data_dir: PathBuf,
    /// Documents/SkillStar: where packages go when no output folder is set
    pub default_out: PathBuf,
}

pub static WORKER: LazyLock<process::Worker> = LazyLock::new(process::Worker::default);

impl AppState {
    pub fn pdfium(&self, app: &tauri::AppHandle) -> Option<PathBuf> {
        use tauri::Manager;
        let res = app.path().resource_dir().ok();
        pdf::find(&self.settings.get().pdfium_path, res.as_deref(), &self.data_dir)
    }

    fn with_db<T>(&self, f: impl FnOnce(&mut Connection) -> Result<T, String>) -> Result<T, String> {
        let mut c = self.db.lock().map_err(|e| e.to_string())?;
        f(&mut c)
    }
}

pub fn ocr_cfg(s: &Settings) -> ocr::OcrCfg {
    ocr::OcrCfg { engine: s.ocr_engine.clone(), lang: s.ocr_lang.clone(), tesseract: s.tesseract_path.clone(), tesseract_lang: s.tesseract_lang.clone() }
}

/// OCR settings of one document: its own engine + language ("win:<tag>" / "tess:<langs>") over the global ones.
/// OCR switched off in the settings stays off.
pub fn ocr_cfg_doc(s: &Settings, doc_lang: &str) -> ocr::OcrCfg {
    let mut c = ocr_cfg(s);
    if c.engine == "off" {
        return c;
    }
    if let Some(tag) = doc_lang.strip_prefix("win:").filter(|t| !t.trim().is_empty()) {
        c.engine = "windows".into();
        c.lang = tag.trim().to_string();
    } else if let Some(langs) = doc_lang.strip_prefix("tess:").filter(|t| !t.trim().is_empty()) {
        c.engine = "tesseract".into();
        c.tesseract_lang = langs.trim().to_string();
    }
    c
}

async fn blocking<T, F>(f: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

// ───────────────────────────── settings ─────────────────────────────

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Settings {
    state.settings.get()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    state.settings.save(settings)
}

/// The UI tells the backend its language (for error messages).
#[tauri::command]
pub fn set_ui_language(lang: String) {
    crate::settings::set_english(lang == "en");
}

#[tauri::command]
pub fn default_out_dir(state: State<'_, AppState>) -> String {
    state.default_out.to_string_lossy().into_owned()
}

// ───────────────────────────── skills ─────────────────────────────

#[tauri::command]
pub fn list_skills(state: State<'_, AppState>) -> Result<Vec<db::Skill>, String> {
    state.with_db(|c| db::skills(c))
}

#[tauri::command]
pub fn get_skill(state: State<'_, AppState>, id: i64) -> Result<db::Skill, String> {
    state.with_db(|c| db::skill(c, id))
}

#[tauri::command]
pub fn create_skill(state: State<'_, AppState>, title: String, language: String) -> Result<i64, String> {
    if title.trim().is_empty() {
        return Err(tr("Adj nevet a skillnek.", "Give the skill a name."));
    }
    let author = state.settings.get().author;
    state.with_db(|c| db::create_skill(c, &title, &author, &language))
}

#[tauri::command]
pub fn update_skill(state: State<'_, AppState>, skill: db::Skill) -> Result<db::Skill, String> {
    if skill.title.trim().is_empty() {
        return Err(tr("A skill neve nem lehet üres.", "The skill name cannot be empty."));
    }
    state.with_db(|c| {
        let name = skill::slugify(&skill.name, "");
        if name.is_empty() {
            return Err(tr("Az azonosító csak betűket, számokat és kötőjelet tartalmazhat.", "The id may contain letters, digits and hyphens only."));
        }
        if db::skills(c)?.iter().any(|s| s.id != skill.id && s.name == name) {
            return Err(tr("Ilyen azonosítójú skill már van.", "A skill with this id already exists."));
        }
        db::update_skill(c, &skill)?;
        db::skill(c, skill.id)
    })
}

#[tauri::command]
pub fn delete_skill(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let waiting = WORKER.waiting();
    let docs: Vec<i64> = state.with_db(|c| Ok(db::documents(c, id)?.into_iter().map(|d| d.id).collect()))?;
    if docs.iter().any(|d| waiting.contains(d)) {
        return Err(tr("A skill dokumentumai épp feldolgozás alatt állnak – előbb állítsd le.", "Documents of this skill are being processed – stop it first."));
    }
    state.with_db(|c| db::delete_skill(c, id))
}

// ───────────────────────────── documents ─────────────────────────────

#[tauri::command]
pub fn list_documents(state: State<'_, AppState>, skill_id: i64) -> Result<Vec<db::Document>, String> {
    state.with_db(|c| db::documents(c, skill_id))
}

#[derive(Serialize)]
pub struct AddResult {
    pub added: Vec<i64>,
    pub skipped: Vec<String>,
}

/// Collects supported files (folders are walked) and adds them to a skill.
#[tauri::command]
pub async fn add_documents(state: State<'_, AppState>, skill_id: i64, paths: Vec<String>) -> Result<AddResult, String> {
    let mut files = vec![];
    let mut skipped = vec![];
    fn walk(p: &Path, files: &mut Vec<PathBuf>, skipped: &mut Vec<String>, depth: usize) {
        if p.is_dir() {
            if depth > 8 {
                return;
            }
            let mut entries: Vec<PathBuf> = std::fs::read_dir(p).map(|r| r.filter_map(|e| e.ok().map(|e| e.path())).collect()).unwrap_or_default();
            entries.sort();
            for e in entries {
                let hidden = e.file_name().map(|n| n.to_string_lossy().starts_with(['.', '~'])).unwrap_or(false);
                if !hidden {
                    walk(&e, files, skipped, depth + 1);
                }
            }
        } else if crate::extract::kind_of(p).is_some() {
            files.push(p.to_path_buf());
        } else if depth == 0 {
            skipped.push(p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
    for p in &paths {
        walk(Path::new(p), &mut files, &mut skipped, 0);
    }
    let added = state.with_db(|c| {
        let mut added = vec![];
        for f in &files {
            if let Some(id) = db::add_document(c, skill_id, f)? {
                added.push(id);
            }
        }
        Ok(added)
    })?;
    Ok(AddResult { added, skipped })
}

#[tauri::command]
pub fn remove_documents(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    let waiting = WORKER.waiting();
    if ids.iter().any(|i| waiting.contains(i)) {
        return Err(tr("Feldolgozás alatt álló dokumentum nem távolítható el.", "A document being processed cannot be removed."));
    }
    state.with_db(|c| {
        for id in ids {
            db::delete_document(c, id)?;
        }
        Ok(())
    })
}

#[tauri::command]
pub fn update_document(state: State<'_, AppState>, id: i64, title: String, summary: String, use_when: String, ocr_mode: String, ocr_lang: Option<String>, include: bool) -> Result<db::Document, String> {
    let mode = if matches!(ocr_mode.as_str(), "auto" | "force" | "off") { ocr_mode } else { "auto".into() };
    state.with_db(|c| {
        let old = db::document(c, id)?;
        let lang = ocr_lang.unwrap_or_else(|| old.ocr_lang.clone());
        // pages of an interrupted run were made with the old OCR mode / language
        if old.ocr_mode != mode || old.ocr_lang != lang {
            db::clear_partial(c, id)?;
        }
        db::update_document_meta(c, id, &title, &summary, &use_when, &mode, &lang, include)?;
        db::document(c, id)
    })
}

#[tauri::command]
pub fn reorder_documents(state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    state.with_db(|c| db::reorder(c, &ids))
}

#[derive(Serialize)]
pub struct DocText {
    pub doc: db::Document,
    pub pages: Vec<db::Page>,
    pub outline: Vec<db::OutlineItem>,
}

#[tauri::command]
pub fn document_text(state: State<'_, AppState>, id: i64) -> Result<DocText, String> {
    state.with_db(|c| Ok(DocText { doc: db::document(c, id)?, pages: db::pages(c, id)?, outline: db::outline(c, id)? }))
}

#[tauri::command]
pub fn save_page(state: State<'_, AppState>, id: i64, no: i64, text: String) -> Result<(), String> {
    state.with_db(|c| db::set_page_text(c, id, no, &text))
}

#[tauri::command]
pub fn process_documents(app: tauri::AppHandle, state: State<'_, AppState>, ids: Vec<i64>) -> Result<(), String> {
    if ids.is_empty() {
        return Ok(());
    }
    // PDFs need pdfium: say it before queueing anything
    let needs_pdf = state.with_db(|c| Ok(ids.iter().filter_map(|i| db::document(c, *i).ok()).any(|d| d.kind == "pdf")))?;
    if needs_pdf && state.pdfium(&app).is_none() {
        return Err("\u{1}pdfium".into());
    }
    WORKER.enqueue(app, state.db_path.clone(), ids)
}

#[tauri::command]
pub fn cancel_processing(state: State<'_, AppState>) {
    WORKER.cancel(&state.db_path);
}

#[tauri::command]
pub fn processing_queue() -> Vec<i64> {
    WORKER.waiting()
}

#[tauri::command]
pub fn search(state: State<'_, AppState>, skill_id: i64, query: String, limit: Option<i64>) -> Result<Vec<db::Hit>, String> {
    state.with_db(|c| db::search(c, skill_id, &query, limit.unwrap_or(50).clamp(1, 500)))
}

// ───────────────────────────── build ─────────────────────────────

#[tauri::command]
pub async fn build_skill(state: State<'_, AppState>, id: i64) -> Result<skill::BuildResult, String> {
    let s = state.settings.get();
    let pkg = state.with_db(|c| skill::package(c, id, &s.author))?;
    let sk = state.with_db(|c| db::skill(c, id))?;
    let root = skill::out_root(&s.output_dir, &sk.out_dir, &state.default_out);
    let res = blocking(move || {
        std::fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
        skill::write(&pkg, &root)
    })
    .await?;
    state.with_db(|c| db::set_built(c, id))?;
    Ok(res)
}

/// The generated files without writing anything (preview in the UI).
#[derive(Serialize)]
pub struct PreviewFile {
    pub path: String,
    pub size: usize,
    pub text: String,
}

#[tauri::command]
pub fn preview_skill(state: State<'_, AppState>, id: i64) -> Result<Vec<PreviewFile>, String> {
    let author = state.settings.get().author;
    let pkg = state.with_db(|c| skill::package(c, id, &author))?;
    Ok(pkg
        .files
        .into_iter()
        .map(|(path, text)| {
            let text = text.to_string();
            let size = text.len();
            // big sources are shown only in part
            let text = if size > 200_000 {
                let mut end = 200_000;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}\n\n… ({} KB)", &text[..end], size / 1024)
            } else {
                text
            };
            PreviewFile { path, size, text }
        })
        .collect())
}

#[tauri::command]
pub fn skill_out_dir(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let s = state.settings.get();
    let sk = state.with_db(|c| db::skill(c, id))?;
    Ok(skill::out_root(&s.output_dir, &sk.out_dir, &state.default_out).join(&sk.name).to_string_lossy().into_owned())
}

// ───────────────────────────── page images ─────────────────────────────

#[derive(Serialize)]
pub struct PageImage {
    /// data: URL of the picture
    pub url: String,
    pub page: i64,
    pub pages: i64,
}

/// The original look of a page: a PDF page rendered by pdfium, or the image file itself.
#[tauri::command]
pub async fn page_image(app: tauri::AppHandle, state: State<'_, AppState>, id: i64, page: i64, width: Option<u32>) -> Result<PageImage, String> {
    use base64::Engine;
    let doc = state.with_db(|c| db::document(c, id))?;
    let path = std::path::PathBuf::from(&doc.path);
    if !path.is_file() {
        return Err(format!("{}: {}", tr("Az eredeti fájl nem található", "The original file is missing"), doc.path));
    }
    let b64 = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    match doc.kind.as_str() {
        "pdf" => {
            let lib = state.pdfium(&app).ok_or_else(|| tr("A PDF megjelenítéséhez pdfium kell (Beállítások → Szövegkinyerés).", "Showing the PDF needs pdfium (Settings → Extraction)."))?;
            let w = width.unwrap_or(1400);
            let (jpg, pages) = blocking(move || pdf::render_page(&lib, &path, page, w)).await?;
            Ok(PageImage { url: format!("data:image/jpeg;base64,{}", b64(&jpg)), page: page.clamp(1, pages), pages })
        }
        "image" => {
            let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let mime = match ext.as_str() {
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "bmp" => "image/bmp",
                "webp" => "image/webp",
                "tif" | "tiff" => "image/tiff",
                _ => "image/png",
            };
            let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
            Ok(PageImage { url: format!("data:{mime};base64,{}", b64(&bytes)), page: 1, pages: 1 })
        }
        _ => Err(tr("Ennek a dokumentumnak nincs oldalképe (csak PDF-nél és képnél).", "This document has no page image (PDFs and images only).")),
    }
}

// ───────────────────────────── OCR / pdfium ─────────────────────────────

#[derive(Serialize)]
pub struct EngineStatus {
    pub ocr: ocr::Status,
    pub pdfium: Option<String>,
    pub pdfium_target: String,
}

#[tauri::command]
pub async fn engine_status(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<EngineStatus, String> {
    let cfg = ocr_cfg(&state.settings.get());
    let pdfium = state.pdfium(&app).map(|p| p.to_string_lossy().into_owned());
    let target = state.data_dir.join("pdfium").join(pdf::lib_name()).to_string_lossy().into_owned();
    let ocr = blocking(move || Ok(ocr::status(&cfg))).await?;
    Ok(EngineStatus { ocr, pdfium, pdfium_target: target })
}

#[tauri::command]
pub async fn pdfium_download(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<String, String> {
    let dir = state.data_dir.clone();
    let path = pdf::download(&dir, move |got, total| {
        let _ = app.emit("pdfium-progress", serde_json::json!({ "received": got, "total": total }));
    })
    .await?;
    Ok(path.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn pdfium_test(path: String) -> Result<String, String> {
    blocking(move || {
        let p = PathBuf::from(&path);
        let p = if p.is_dir() { p.join(pdf::lib_name()) } else { p };
        pdf::test(&p)
    })
    .await
}

// ───────────────────────────── shell helpers ─────────────────────────────

use std::process::Command as Proc;

fn spawn(cmd: &mut Proc) -> Result<(), String> {
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Shows a file (selected) or a folder in the system file manager.
#[tauri::command]
pub fn reveal_in_explorer(path: String) -> Result<(), String> {
    let is_file = Path::new(&path).is_file();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // explorer.exe has its own parser: the quotes must wrap only the path
        let p = path.replace('/', "\\");
        let mut cmd = Proc::new("explorer");
        if is_file { cmd.raw_arg(format!("/select,\"{p}\"")) } else { cmd.raw_arg(format!("\"{p}\"")) };
        spawn(&mut cmd)
    }
    #[cfg(target_os = "macos")]
    {
        let mut cmd = Proc::new("open");
        if is_file { cmd.args(["-R", &path]) } else { cmd.arg(&path) };
        spawn(&mut cmd)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let dir = if is_file { Path::new(&path).parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or(path) } else { path };
        spawn(Proc::new("xdg-open").arg(dir))
    }
}

/// Opens a file with its default application.
#[tauri::command]
pub fn open_file(path: String) -> Result<(), String> {
    #[cfg(windows)]
    return spawn(Proc::new("explorer").arg(path.replace('/', "\\")));
    #[cfg(target_os = "macos")]
    return spawn(Proc::new("open").arg(&path));
    #[cfg(not(any(windows, target_os = "macos")))]
    return spawn(Proc::new("xdg-open").arg(&path));
}

/// Opens a web page in the default browser (http(s) only).
#[tauri::command]
pub fn open_url(url: String) -> Result<(), String> {
    // the only non-web address: the Windows language settings (installing an OCR language)
    let settings_page = url == "ms-settings:regionlanguage";
    if !(settings_page || url.starts_with("https://") || url.starts_with("http://")) || url.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err(tr("Érvénytelen webcím.", "Invalid web address."));
    }
    #[cfg(windows)]
    return spawn(Proc::new("rundll32").args(["url.dll,FileProtocolHandler", &url]));
    #[cfg(target_os = "macos")]
    return spawn(Proc::new("open").arg(&url));
    #[cfg(not(any(windows, target_os = "macos")))]
    return spawn(Proc::new("xdg-open").arg(&url));
}

#[tauri::command]
pub fn exe_path() -> String {
    std::env::current_exe().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

// ───────────────────────────── AI ─────────────────────────────

/// Earlier questions and answers of the AI panel, for follow-up questions.
#[derive(serde::Deserialize, Default)]
pub struct Turn {
    pub q: String,
    pub a: String,
}

fn ai_check(cfg: &Settings) -> Result<(), String> {
    if !cfg.ai_enabled {
        return Err(tr("Az AI funkciók ki vannak kapcsolva (Beállítások → AI).", "The AI features are switched off (Settings → AI)."));
    }
    Ok(())
}

async fn ai_run(cfg: &Settings, system: String, user: String, max_tokens: u32) -> Result<String, String> {
    ai::complete(ai::AiRequest {
        provider: &cfg.ai_provider,
        api_key: cfg.ai_key(),
        model: cfg.ai_model_id(),
        system,
        user,
        effort: "medium",
        max_tokens,
    })
    .await
}

/// Text sample of a document for the AI: the start, then pages spread over the rest.
fn sample(pages: &[db::Page], budget: usize) -> (String, bool) {
    let total: usize = pages.iter().map(|p| p.text.len() + 20).sum();
    let mut out = String::new();
    if total <= budget {
        for p in pages {
            out.push_str(&format!("--- PAGE {} ---\n{}\n", p.no, p.text));
        }
        return (out, false);
    }
    let head = budget * 6 / 10;
    let mut i = 0;
    while i < pages.len() && out.len() < head {
        out.push_str(&format!("--- PAGE {} ---\n{}\n", pages[i].no, pages[i].text));
        i += 1;
    }
    let rest = &pages[i..];
    if !rest.is_empty() {
        let step = (rest.len() / 12).max(1);
        for p in rest.iter().step_by(step) {
            if out.len() >= budget {
                break;
            }
            let t: String = p.text.chars().take(2500).collect();
            out.push_str(&format!("--- PAGE {} ---\n{}\n", p.no, t));
        }
    }
    let mut end = budget.min(out.len());
    while !out.is_char_boundary(end) {
        end -= 1;
    }
    out.truncate(end);
    (out, true)
}

#[derive(Serialize)]
pub struct Described {
    pub title: String,
    pub summary: String,
    pub use_when: String,
    pub raw: String,
}

fn field(text: &str, key: &str) -> String {
    let mut out = String::new();
    let mut on = false;
    for line in text.lines() {
        let t = line.trim().trim_start_matches(['*', '#', ' ']);
        let upper = t.to_uppercase();
        if let Some(rest) = ["TITLE:", "SUMMARY:", "USE_WHEN:"].iter().find(|k| upper.starts_with(**k)) {
            on = *rest == key;
            if on {
                out.push_str(t[rest.len()..].trim().trim_start_matches(['*', ' ']));
            }
            continue;
        }
        if on && !t.is_empty() {
            out.push(' ');
            out.push_str(t);
        }
    }
    out.trim().to_string()
}

/// AI: title, summary and "use when" of one document (not saved; the UI applies it).
#[tauri::command]
pub async fn ai_describe(state: State<'_, AppState>, id: i64) -> Result<Described, String> {
    let cfg = state.settings.get();
    ai_check(&cfg)?;
    let (doc, pages, outline, lang) = state.with_db(|c| {
        let d = db::document(c, id)?;
        let lang = db::skill(c, d.skill_id)?.language;
        Ok((d.clone(), db::pages(c, id)?, db::outline(c, id)?, lang))
    })?;
    if pages.is_empty() {
        return Err(tr("A dokumentum még nincs feldolgozva.", "The document has not been processed yet."));
    }
    let budget = if cfg.ai_provider == "deepseek" { 60_000 } else { 120_000 };
    let (text, truncated) = sample(&pages, budget);
    let mut user = format!("FILE: {}\nPAGES: {}\n", doc.name, pages.len());
    if !outline.is_empty() {
        user.push_str("OUTLINE:\n");
        for o in outline.iter().take(150) {
            user.push_str(&format!("{}- {} (p. {})\n", "  ".repeat((o.level - 1).clamp(0, 4) as usize), o.title, o.page));
        }
    }
    user.push_str(&format!("\n<document{}>\n{text}\n</document>\n", if truncated { " sampled=\"true\"" } else { "" }));
    let raw = ai_run(&cfg, ai::describe_system(&lang), user, 2000).await?;
    let d = Described { title: field(&raw, "TITLE:"), summary: field(&raw, "SUMMARY:"), use_when: field(&raw, "USE_WHEN:"), raw };
    if d.summary.is_empty() {
        return Err(format!("{}\n\n{}", tr("Váratlan AI-válasz:", "Unexpected AI answer:"), d.raw));
    }
    Ok(d)
}

/// AI: the trigger description of the whole skill.
#[tauri::command]
pub async fn ai_skill_description(state: State<'_, AppState>, id: i64) -> Result<String, String> {
    let cfg = state.settings.get();
    ai_check(&cfg)?;
    let (sk, docs, pkg) = state.with_db(|c| Ok((db::skill(c, id)?, db::documents(c, id)?, skill::package(c, id, "")?)))?;
    let mut user = format!("SKILL TITLE: {}\nEXTRA KEYWORDS: {}\n\nDOCUMENTS:\n", sk.title, sk.keywords);
    for d in docs.iter().filter(|d| d.include && d.status == "done") {
        user.push_str(&format!("- {} ({} pages){}{}\n", if d.title.is_empty() { &d.name } else { &d.title }, d.pages,
            if d.summary.is_empty() { String::new() } else { format!("\n  Summary: {}", d.summary) },
            if d.use_when.is_empty() { String::new() } else { format!("\n  Use when: {}", d.use_when) }));
    }
    if let Some((_, idx)) = pkg.files.iter().find(|f| f.0 == "knowledge/index.md") {
        if let Some(k) = idx.find("\n## ") {
            let kw: String = idx[k..].lines().filter(|l| l.starts_with("- **")).take(150).map(|l| l.split("**").nth(1).unwrap_or("")).collect::<Vec<_>>().join(", ");
            user.push_str(&format!("\nCHARACTERISTIC WORDS: {kw}\n"));
        }
    }
    let text = ai_run(&cfg, ai::skill_desc_system(&sk.language), user, 1500).await?;
    Ok(text.trim().trim_matches('"').to_string())
}

#[derive(Serialize)]
pub struct AskResult {
    /// the conversation the turn was saved into
    pub chat_id: i64,
    pub text: String,
    /// the passages the answer could cite ([n] = passages[n-1])
    pub sources: Vec<rag::Passage>,
    /// the search queries used (the question first)
    pub queries: Vec<String>,
    /// web pages found by the web search ([Wn] = web[n-1])
    pub web: Vec<ai::WebSource>,
}

/// AI in RAG mode: (AI-written extra search queries) → local full-text retrieval
/// → numbered passages → answer with [n] citations. `all`: every skill.
#[tauri::command]
pub async fn ai_ask(state: State<'_, AppState>, id: i64, question: String, history: Option<Vec<Turn>>, all: Option<bool>, expand: Option<bool>, chat: Option<i64>, web: Option<bool>) -> Result<AskResult, String> {
    let use_web = web.unwrap_or(false);
    let cfg = state.settings.get();
    ai_check(&cfg)?;
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err(tr("Írj be egy kérdést.", "Type a question."));
    }
    // the earlier turns: from the saved conversation, else what the panel sent
    let history: Vec<Turn> = match chat {
        Some(ch) => state.with_db(|c| db::chat_turns(c, ch))?.into_iter().map(|t| Turn { q: t.q, a: t.a }).collect(),
        None => history.unwrap_or_default(),
    };
    let recent: Vec<&Turn> = history.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    let mut queries = vec![question.clone()];
    if expand.unwrap_or(true) {
        let mut user = String::new();
        for t in &recent {
            user.push_str(&format!("Earlier question: {}\n", t.q));
        }
        user.push_str(&format!("Question: {question}"));
        // a failing rewrite only costs recall, the question itself is still searched
        if let Ok(text) = ai::complete(ai::AiRequest {
            provider: &cfg.ai_provider,
            api_key: cfg.ai_key(),
            model: cfg.ai_model_id(),
            system: ai::rag_queries_system(),
            user,
            effort: "low",
            max_tokens: 400,
        })
        .await
        {
            for q in rag::parse_queries(&text, 5) {
                if !queries.iter().any(|x| x.eq_ignore_ascii_case(&q)) {
                    queries.push(q);
                }
            }
        }
    }
    let skill = if all.unwrap_or(false) { 0 } else { id };
    // search by meaning too when the skill has a vector index (the model is loaded once)
    let mut vec_hits: Vec<Vec<(i64, i64, usize, f32)>> = vec![];
    let has_vectors = vector::model_present(&state.data_dir)
        && state.with_db(|c| c.query_row("SELECT EXISTS(SELECT 1 FROM vectors v JOIN documents d ON d.id = v.doc_id WHERE (?1 = 0 OR d.skill_id = ?1))", [skill], |r| r.get::<_, bool>(0)).map_err(|e| e.to_string()))?;
    if has_vectors {
        let dir = state.data_dir.clone();
        let qs = queries.clone();
        if let Ok(Ok(qv)) = tauri::async_runtime::spawn_blocking(move || vector::embed_queries(&dir, &qs)).await {
            for v in &qv {
                vec_hits.push(state.with_db(|c| vector::search(c, skill, v, 25))?);
            }
        }
    }
    let qs = queries.clone();
    let sources = state.with_db(|c| rag::retrieve(c, skill, &qs, &vec_hits, 14, 60_000))?;
    if sources.is_empty() && !use_web {
        let text = tr(
            "A keresés nem talált a kérdéshez illő oldalt a dokumentumokban. Próbáld más szavakkal, kapcsold be a „Keresés bővítése AI-val” lehetőséget, vagy keress a weben is.",
            "The search found no page that fits the question. Try other words, turn on “Expand the search with AI”, or search the web too.",
        );
        let chat_id = save_turn(&state, id, chat, &question, &text, &sources, &queries, &[])?;
        return Ok(AskResult { chat_id, text, sources, queries, web: vec![] });
    }
    let mut user = rag::context(&sources);
    if !recent.is_empty() {
        user.push_str("\n<conversation>\n");
        for t in &recent {
            let a: String = t.a.chars().take(2500).collect();
            user.push_str(&format!("Question: {}\nAnswer: {}\n\n", t.q, a));
        }
        user.push_str("</conversation>\n");
    }
    user.push_str(&format!("\nQuestion: {question}"));
    let (text, pages) = if use_web {
        ai::complete_web(
            ai::AiRequest {
                provider: &cfg.ai_provider,
                api_key: cfg.ai_key(),
                model: cfg.ai_model_id(),
                system: ai::ask_system(&cfg.ai_language, true),
                user,
                effort: "medium",
                max_tokens: 12000,
            },
            4,
        )
        .await?
    } else {
        (ai_run(&cfg, ai::ask_system(&cfg.ai_language, false), user, 8000).await?, vec![])
    };
    let chat_id = save_turn(&state, id, chat, &question, &text, &sources, &queries, &pages)?;
    Ok(AskResult { chat_id, text, sources, queries, web: pages })
}

/// Saves a question + answer into the conversation (a new one, named after the first question).
#[allow(clippy::too_many_arguments)]
fn save_turn(state: &AppState, skill_id: i64, chat: Option<i64>, q: &str, a: &str, sources: &[rag::Passage], queries: &[String], web: &[ai::WebSource]) -> Result<i64, String> {
    let src = serde_json::to_string(sources).unwrap_or_else(|_| "[]".into());
    let wb = serde_json::to_string(web).unwrap_or_else(|_| "[]".into());
    let qs = serde_json::to_string(queries).unwrap_or_else(|_| "[]".into());
    state.with_db(|c| {
        let id = match chat {
            Some(ch) => ch,
            None => {
                let mut title: String = q.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(70).collect();
                if q.chars().count() > 70 {
                    title.push('…');
                }
                db::create_chat(c, skill_id, &title)?
            }
        };
        db::add_chat_turn(c, id, q, a, &src, &qs, &wb)?;
        Ok(id)
    })
}

#[tauri::command]
pub fn vector_status(state: State<'_, AppState>, skill_id: i64) -> Result<vector::Status, String> {
    state.with_db(|c| vector::status(c, &state.data_dir, skill_id))
}

/// Builds the vector index of a skill in the background (first use downloads the model); progress: "vec-progress".
#[tauri::command]
pub fn vector_build(app: tauri::AppHandle, state: State<'_, AppState>, skill_id: i64) -> Result<(), String> {
    vector::start(app, state.db_path.clone(), state.data_dir.clone(), skill_id);
    Ok(())
}

#[tauri::command]
pub fn vector_stop() {
    vector::stop();
}

#[tauri::command]
pub fn chat_list(state: State<'_, AppState>, skill_id: i64) -> Result<Vec<db::Chat>, String> {
    state.with_db(|c| db::chats(c, skill_id))
}

#[tauri::command]
pub fn chat_turns(state: State<'_, AppState>, id: i64) -> Result<Vec<db::ChatTurn>, String> {
    state.with_db(|c| db::chat_turns(c, id))
}

#[tauri::command]
pub fn chat_rename(state: State<'_, AppState>, id: i64, title: String) -> Result<(), String> {
    state.with_db(|c| db::rename_chat(c, id, &title))
}

#[tauri::command]
pub fn chat_delete(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    state.with_db(|c| db::delete_chat(c, id))
}

/// Settings → AI → Test: a tiny request with the values typed in the dialog.
#[tauri::command]
pub async fn ai_test(provider: String, api_key: String, model: String) -> Result<String, String> {
    let started = std::time::Instant::now();
    let text = ai::complete(ai::AiRequest {
        provider: &provider,
        api_key: &api_key,
        model: &model,
        system: "You are a connection test. Answer with exactly: OK".to_string(),
        user: "Connection test. Answer with exactly: OK".to_string(),
        effort: "low",
        max_tokens: 200,
    })
    .await?;
    let first: String = text.lines().next().unwrap_or("").chars().take(80).collect();
    Ok(format!("{model} · {} ms · „{first}”", started.elapsed().as_millis()))
}

// ───────────────────────────── about ─────────────────────────────

#[derive(Serialize)]
pub struct AppInfo {
    pub version: String,
    pub tauri: String,
    pub settings_path: String,
    pub db_path: String,
    pub out_dir: String,
    pub os: String,
    pub exe: String,
}

#[tauri::command]
pub fn app_info(state: State<'_, AppState>) -> AppInfo {
    let s = state.settings.get();
    AppInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        tauri: tauri::VERSION.to_string(),
        settings_path: state.settings.path.to_string_lossy().into_owned(),
        db_path: state.db_path.to_string_lossy().into_owned(),
        out_dir: skill::out_root(&s.output_dir, "", &state.default_out).to_string_lossy().into_owned(),
        os: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        exe: exe_path(),
    }
}

// ───────────────────────────── splash screen / window ─────────────────────────────

static SPLASH_DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What is remembered of the main window: everything but the visibility
/// (the main window must stay hidden until the splash is over).
pub fn window_state_flags() -> tauri_plugin_window_state::StateFlags {
    tauri_plugin_window_state::StateFlags::all() & !tauri_plugin_window_state::StateFlags::VISIBLE
}

/// How long the splash screen is shown; the main window appears after it.
pub const SPLASH_TIME: std::time::Duration = std::time::Duration::from_millis(2200);

/// Creates the main window (hidden: `create: false` + `visible: false` in
/// tauri.conf.json), after the splash: two WebView2 windows created at the
/// same moment sometimes left the main one without a page.
pub fn create_main_window(app: &tauri::AppHandle) -> Result<(), String> {
    let cfg = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .cloned()
        .ok_or("no 'main' window in tauri.conf.json")?;
    tauri::WebviewWindowBuilder::from_config(app, &cfg)
        .and_then(|b| b.build())
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Shows the main window and closes the splash (only the first call counts).
pub fn finish_splash(app: &tauri::AppHandle) {
    use tauri::Manager;
    let Some(main) = app.get_webview_window("main") else {
        return; // keep the splash: closing the last window would quit the app
    };
    if SPLASH_DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let _ = main.show();
    {
        use tauri_plugin_window_state::WindowExt;
        let _ = main.restore_state(window_state_flags());
    }
    let _ = main.set_focus();
    if let Some(splash) = app.get_webview_window("splashscreen") {
        let _ = splash.close();
    }
}

/// Zoom of the whole page (Ctrl + / Ctrl - / Ctrl 0, Ctrl + mouse wheel).
#[tauri::command]
pub fn set_zoom(window: tauri::WebviewWindow, scale: f64) -> Result<(), String> {
    window.set_zoom(scale.clamp(0.5, 3.0)).map_err(|e| e.to_string())
}

/// Text on the system clipboard (read here: navigator.clipboard.readText()
/// makes the WebView ask the user for permission).
#[tauri::command]
pub fn clipboard_text() -> Result<String, String> {
    arboard::Clipboard::new().and_then(|mut c| c.get_text()).map_err(|e| e.to_string())
}

pub fn save_window_state(app: &tauri::AppHandle) {
    use tauri_plugin_window_state::AppHandleExt;
    let _ = app.save_window_state(window_state_flags());
}

/// Saves the window state shortly after the last move/resize.
pub fn save_window_state_later(app: &tauri::AppHandle) {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};
    static LAST: AtomicU64 = AtomicU64::new(0);
    static PENDING: AtomicBool = AtomicBool::new(false);
    let now = || SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    LAST.store(now(), Ordering::SeqCst);
    if PENDING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        while now().saturating_sub(LAST.load(Ordering::SeqCst)) < 1000 {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        PENDING.store(false, Ordering::SeqCst);
        let main = app.clone();
        let _ = app.run_on_main_thread(move || save_window_state(&main));
    });
}
