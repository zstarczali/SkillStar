//! PDF through pdfium (the PDF engine of Chromium): the text layer of every
//! page, bookmarks as outline, and page images for OCR when a page has no
//! (usable) text. The pdfium library is bundled next to the app or
//! downloaded once into the app data folder.

use crate::extract::{Extracted, Opts, OutlineEntry, PageText, Progress};
use crate::settings::tr;
use pdfium_render::prelude::*;
use std::io::Read;
use std::path::{Path, PathBuf};

/// File name of the pdfium library on this platform.
pub fn lib_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// Where pdfium is looked for, in order: the path from the settings, the
/// app's resources, the folder of the executable, the downloaded copy.
pub fn candidates(setting: &str, resource_dir: Option<&Path>, data_dir: &Path) -> Vec<PathBuf> {
    let mut v = vec![];
    let s = setting.trim();
    if !s.is_empty() {
        let p = PathBuf::from(s);
        v.push(if p.is_dir() { p.join(lib_name()) } else { p });
    }
    if let Some(r) = resource_dir {
        v.push(r.join("resources").join("pdfium").join(lib_name()));
        v.push(r.join("pdfium").join(lib_name()));
        v.push(r.join(lib_name()));
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)) {
        v.push(dir.join(lib_name()));
        v.push(dir.join("resources").join("pdfium").join(lib_name()));
    }
    v.push(data_dir.join("pdfium").join(lib_name()));
    v
}

pub fn find(setting: &str, resource_dir: Option<&Path>, data_dir: &Path) -> Option<PathBuf> {
    candidates(setting, resource_dir, data_dir).into_iter().find(|p| p.is_file())
}

fn bind(lib: &Path) -> Result<Pdfium, String> {
    Pdfium::bind_to_library(lib)
        .map(Pdfium::new)
        .map_err(|e| format!("{} ({}): {e:?}", tr("A pdfium könyvtár nem tölthető be", "The pdfium library could not be loaded"), lib.display()))
}

/// Checks that the library loads; returns its path.
pub fn test(lib: &Path) -> Result<String, String> {
    bind(lib).map(|_| lib.to_string_lossy().into_owned())
}

fn no_pdfium() -> String {
    tr(
        "PDF-feldolgozáshoz a pdfium könyvtár kell. Beállítások → Szövegkinyerés → „pdfium letöltése”.",
        "Processing PDFs needs the pdfium library. Settings → Text extraction → “Download pdfium”.",
    )
}

/// Non-whitespace characters of a text.
fn visible(s: &str) -> usize {
    s.chars().filter(|c| !c.is_whitespace()).count()
}

/// A text layer that is mostly replacement / private-use characters is as bad as none.
fn garbage(s: &str) -> bool {
    let n = visible(s);
    if n == 0 {
        return true;
    }
    let bad = s.chars().filter(|&c| c == '\u{FFFD}' || ('\u{E000}'..='\u{F8FF}').contains(&c) || (c.is_control() && !c.is_whitespace())).count();
    bad * 3 > n
}

