//! Background processing queue: one document at a time, progress as
//! `doc-progress` events, cancellable.

use crate::db;
use crate::extract::{self, Opts, CANCELLED};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{Emitter, Manager};

#[derive(Default)]
pub struct Worker {
    queue: Mutex<VecDeque<i64>>,
    running: AtomicBool,
    cancel: AtomicBool,
    current: Mutex<Option<i64>>,
}

fn self_db_path(app: &tauri::AppHandle) -> PathBuf {
    app.state::<crate::commands::AppState>().db_path.clone()
}

#[derive(Serialize, Clone)]
pub struct Progress {
    pub id: i64,
    pub skill_id: i64,
    /// "queued" | "processing" | "done" | "error" | "new"
    pub status: String,
    /// "start" | "page" | "ocr" | "save"
    pub stage: String,
    pub done: usize,
    pub total: usize,
    pub error: String,
    /// documents still waiting
    pub waiting: usize,
}

fn sha_of(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

impl Worker {
    pub fn waiting(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.queue.lock().unwrap().iter().copied().collect();
        if let Some(c) = *self.current.lock().unwrap() {
            v.insert(0, c);
        }
        v
    }

    pub fn cancel(&self, db_path: &Path) {
        let ids: Vec<i64> = self.queue.lock().unwrap().drain(..).collect();
        if let Ok(c) = db::open(db_path) {
            for id in ids {
                let _ = db::set_status(&c, id, "new", "");
            }
        }
        if self.running.load(Ordering::SeqCst) {
            self.cancel.store(true, Ordering::SeqCst);
        }
    }

    pub fn enqueue(&'static self, app: tauri::AppHandle, db_path: PathBuf, ids: Vec<i64>) -> Result<(), String> {
        {
            let c = db::open(&db_path)?;
            let mut q = self.queue.lock().unwrap();
            let cur = *self.current.lock().unwrap();
            for id in ids {
                if q.contains(&id) || cur == Some(id) {
                    continue;
                }
                db::set_status(&c, id, "queued", "")?;
                q.push_back(id);
                let d = db::document(&c, id)?;
                let _ = app.emit("doc-progress", Progress { id, skill_id: d.skill_id, status: "queued".into(), stage: String::new(), done: 0, total: 0, error: String::new(), waiting: q.len() });
            }
        }
        if self.running.swap(true, Ordering::SeqCst) {
            return Ok(());
        }
        self.cancel.store(false, Ordering::SeqCst);
        std::thread::spawn(move || {
            self.run(&app, &db_path);
            self.running.store(false, Ordering::SeqCst);
            *self.current.lock().unwrap() = None;
            // something queued while the loop was finishing
            if !self.queue.lock().unwrap().is_empty() {
                let ids: Vec<i64> = self.queue.lock().unwrap().drain(..).collect();
                let _ = self.enqueue(app.clone(), db_path.clone(), ids);
            }
        });
        Ok(())
    }

    fn run(&self, app: &tauri::AppHandle, db_path: &Path) {
        let Ok(mut c) = db::open(db_path) else { return };
        loop {
            let next = self.queue.lock().unwrap().pop_front();
            let Some(id) = next else { break };
            *self.current.lock().unwrap() = Some(id);
            let waiting = self.queue.lock().unwrap().len();
            let Ok(doc) = db::document(&c, id) else { continue };
            let emit = |status: &str, stage: &str, done: usize, total: usize, error: &str| {
                let _ = app.emit("doc-progress", Progress { id, skill_id: doc.skill_id, status: status.into(), stage: stage.into(), done, total, error: error.into(), waiting });
            };
            let _ = db::set_status(&c, id, "processing", "");
            emit("processing", "start", 0, 0, "");
            let result = self.process_one(app, &mut c, &doc, &mut |stage, done, total| emit("processing", stage, done, total, ""));
            match result {
                Ok(()) => emit("done", "save", 1, 1, ""),
                Err(e) if e == CANCELLED => {
                    let _ = db::set_status(&c, id, "new", "");
                    emit("new", "", 0, 0, "");
                    let rest: Vec<i64> = self.queue.lock().unwrap().drain(..).collect();
                    for r in rest {
                        let _ = db::set_status(&c, r, "new", "");
                        let _ = app.emit("doc-progress", Progress { id: r, skill_id: doc.skill_id, status: "new".into(), stage: String::new(), done: 0, total: 0, error: String::new(), waiting: 0 });
                    }
                    break;
                }
                Err(e) => {
                    let _ = db::set_status(&c, id, "error", &e);
                    emit("error", "", 0, 0, &e);
                }
            }
        }
        self.cancel.store(false, Ordering::SeqCst);
    }

    fn process_one(&self, app: &tauri::AppHandle, c: &mut rusqlite::Connection, doc: &db::Document, progress: extract::Progress) -> Result<(), String> {
        let path = PathBuf::from(&doc.path);
        let meta = std::fs::metadata(&path).map_err(|e| format!("{}: {e}", crate::settings::tr("A fájl nem olvasható", "The file cannot be read")))?;
        let state = app.state::<crate::commands::AppState>();
        let s = state.settings.get();
        let sha = sha_of(&path)?;
        let done = db::partial_pages(c, doc.id, &sha)?;
        // every finished page is kept at once (own connection: the main one is busy)
        let saver = db::open(&self_db_path(app)).ok();
        let save = |p: &extract::PageText| {
            if let Some(sc) = &saver {
                let _ = db::save_partial(sc, doc.id, &sha, p);
            }
        };
        let o = Opts {
            ocr: crate::commands::ocr_cfg_doc(&s, &doc.ocr_lang),
            ocr_mode: if s.ocr_engine == "off" && doc.ocr_mode != "off" { "off" } else { doc.ocr_mode.as_str() },
            dpi: s.ocr_dpi,
            min_chars: s.ocr_min_chars,
            pdfium: state.pdfium(app),
            cancel: &self.cancel,
            done,
            save: Some(&save),
        };
        let x = extract::extract(&path, &o, progress)?;
        o.check()?;
        db::store_result(c, doc.id, &x, &sha, meta.len() as i64, db::file_mtime(&meta))?;
        Ok(())
    }
}
