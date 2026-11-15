//! Persistent settings (app config dir / settings.json).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Language of backend messages (errors shown in the UI). Set by the UI.
static ENGLISH: AtomicBool = AtomicBool::new(false);

pub fn set_english(on: bool) {
    ENGLISH.store(on, Ordering::Relaxed);
}

pub fn is_english() -> bool {
    ENGLISH.load(Ordering::Relaxed)
}

/// Picks the Hungarian or the English text.
pub fn tr(hu: &str, en: &str) -> String {
    if is_english() { en.to_string() } else { hu.to_string() }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Settings {
    /// "dark" | "light" | "ai" | "retro" | "modern" | "modern-dark" | "system"
    pub theme: String,
    /// "auto" | "hu" | "en"
    pub ui_language: String,
    pub font_size: u32,
    /// Zoom of the whole UI (1.0 = 100%)
    pub zoom: f64,

    // ── skills ──
    /// Where built skill packages go; empty = Documents/SkillStar
    pub output_dir: String,
    /// Default author written into plugin.json
    pub author: String,
    /// Skill selected when the app was closed
    pub active_skill: i64,
    pub restore_session: bool,
    /// Process documents right after they are added
    pub auto_process: bool,

    // ── text extraction / OCR ──
    /// "auto" | "windows" | "tesseract" | "off"
    pub ocr_engine: String,
    /// Windows OCR language tag (e.g. "hu-HU"); empty = the user's profile languages
    pub ocr_lang: String,
    /// Tesseract executable; empty = find it on PATH / usual install folders
    pub tesseract_path: String,
    /// Tesseract languages, e.g. "hun+eng"
    pub tesseract_lang: String,
    /// Rendering resolution of PDF pages for OCR
    pub ocr_dpi: u32,
    /// A PDF page with fewer characters in its text layer is OCR'd
    pub ocr_min_chars: u32,
    /// pdfium library (pdfium.dll / libpdfium); empty = bundled or downloaded copy
    pub pdfium_path: String,

    // ── AI ──
    /// AI features are shown only when switched on
    pub ai_enabled: bool,
    /// "anthropic" | "deepseek" | "claude-cli"
    pub ai_provider: String,
    pub claude_cli_path: String,
    pub anthropic_api_key: String,
    pub ai_model: String,
    pub deepseek_api_key: String,
    pub deepseek_model: String,
    /// "hu" | "en"
    pub ai_language: String,

    // ── update ──
    pub update_server: String,
    pub update_check: bool,
    pub update_skip: String,

    /// UI layout state saved by the frontend (panel sizes, open folders...)
    pub ui_state: HashMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "dark".into(),
            ui_language: "auto".into(),
            font_size: 13,
            zoom: 1.0,
            output_dir: String::new(),
            author: String::new(),
            active_skill: 0,
            restore_session: true,
            auto_process: true,
            ocr_engine: "auto".into(),
            ocr_lang: String::new(),
            tesseract_path: String::new(),
            tesseract_lang: "hun+eng".into(),
            ocr_dpi: 220,
            ocr_min_chars: 40,
            pdfium_path: String::new(),
            ai_enabled: false,
            ai_provider: "anthropic".into(),
            claude_cli_path: String::new(),
            anthropic_api_key: String::new(),
            ai_model: "claude-opus-5-5".into(),
            deepseek_api_key: String::new(),
            deepseek_model: "deepseek-flash".into(),
            ai_language: "hu".into(),
            update_server: String::new(),
            update_check: true,
            update_skip: String::new(),
            ui_state: HashMap::new(),
        }
    }
}

impl Settings {
    /// API key of the selected AI provider (the CLI path for "claude-cli").
    pub fn ai_key(&self) -> &str {
        match self.ai_provider.as_str() {
            "deepseek" => &self.deepseek_api_key,
            "claude-cli" => &self.claude_cli_path,
            _ => &self.anthropic_api_key,
        }
    }

    /// Model id of the selected AI provider.
    pub fn ai_model_id(&self) -> &str {
        if self.ai_provider == "deepseek" { &self.deepseek_model } else { &self.ai_model }
    }
}

pub struct SettingsStore {
    pub path: PathBuf,
    pub data: Mutex<Settings>,
}

impl SettingsStore {
    pub fn load(dir: PathBuf) -> Self {
        let path = dir.join("settings.json");
        let data = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default();
        SettingsStore { path, data: Mutex::new(data) }
    }

    pub fn get(&self) -> Settings {
        self.data.lock().unwrap().clone()
    }

    pub fn save(&self, s: Settings) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?;
        // write + rename: a crash in the middle never leaves a half settings file
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, json).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        *self.data.lock().unwrap() = s;
        Ok(())
    }
}