pub fn extract(path: &Path, o: &Opts, progress: Progress) -> Result<Extracted, String> {
    let lib = o.pdfium.clone().ok_or_else(no_pdfium)?;
    let pdfium = bind(&lib)?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| match e {
        PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError) => tr("A PDF jelszóval védett.", "The PDF is password protected."),
        e => format!("{}: {e:?}", tr("A PDF nem nyitható meg", "The PDF could not be opened")),
    })?;
    let title = doc.metadata().get(PdfDocumentMetadataTagType::Title).map(|t| t.value().trim().to_string()).filter(|t| !t.is_empty());

    // outline from the bookmarks
    let mut outline = vec![];
    for b in doc.bookmarks().iter() {
        let Some(t) = b.title().map(|t| t.trim().to_string()).filter(|t| !t.is_empty()) else { continue };
        let page = b.destination().and_then(|d| d.page_index().ok()).map(|i| i as i64 + 1).unwrap_or(0);
        let mut level = 1;
        let mut p = b.parent();
        while let Some(x) = p {
            level += 1;
            p = x.parent();
            if level > 8 {
                break;
            }
        }
        outline.push(OutlineEntry { level, title: t, page });
        if outline.len() > 5000 {
            break;
        }
    }

    let pages = doc.pages();
    let total = pages.len() as usize;
    let mut out = Vec::with_capacity(total);
    let scale = o.dpi.clamp(72, 600) as f32 / 72.0;
    let mut ocr_error: Option<String> = None;
    let resumed = o.done.len();
    for (i, page) in pages.iter().enumerate() {
        o.check()?;
        // finished by an earlier, interrupted run
        if let Some(p) = o.done.get(&(i as i64 + 1)) {
            out.push(p.clone());
            continue;
        }
        progress(if resumed > 0 { "resume" } else { "page" }, i, total);
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        let need = match o.ocr_mode {
            "force" => true,
            "off" => false,
            _ => visible(&text) < o.min_chars as usize || garbage(&text),
        };
        let mut page_text = PageText { no: i as i64 + 1, text: text.clone(), ocr: false };
        let mut keep = true;
        if need && ocr_error.is_none() {
            progress("ocr", i, total);
            let cfg = PdfRenderConfig::new()
                .scale_page_by_factor(scale)
                .set_maximum_width(6000)
                .set_maximum_height(8000)
                .render_form_data(true);
            match page.render_with_config(&cfg).map_err(|e| format!("{e:?}")).and_then(|bmp| png_of(&bmp)) {
                Ok(png) => match crate::ocr::ocr_png(&png, &o.ocr) {
                    // OCR text only replaces a weaker text layer
                    Ok(t) if visible(&t) > visible(&text) || o.ocr_mode == "force" || garbage(&text) => page_text = PageText { no: i as i64 + 1, text: t, ocr: true },
                    Ok(_) => {}
                    // without a working OCR engine the rest of the pages are not tried either
                    Err(crate::ocr::OcrError::Setup(e)) => {
                        ocr_error = Some(e);
                        keep = false;
                    }
                    Err(e) => eprintln!("SkillStar: OCR of page {} failed: {e}", i + 1),
                },
                Err(e) => eprintln!("SkillStar: render of page {} failed: {e}", i + 1),
            }
        } else if need {
            // OCR is broken: this page must be tried again on the next run
            keep = false;
        }
        if keep {
            if let Some(save) = o.save {
                save(&page_text);
            }
        }
        out.push(page_text);
    }
    progress("page", total, total);
    if let Some(e) = ocr_error {
        // a scanned PDF without OCR is useless: say why
        if out.iter().all(|p| visible(&p.text) < o.min_chars as usize) {
            return Err(e);
        }
    }
    Ok(Extracted { pages: out, outline, title })
}

/// The rendered page as PNG bytes.
/// One page as a JPEG for the page viewer (`page` 1-based, clamped), with the page count.
pub fn render_page(lib: &Path, path: &Path, page: i64, width: u32) -> Result<(Vec<u8>, i64), String> {
    let pdfium = bind(lib)?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| format!("{}: {e:?}", tr("A PDF nem nyitható meg", "The PDF could not be opened")))?;
    let count = doc.pages().len() as i64;
    if count == 0 {
        return Err(tr("A PDF-ben nincs oldal.", "The PDF has no pages."));
    }
    let idx = (page.clamp(1, count) - 1) as u16;
    let p = doc.pages().get(idx).map_err(|e| format!("{e:?}"))?;
    let width = width.clamp(300, 3000) as i32;
    let cfg = PdfRenderConfig::new().set_target_width(width).set_maximum_height(width * 3).render_form_data(true);
    let bmp = p.render_with_config(&cfg).map_err(|e| format!("{e:?}"))?;
    Ok((jpeg_of(&bmp)?, count))
}

