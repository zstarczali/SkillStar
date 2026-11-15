//! Local OCR: the OCR engine built into Windows 10/11 (Windows.Media.Ocr,
//! no install needed) or Tesseract (any OS, when installed).
//! Nothing leaves the computer.

use crate::settings::tr;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct OcrCfg {
    /// "auto" | "windows" | "tesseract" | "off"
    pub engine: String,
    /// Windows OCR language tag; empty = user profile languages
    pub lang: String,
    pub tesseract: String,
    pub tesseract_lang: String,
}

#[derive(Debug)]
pub enum OcrError {
    /// No engine / language: every page would fail the same way
    Setup(String),
    /// This one image failed
    Page(String),
}

impl std::fmt::Display for OcrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OcrError::Setup(s) | OcrError::Page(s) => f.write_str(s),
        }
    }
}

fn no_engine() -> OcrError {
    OcrError::Setup(tr(
        "Nincs használható OCR motor. Windows 10/11-en a beépített OCR működik; más rendszeren telepítsd a Tesseractot (Beállítások → Szövegkinyerés).",
        "No OCR engine is available. On Windows 10/11 the built-in OCR works; elsewhere install Tesseract (Settings → Text extraction).",
    ))
}

/// The engine that will actually run.
fn pick(cfg: &OcrCfg) -> Result<&'static str, OcrError> {
    match cfg.engine.as_str() {
        "off" => Err(OcrError::Setup(tr("Az OCR ki van kapcsolva (Beállítások → Szövegkinyerés).", "OCR is switched off (Settings → Text extraction)."))),
        "tesseract" => Ok("tesseract"),
        "windows" => {
            if cfg!(windows) {
                Ok("windows")
            } else {
                Err(no_engine())
            }
        }
        _ => {
            if cfg!(windows) && win::available() {
                Ok("windows")
            } else if find_tesseract(&cfg.tesseract).is_some() {
                Ok("tesseract")
            } else {
                Err(no_engine())
            }
        }
    }
}

/// OCR of an encoded image (PNG/JPEG/TIFF/BMP…).
pub fn ocr_png(bytes: &[u8], cfg: &OcrCfg) -> Result<String, OcrError> {
    match pick(cfg)? {
        "windows" => win::recognize(bytes, &cfg.lang),
        _ => {
            let tmp = std::env::temp_dir().join(format!("skillstar-ocr-{}-{}.png", std::process::id(), unique()));
            std::fs::write(&tmp, bytes).map_err(|e| OcrError::Page(e.to_string()))?;
            let r = tesseract(&tmp, cfg);
            let _ = std::fs::remove_file(&tmp);
            r
        }
    }
}

pub fn ocr_file(path: &Path, cfg: &OcrCfg) -> Result<String, OcrError> {
    match pick(cfg)? {
        "windows" => {
            let b = std::fs::read(path).map_err(|e| OcrError::Page(e.to_string()))?;
            win::recognize(&b, &cfg.lang)
        }
        _ => tesseract(path, cfg),
    }
}

fn unique() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)
}

// ───────────────────────── Tesseract ─────────────────────────

pub fn find_tesseract(setting: &str) -> Option<PathBuf> {
    let s = setting.trim();
    if !s.is_empty() {
        let p = PathBuf::from(s);
        return p.is_file().then_some(p);
    }
    let exe = if cfg!(windows) { "tesseract.exe" } else { "tesseract" };
    if let Some(paths) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&paths) {
            let p = d.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let mut extra: Vec<PathBuf> = vec![];
    if cfg!(windows) {
        for v in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
            if let Some(d) = std::env::var_os(v) {
                extra.push(PathBuf::from(&d).join("Tesseract-OCR").join(exe));
                extra.push(PathBuf::from(&d).join("Programs").join("Tesseract-OCR").join(exe));
            }
        }
    } else {
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"] {
            extra.push(PathBuf::from(d).join(exe));
        }
    }
    extra.into_iter().find(|p| p.is_file())
}

fn command(exe: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut c = std::process::Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    c
}

