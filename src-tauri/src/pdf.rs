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
}