/// The words of a PDF page's text layer with their boxes (fractions of the page, origin top left).
pub fn page_words(lib: &Path, path: &Path, page: i64) -> Result<Vec<crate::ocr::WordBox>, String> {
    use crate::ocr::WordBox;
    let pdfium = bind(lib)?;
    let doc = pdfium.load_pdf_from_file(path, None).map_err(|e| format!("{}: {e:?}", tr("A PDF nem nyitható meg", "The PDF could not be opened")))?;
    let count = doc.pages().len() as i64;
    let idx = (page.clamp(1, count.max(1)) - 1) as u16;
    let p = doc.pages().get(idx).map_err(|e| format!("{e:?}"))?;
    // char boxes are in the unrotated page space; the image is rendered rotated
    let rot = match p.rotation() {
        Ok(PdfPageRenderRotation::Degrees90) => 90,
        Ok(PdfPageRenderRotation::Degrees180) => 180,
        Ok(PdfPageRenderRotation::Degrees270) => 270,
        _ => 0,
    };
    let (dw, dh) = (p.width().value, p.height().value);
    let (pw, ph) = if rot == 90 || rot == 270 { (dh, dw) } else { (dw, dh) };
    if pw <= 0.0 || ph <= 0.0 {
        return Ok(vec![]);
    }
    let text = p.text().map_err(|e| format!("{e:?}"))?;
    // unrotated (u, v from the top) → displayed
    let turn = |u: f32, v: f32| -> (f32, f32) {
        match rot {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        }
    };
    let mut out: Vec<WordBox> = vec![];
    // the word being collected: text, left, bottom, right, top (points), last char centre and size
    struct Cur {
        t: String,
        l: f32,
        b: f32,
        r: f32,
        tp: f32,
        cx: f32,
        cy: f32,
        size: f32,
    }
    let mut cur: Option<Cur> = None;
    let flush = |cur: &mut Option<Cur>, out: &mut Vec<WordBox>| {
        if let Some(w) = cur.take() {
            if !w.t.trim().is_empty() {
                let (x1, y1) = turn(w.l / pw, 1.0 - w.tp / ph);
                let (x2, y2) = turn(w.r / pw, 1.0 - w.b / ph);
                out.push(WordBox { t: w.t, x: x1.min(x2), y: y1.min(y2), w: (x1 - x2).abs(), h: (y1 - y2).abs() });
            }
        }
    };
    for ch in text.chars().iter() {
        let Some(c) = ch.unicode_char() else { continue };
        if c.is_whitespace() || c == '\u{2}' || c == '\u{ad}' || c == '\u{fffe}' {
            flush(&mut cur, &mut out);
            continue;
        }
        let Ok(r) = ch.loose_bounds() else { continue };
        let (l, b, rt, tp) = (r.left().value, r.bottom().value, r.right().value, r.top().value);
        let (cx, cy) = ((l + rt) / 2.0, (b + tp) / 2.0);
        let size = (tp - b).abs().max((rt - l).abs()).max(1.0);
        match cur.as_mut() {
            // the next letter of the word: close to the previous one, in any writing direction
            Some(w) if ((cx - w.cx).powi(2) + (cy - w.cy).powi(2)).sqrt() < w.size.max(size) * 1.6 => {
                w.t.push(c);
                w.l = w.l.min(l);
                w.b = w.b.min(b);
                w.r = w.r.max(rt);
                w.tp = w.tp.max(tp);
                w.cx = cx;
                w.cy = cy;
                w.size = size;
            }
            _ => {
                flush(&mut cur, &mut out);
                cur = Some(Cur { t: c.to_string(), l, b, r: rt, tp, cx, cy, size });
            }
        }
    }
    flush(&mut cur, &mut out);
    Ok(out)
}