fn tesseract(image: &Path, cfg: &OcrCfg) -> Result<String, OcrError> {
    let exe = find_tesseract(&cfg.tesseract).ok_or_else(|| {
        OcrError::Setup(tr(
            "A Tesseract nem található. Telepítsd (pl. github.com/UB-Mannheim/tesseract), vagy add meg az útvonalát a Beállításokban.",
            "Tesseract was not found. Install it (e.g. github.com/UB-Mannheim/tesseract) or set its path in the Settings.",
        ))
    })?;
    let lang = if cfg.tesseract_lang.trim().is_empty() { "eng" } else { cfg.tesseract_lang.trim() };
    let out = command(&exe)
        .arg(image)
        .arg("stdout")
        .args(["-l", lang, "--psm", "3", "-c", "preserve_interword_spaces=1"])
        .output()
        .map_err(|e| OcrError::Setup(format!("Tesseract: {e}")))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        // a missing language pack fails every page
        if err.contains("Failed loading language") || err.contains("Error opening data file") {
            return Err(OcrError::Setup(format!("Tesseract ({lang}): {err}")));
        }
        return Err(OcrError::Page(format!("Tesseract: {err}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn tesseract_langs(setting: &str) -> Vec<String> {
    let Some(exe) = find_tesseract(setting) else { return vec![] };
    let Ok(out) = command(&exe).arg("--list-langs").output() else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    text.lines().skip_while(|l| !l.contains("List of")).skip(1).map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && l != "osd").collect()
}

// ───────────────────────── status for the UI ─────────────────────────

#[derive(Serialize)]
pub struct Lang {
    pub tag: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct Status {
    pub windows: bool,
    pub windows_langs: Vec<Lang>,
    pub tesseract: Option<String>,
    pub tesseract_langs: Vec<String>,
    /// the engine "auto" would use (empty = none)
    pub active: String,
}

pub fn status(cfg: &OcrCfg) -> Status {
    let windows_langs = win::languages();
    let tess = find_tesseract(&cfg.tesseract);
    Status {
        windows: cfg!(windows) && !windows_langs.is_empty(),
        windows_langs,
        tesseract: tess.as_ref().map(|p| p.to_string_lossy().into_owned()),
        tesseract_langs: if tess.is_some() { tesseract_langs(&cfg.tesseract) } else { vec![] },
        active: pick(cfg).map(|s| s.to_string()).unwrap_or_default(),
    }
}

// ───────────────────────── Windows.Media.Ocr ─────────────────────────

#[cfg(windows)]
mod win {
    use super::{Lang, OcrError};
    use crate::settings::tr;
    use windows::core::HSTRING;
    use windows::Globalization::Language;
    use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapDecoder, BitmapInterpolationMode, BitmapPixelFormat, BitmapTransform, ColorManagementMode, ExifOrientationMode, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::{DataWriter, InMemoryRandomAccessStream};

    /// WinRT needs an initialised apartment on the calling thread.
    fn init() {
        thread_local!(static DONE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
        DONE.with(|d| {
            if !d.get() {
                unsafe {
                    let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
                }
                d.set(true);
            }
        });
    }

    pub fn available() -> bool {
        !languages().is_empty()
    }

    pub fn languages() -> Vec<Lang> {
        init();
        let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else { return vec![] };
        list.into_iter()
            .filter_map(|l| Some(Lang { tag: l.LanguageTag().ok()?.to_string(), name: l.NativeName().or_else(|_| l.DisplayName()).ok()?.to_string() }))
            .collect()
    }

    fn engine(lang: &str) -> Result<OcrEngine, OcrError> {
        let lang = lang.trim();
        let e = if lang.is_empty() {
            OcrEngine::TryCreateFromUserProfileLanguages()
        } else {
            let l = Language::CreateLanguage(&HSTRING::from(lang)).map_err(|e| OcrError::Setup(e.to_string()))?;
            if !OcrEngine::IsLanguageSupported(&l).unwrap_or(false) {
                return Err(OcrError::Setup(format!(
                    "{} „{lang}”. {}",
                    tr("A Windows OCR nem ismeri ezt a nyelvet:", "Windows OCR does not support the language"),
                    tr(
                        "Telepítsd: Gépház → Idő és nyelv → Nyelv és régió → nyelv hozzáadása (az „Optikai karakterfelismerés” résszel), vagy válassz másik nyelvet.",
                        "Install it: Settings → Time & language → Language & region → add a language (with “Optical character recognition”), or pick another language.",
                    )
                )));
            }
            OcrEngine::TryCreateFromLanguage(&l)
        };
        e.map_err(|_| super::no_engine())
    }

    pub fn recognize(bytes: &[u8], lang: &str) -> Result<String, OcrError> {
        init();
        let engine = engine(lang)?;
        let page = |e: windows::core::Error| OcrError::Page(format!("Windows OCR: {e}"));
        let stream = InMemoryRandomAccessStream::new().map_err(page)?;
        let writer = DataWriter::CreateDataWriter(&stream).map_err(page)?;
        writer.WriteBytes(bytes).map_err(page)?;
        writer.StoreAsync().map_err(page)?.get().map_err(page)?;
        writer.FlushAsync().map_err(page)?.get().map_err(page)?;
        writer.DetachStream().map_err(page)?;
        stream.Seek(0).map_err(page)?;
        let decoder = BitmapDecoder::CreateAsync(&stream).map_err(page)?.get().map_err(page)?;
        let (w, h) = (decoder.PixelWidth().map_err(page)?, decoder.PixelHeight().map_err(page)?);
        // the engine has a size limit (MaxImageDimension, usually 10000 px)
        let max = OcrEngine::MaxImageDimension().unwrap_or(10000).max(1000);
        let bitmap: SoftwareBitmap = if w > max || h > max {
            let k = max as f64 / w.max(h) as f64;
            let t = BitmapTransform::new().map_err(page)?;
            t.SetScaledWidth(((w as f64) * k) as u32).map_err(page)?;
            t.SetScaledHeight(((h as f64) * k) as u32).map_err(page)?;
            t.SetInterpolationMode(BitmapInterpolationMode::Fant).map_err(page)?;
            decoder
                .GetSoftwareBitmapTransformedAsync(BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied, &t, ExifOrientationMode::RespectExifOrientation, ColorManagementMode::DoNotColorManage)
                .map_err(page)?
                .get()
                .map_err(page)?
        } else {
            let b = decoder.GetSoftwareBitmapAsync().map_err(page)?.get().map_err(page)?;
            SoftwareBitmap::ConvertWithAlpha(&b, BitmapPixelFormat::Bgra8, BitmapAlphaMode::Premultiplied).map_err(page)?
        };
        let result = engine.RecognizeAsync(&bitmap).map_err(page)?.get().map_err(page)?;
        let mut out = String::new();
        let mut last_bottom: Option<f32> = None;
        for line in result.Lines().map_err(page)? {
            let text = line.Text().map_err(page)?.to_string();
            // a big vertical gap between lines = new paragraph
            let words = line.Words().map_err(page)?;
            let (mut top, mut bottom, mut height) = (f32::MAX, 0f32, 0f32);
            for w in &words {
                if let Ok(r) = w.BoundingRect() {
                    top = top.min(r.Y);
                    bottom = bottom.max(r.Y + r.Height);
                    height = height.max(r.Height);
                }
            }
            if let Some(lb) = last_bottom {
                if top != f32::MAX && height > 0.0 && top - lb > height * 1.2 {
                    out.push('\n');
                }
            }
            if bottom > 0.0 {
                last_bottom = Some(bottom);
            }
            out.push_str(&text);
            out.push('\n');
        }
        Ok(out)
    }
}

#[cfg(not(windows))]
mod win {
    use super::{Lang, OcrError};
    pub fn available() -> bool {
        false
    }
    pub fn languages() -> Vec<Lang> {
        vec![]
    }
    pub fn recognize(_: &[u8], _: &str) -> Result<String, OcrError> {
        Err(super::no_engine())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders a text into a PNG and reads it back with the Windows OCR.
    #[cfg(windows)]
    #[test]
    fn windows_ocr_reads_rendered_text() {
        if !win::available() {
            return;
        }
        // 5x7 bitmap font for a few letters, scaled up
        let glyphs: &[(char, [u8; 7])] = &[
            ('H', [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11]),
            ('E', [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F]),
            ('L', [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F]),
            ('O', [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E]),
        ];
        let word = "HELLO";
        let scale = 8usize;
        let (w, h) = ((word.len() * 6 + 4) * scale, 11 * scale);
        let mut px = vec![255u8; w * h];
        for (i, ch) in word.chars().enumerate() {
            let g = glyphs.iter().find(|g| g.0 == ch).unwrap().1;
            for (row, bits) in g.iter().enumerate() {
                for col in 0..5 {
                    if bits & (0x10 >> col) != 0 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                let x = (2 + i * 6 + col) * scale + dx;
                                let y = (2 + row) * scale + dy;
                                px[y * w + x] = 0;
                            }
                        }
                    }
                }
            }
        }
        let mut png = vec![];
        use image::ImageEncoder;
        image::codecs::png::PngEncoder::new(&mut png).write_image(&px, w as u32, h as u32, image::ExtendedColorType::L8).unwrap();
        let cfg = OcrCfg { engine: "windows".into(), ..Default::default() };
        let text = ocr_png(&png, &cfg).unwrap();
        assert!(text.to_uppercase().contains("HELLO"), "{text:?}");
    }
}