fn jpeg_of(bmp: &PdfBitmap) -> Result<Vec<u8>, String> {
    let w = bmp.width() as u32;
    let h = bmp.height() as u32;
    let raw = bmp.as_raw_bytes();
    let stride = if h > 0 { raw.len() / h as usize } else { 0 };
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h as usize {
        let row = &raw[y * stride..];
        for x in 0..w as usize {
            let p = &row[x * 4..x * 4 + 4];
            let a = p[3] as u32;
            // BGRA, transparent = white paper
            for c in [p[2], p[1], p[0]] {
                rgb.push(((c as u32 * a + 255 * (255 - a)) / 255) as u8);
            }
        }
    }
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .write_image(&rgb, w, h, image::ExtendedColorType::Rgb8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

fn png_of(bmp: &PdfBitmap) -> Result<Vec<u8>, String> {
    let w = bmp.width() as u32;
    let h = bmp.height() as u32;
    let raw = bmp.as_raw_bytes();
    let stride = if h > 0 { raw.len() / h as usize } else { 0 };
    // BGRA → grey: smaller PNGs and the same OCR quality
    let mut grey = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        let row = &raw[y * stride..];
        for x in 0..w as usize {
            let p = &row[x * 4..x * 4 + 4];
            let (b, g, r, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
            // transparent = white paper
            let l = (r * 299 + g * 587 + b * 114) / 1000;
            let l = (l * a + 255 * (255 - a)) / 255;
            grey.push(l as u8);
        }
    }
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(&grey, w, h, image::ExtendedColorType::L8)
        .map_err(|e| e.to_string())?;
    Ok(png)
}

use image::ImageEncoder;

/// The pdfium build for this platform on github.com/bblanchon/pdfium-binaries.
fn download_asset() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("pdfium-win-x64.tgz")
    } else if cfg!(all(windows, target_arch = "aarch64")) {
        Some("pdfium-win-arm64.tgz")
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        Some("pdfium-mac-arm64.tgz")
    } else if cfg!(target_os = "macos") {
        Some("pdfium-mac-x64.tgz")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("pdfium-linux-x64.tgz")
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some("pdfium-linux-arm64.tgz")
    } else {
        None
    }
}

pub const DOWNLOAD_BASE: &str = "https://github.com/bblanchon/pdfium-binaries/releases/latest/download/";

/// Downloads pdfium into `<data_dir>/pdfium/`; returns the library path.
pub async fn download(data_dir: &Path, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    let asset = download_asset().ok_or_else(|| tr("Ehhez a platformhoz nincs letölthető pdfium.", "No pdfium download for this platform."))?;
    let client = reqwest::Client::builder()
        .user_agent(concat!("SkillStar/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(format!("{DOWNLOAD_BASE}{asset}")).send().await.map_err(|e| format!("{}: {e}", tr("Hálózati hiba", "Network error")))?;
    if !resp.status().is_success() {
        return Err(format!("{}: HTTP {}", tr("A letöltés nem sikerült", "Download failed"), resp.status()));
    }
    let total = resp.content_length().unwrap_or(0);
    let mut data = Vec::with_capacity(total as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        data.extend_from_slice(&chunk);
        progress(data.len() as u64, total);
    }
    let dir = data_dir.join("pdfium");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(lib_name());
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(&data[..]));
    let mut found = false;
    for entry in ar.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
        if path.file_name().map(|n| n == lib_name()).unwrap_or(false) {
            let mut buf = vec![];
            entry.read_to_end(&mut buf).map_err(|e| e.to_string())?;
            // write + rename: never leave a half library behind
            let tmp = target.with_extension("part");
            std::fs::write(&tmp, &buf).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
            found = true;
            break;
        }
    }
    if !found {
        return Err(tr("A letöltött csomagban nincs pdfium könyvtár.", "The downloaded package has no pdfium library."));
    }
    bind(&target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_detection() {
        assert!(garbage(""));
        assert!(garbage("\u{FFFD}\u{FFFD}\u{FFFD}a"));
        assert!(!garbage("Rendes szöveg, ékezetekkel."));
    }

    /// Runs only where a pdfium library is at hand (SKILLSTAR_PDFIUM=path).
    #[test]
    fn pdfium_text_layer() {
        let Ok(lib) = std::env::var("SKILLSTAR_PDFIUM") else { return };
        let Ok(pdf) = std::env::var("SKILLSTAR_TEST_PDF") else { return };
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let mode = std::env::var("SKILLSTAR_OCR_MODE").unwrap_or_else(|_| "auto".into());
        let o = Opts {
            ocr: crate::ocr::OcrCfg { lang: std::env::var("SKILLSTAR_OCR_LANG").unwrap_or_default(), ..Default::default() },
            ocr_mode: &mode,
            dpi: 150,
            min_chars: 40,
            pdfium: Some(PathBuf::from(lib)),
            cancel: &cancel,
            done: Default::default(),
            save: None,
        };
        let x = extract(Path::new(&pdf), &o, &mut |_, _, _| {}).unwrap();
        assert!(!x.pages.is_empty());
        eprintln!("pages {} outline {} ocr {} title {:?}", x.pages.len(), x.outline.len(), x.pages.iter().filter(|p| p.ocr).count(), x.title);
        for o in x.outline.iter().take(8) {
            eprintln!("  {} {} p{}", "  ".repeat(o.level as usize), o.title, o.page);
        }
        let show = std::env::var("SKILLSTAR_SHOW_PAGE").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
        eprintln!("--- page {show} ---
{}", x.pages.get(show - 1).map(|p| p.text.chars().take(900).collect::<String>()).unwrap_or_default());
    }

    #[test]
    fn renders_a_page() {
        let Ok(lib) = std::env::var("SKILLSTAR_PDFIUM") else { return };
        let Ok(pdf) = std::env::var("SKILLSTAR_TEST_PDF") else { return };
        let (jpg, pages) = render_page(Path::new(&lib), Path::new(&pdf), 3, 1000).unwrap();
        assert!(pages >= 1);
        assert_eq!(&jpg[..2], &[0xFF, 0xD8]);
        // out of range pages are clamped
        assert!(render_page(Path::new(&lib), Path::new(&pdf), 99_999, 400).is_ok());
        if let Ok(out) = std::env::var("SKILLSTAR_PAGE_JPG") {
            std::fs::write(out, &jpg).unwrap();
        }
    }

    /// SKILLSTAR_PDFIUM=… SKILLSTAR_TEST_PDF=… SKILLSTAR_WORDS_PAGE=n cargo test words_of_a_page -- --nocapture
    #[test]
    fn words_of_a_page() {
        let (Ok(lib), Ok(pdf), Ok(page)) = (std::env::var("SKILLSTAR_PDFIUM"), std::env::var("SKILLSTAR_TEST_PDF"), std::env::var("SKILLSTAR_WORDS_PAGE")) else { return };
        let w = page_words(Path::new(&lib), Path::new(&pdf), page.parse().unwrap()).unwrap();
        eprintln!("{} words; first: {:?}", w.len(), w.iter().take(6).collect::<Vec<_>>());
        for x in &w {
            assert!(x.x >= -0.01 && x.x <= 1.01 && x.y >= -0.01 && x.y <= 1.01, "{x:?}");
        }
        if let Ok(js) = std::env::var("SKILLSTAR_WORDS_JSON") {
            use base64::Engine;
            let (jpg, _) = render_page(Path::new(&lib), Path::new(&pdf), page.parse().unwrap(), 900).unwrap();
            std::fs::write(js, format!("{{\"img\":\"data:image/jpeg;base64,{}\",\"words\":{}}}", base64::engine::general_purpose::STANDARD.encode(&jpg), serde_json::to_string(&w).unwrap())).unwrap();
        }
        if let Ok(html) = std::env::var("SKILLSTAR_WORDS_HTML") {
            use base64::Engine;
            let (jpg, _) = render_page(Path::new(&lib), Path::new(&pdf), page.parse().unwrap(), 900).unwrap();
            let boxes: String = w.iter().map(|b| format!("<i style='left:{}%;top:{}%;width:{}%;height:{}%'></i>", b.x * 100.0, b.y * 100.0, b.w * 100.0, b.h * 100.0)).collect();
            std::fs::write(html, format!("<div style='position:relative;width:900px'><img style='width:100%;display:block' src='data:image/jpeg;base64,{}'>{boxes}</div><style>i{{position:absolute;background:rgba(255,200,0,.35);outline:1px solid rgba(255,120,0,.6)}}</style>", base64::engine::general_purpose::STANDARD.encode(&jpg))).unwrap();
        }
    }
}
