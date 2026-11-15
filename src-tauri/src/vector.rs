//! Vector (semantic) index for the RAG mode: a local multilingual embedding
//! model (multilingual-e5-small, int8 ONNX through ONNX Runtime, ~135 MB, downloaded
//! once into the app data folder) turns every page chunk into a 384-dimensional vector.
//! Search is a brute-force cosine scan over the skill's vectors – a few
//! thousand to a few hundred thousand chunks fit easily – and its ranking is
//! fused with the full-text search in rag.rs.

use crate::db;
use crate::skill::chunk_page;
use rusqlite::{params, Connection};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::Emitter;

/// Shown in the UI; the files live in <data>/models.
pub const MODEL: &str = "multilingual-e5-small";
/// Characters per embedded chunk (about 300 tokens, under the model's 512).
pub const CHUNK: usize = 1200;
#[cfg(test)]
const DIM: usize = 384;
const BATCH: usize = 32;

static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// skills waiting for (re)indexing while a run is going
static PENDING: Mutex<Vec<i64>> = Mutex::new(Vec::new());

/// Files of one backend: Hugging Face folder URL, file names (the first is the big one), local folder.
pub struct Files {
    pub repo: &'static str,
    pub files: &'static [&'static str],
    pub dir: &'static str,
}

/// ONNX Runtime: the int8 quantized export (~118 MB) and the tokenizer.
#[allow(dead_code)]
pub const ONNX_FILES: Files = Files {
    repo: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/onnx/",
    files: &["model_qint8_avx512_vnni.onnx", "tokenizer.json", "config.json", "special_tokens_map.json", "tokenizer_config.json"],
    dir: "multilingual-e5-small",
};

/// candle (pure Rust, Intel macOS): the full precision weights (~470 MB).
#[allow(dead_code)]
pub const CANDLE_FILES: Files = Files {
    repo: "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main/",
    files: &["model.safetensors", "tokenizer.json", "config.json"],
    dir: "multilingual-e5-small-f32",
};

#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
const FILES: &Files = &ONNX_FILES;
#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
const FILES: &Files = &CANDLE_FILES;

/// Download size of the model in MB (for the UI).
pub const DOWNLOAD_MB: u32 = if cfg!(all(target_os = "macos", target_arch = "x86_64")) { 490 } else { 135 };

fn files_dir(data_dir: &Path, f: &Files) -> PathBuf {
    data_dir.join("models").join(f.dir)
}

pub fn model_dir(data_dir: &Path) -> PathBuf {
    files_dir(data_dir, FILES)
}

/// Every model file is on disk.
pub fn model_present(data_dir: &Path) -> bool {
    let d = model_dir(data_dir);
    FILES.files.iter().all(|f| d.join(f).is_file())
}

fn download(data_dir: &Path, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    download_files(data_dir, FILES, progress)
}

/// Downloads the missing files; `progress(done_bytes, total_bytes)` for the big one.
fn download_files(data_dir: &Path, set: &Files, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    use crate::settings::tr;
    let dir = files_dir(data_dir, set);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tauri::async_runtime::block_on(async {
        let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(3600)).build().map_err(|e| e.to_string())?;
        for (i, f) in set.files.iter().enumerate() {
            let target = dir.join(f);
            if target.is_file() {
                continue;
            }
            let mut resp = client
                .get(format!("{}{f}", set.repo))
                .send()
                .await
                .and_then(|r| r.error_for_status())
                .map_err(|e| format!("{}: {e}", tr("A vektoros modell letöltése nem sikerült", "The vector model could not be downloaded")))?;
            let total = resp.content_length().unwrap_or(0);
            let part = dir.join(format!("{f}.part"));
            let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
            let mut done = 0u64;
            let mut last = 0u64;
            while let Some(chunk) = resp.chunk().await.map_err(|e| format!("{}: {e}", tr("A vektoros modell letöltése megszakadt", "The vector model download broke off")))? {
                if STOP.load(Ordering::SeqCst) {
                    drop(out);
                    let _ = std::fs::remove_file(&part);
                    return Err(STOPPED.into());
                }
                std::io::Write::write_all(&mut out, &chunk).map_err(|e| e.to_string())?;
                done += chunk.len() as u64;
                if i == 0 && done - last > 1_000_000 {
                    last = done;
                    progress(done, total);
                }
            }
            drop(out);
            std::fs::rename(&part, &target).map_err(|e| e.to_string())?;
        }
        Ok(())
    })
}

const STOPPED: &str = "stopped";

fn not_downloaded() -> String {
    crate::settings::tr("A vektoros modell még nincs letöltve.", "The vector model has not been downloaded yet.")
}

fn load_err(e: impl std::fmt::Display) -> String {
    format!("{}: {e}", crate::settings::tr("A vektoros modell nem tölthető be", "The vector model cannot be loaded"))
}

/// ONNX Runtime backend (every platform with a prebuilt ONNX Runtime).
#[cfg(not(all(target_os = "macos", target_arch = "x86_64")))]
mod engine {
    use super::*;
    use fastembed::TextEmbedding;

    static ENGINE: Mutex<Option<TextEmbedding>> = Mutex::new(None);

    fn with_engine<T>(data_dir: &Path, f: impl FnOnce(&mut TextEmbedding) -> Result<T, String>) -> Result<T, String> {
        use fastembed::{InitOptionsUserDefined, Pooling, QuantizationMode, TokenizerFiles, UserDefinedEmbeddingModel};
        let mut g = ENGINE.lock().map_err(|e| e.to_string())?;
        if g.is_none() {
            if !model_present(data_dir) {
                return Err(not_downloaded());
            }
            let d = model_dir(data_dir);
            let read = |f: &str| std::fs::read(d.join(f)).map_err(|e| e.to_string());
            let tok = TokenizerFiles {
                tokenizer_file: read("tokenizer.json")?,
                config_file: read("config.json")?,
                special_tokens_map_file: read("special_tokens_map.json")?,
                tokenizer_config_file: read("tokenizer_config.json")?,
            };
            let model = UserDefinedEmbeddingModel::new(read(ONNX_FILES.files[0])?, tok).with_pooling(Pooling::Mean).with_quantization(QuantizationMode::Static);
            let m = TextEmbedding::try_new_from_user_defined(model, InitOptionsUserDefined::new().with_max_length(512)).map_err(load_err)?;
            *g = Some(m);
        }
        f(g.as_mut().unwrap())
    }

    pub fn embed_raw(data_dir: &Path, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        with_engine(data_dir, |m| m.embed(texts, Some(BATCH)).map_err(|e| e.to_string()))
    }

    pub fn load(data_dir: &Path) -> Result<(), String> {
        with_engine(data_dir, |_| Ok(()))
    }
}

/// candle backend (pure Rust): BERT encoder + mean pooling, like the ONNX export.
#[cfg(any(feature = "candle", all(target_os = "macos", target_arch = "x86_64")))]
pub mod candle_engine {
    use super::*;
    use candle_core::{DType, Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config};
    use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

    pub struct Engine {
        model: BertModel,
        tok: Tokenizer,
    }

    static ENGINE: Mutex<Option<Engine>> = Mutex::new(None);

    fn open(dir: &Path) -> Result<Engine, String> {
        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).map_err(load_err)?).map_err(load_err)?;
        let vb = unsafe { VarBuilder::from_mmaped_safetensors(&[dir.join("model.safetensors")], DType::F32, &Device::Cpu).map_err(load_err)? };
        let model = BertModel::load(vb, &cfg).map_err(load_err)?;
        let mut tok = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(load_err)?;
        tok.with_padding(Some(PaddingParams::default()));
        tok.with_truncation(Some(TruncationParams { max_length: 512, ..Default::default() })).map_err(load_err)?;
        Ok(Engine { model, tok })
    }

    fn run(e: &Engine, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let err = |x: candle_core::Error| x.to_string();
        let mut out = vec![];
        for part in texts.chunks(16) {
            let enc = e.tok.encode_batch(part.to_vec(), true).map_err(|x| x.to_string())?;
            let ids: Vec<Tensor> = enc.iter().map(|x| Tensor::new(x.get_ids(), &Device::Cpu)).collect::<Result<_, _>>().map_err(err)?;
            let mask: Vec<Tensor> = enc.iter().map(|x| Tensor::new(x.get_attention_mask(), &Device::Cpu)).collect::<Result<_, _>>().map_err(err)?;
            let ids = Tensor::stack(&ids, 0).map_err(err)?;
            let mask = Tensor::stack(&mask, 0).map_err(err)?;
            let types = ids.zeros_like().map_err(err)?;
            let h = e.model.forward(&ids, &types, Some(&mask)).map_err(err)?;
            // mean over the real tokens
            let m = mask.to_dtype(DType::F32).map_err(err)?.unsqueeze(2).map_err(err)?;
            let sum = h.broadcast_mul(&m).map_err(err)?.sum(1).map_err(err)?;
            let n = m.sum(1).map_err(err)?;
            let mean = sum.broadcast_div(&n).map_err(err)?;
            out.extend(mean.to_vec2::<f32>().map_err(err)?);
        }
        Ok(out)
    }

    /// Embeds with the model files in `dir` (tests use their own folder).
    pub fn embed_in(dir: &Path, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        let mut g = ENGINE.lock().map_err(|e| e.to_string())?;
        if g.is_none() {
            if !CANDLE_FILES.files.iter().all(|f| dir.join(f).is_file()) {
                return Err(not_downloaded());
            }
            *g = Some(open(dir)?);
        }
        run(g.as_ref().unwrap(), &texts)
    }

    pub fn files_dir_of(data_dir: &Path) -> PathBuf {
        files_dir(data_dir, &CANDLE_FILES)
    }
}

#[cfg(all(target_os = "macos", target_arch = "x86_64"))]
mod engine {
    use super::*;

    pub fn embed_raw(data_dir: &Path, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
        candle_engine::embed_in(&model_dir(data_dir), texts)
    }

    pub fn load(data_dir: &Path) -> Result<(), String> {
        candle_engine::embed_in(&model_dir(data_dir), vec!["query: x".into()]).map(|_| ())
    }
}

fn embed(data_dir: &Path, texts: Vec<String>) -> Result<Vec<Vec<f32>>, String> {
    engine::embed_raw(data_dir, texts).map(|vs| vs.into_iter().map(normalize).collect())
}

/// e5 models expect "query: " / "passage: " prefixes.
pub fn embed_queries(data_dir: &Path, qs: &[String]) -> Result<Vec<Vec<f32>>, String> {
    embed(data_dir, qs.iter().map(|q| format!("query: {q}")).collect())
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
    v
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn page_hash(text: &str) -> String {
    let d = Sha256::digest(text.as_bytes());
    d[..8].iter().map(|b| format!("{b:02x}")).collect()
}

/// The chunks of a page as they are embedded (also used to show the matched chunk).
pub fn chunks(text: &str) -> Vec<String> {
    chunk_page(text, CHUNK).into_iter().filter(|c| c.trim().chars().count() >= 20).collect()
}

/// Pages of the skill (0 = every skill) whose vectors are missing or outdated.
fn stale_pages(c: &Connection, skill_id: i64) -> Result<(Vec<(i64, i64, String)>, i64), String> {
    let mut st = c
        .prepare(
            "SELECT p.doc_id, p.no, p.text, (SELECT v.hash FROM vectors v WHERE v.doc_id = p.doc_id AND v.no = p.no LIMIT 1)
             FROM pages p JOIN documents d ON d.id = p.doc_id
             WHERE (?1 = 0 OR d.skill_id = ?1) AND d.status = 'done'
             ORDER BY p.doc_id, p.no",
        )
        .map_err(|e| e.to_string())?;
    let rows = st
        .query_map([skill_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))
        .map_err(|e| e.to_string())?;
    let mut out = vec![];
    let mut total = 0;
    for r in rows {
        let (d, no, text, hash) = r.map_err(|e| e.to_string())?;
        if text.trim().is_empty() {
            continue;
        }
        total += 1;
        if hash.as_deref() != Some(page_hash(&text).as_str()) {
            out.push((d, no, text));
        }
    }
    Ok((out, total))
}

#[derive(Serialize, Clone)]
pub struct Status {
    pub model: String,
    pub supported: bool,
    /// model download size in MB
    pub download_mb: u32,
    /// the model files are on disk
    pub ready: bool,
    pub running: bool,
    /// pages with text / pages with current vectors
    pub pages: i64,
    pub indexed: i64,
    pub chunks: i64,
}

pub fn status(c: &Connection, data_dir: &Path, skill_id: i64) -> Result<Status, String> {
    let (stale, total) = stale_pages(c, skill_id)?;
    let chunks: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM vectors v JOIN documents d ON d.id = v.doc_id WHERE (?1 = 0 OR d.skill_id = ?1)",
            [skill_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    Ok(Status { model: MODEL.into(), supported: true, download_mb: DOWNLOAD_MB, ready: model_present(data_dir), running: RUNNING.load(Ordering::SeqCst), pages: total, indexed: total - stale.len() as i64, chunks })
}

/// The `k` chunks most similar to the query vector: (doc_id, page, chunk, score).
pub fn search(c: &Connection, skill_id: i64, q: &[f32], k: usize) -> Result<Vec<(i64, i64, usize, f32)>, String> {
    let mut st = c
        .prepare(
            "SELECT v.doc_id, v.no, v.chunk, v.vec FROM vectors v JOIN documents d ON d.id = v.doc_id
             WHERE (?1 = 0 OR d.skill_id = ?1)",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = st.query([skill_id]).map_err(|e| e.to_string())?;
    let mut best: Vec<(i64, i64, usize, f32)> = vec![];
    while let Some(r) = rows.next().map_err(|e| e.to_string())? {
        let blob: Vec<u8> = r.get(3).map_err(|e| e.to_string())?;
        let v = from_blob(&blob);
        if v.len() != q.len() {
            continue;
        }
        let s: f32 = v.iter().zip(q).map(|(a, b)| a * b).sum();
        if best.len() < k || s > best[best.len() - 1].3 {
            best.push((r.get(0).map_err(|e| e.to_string())?, r.get(1).map_err(|e| e.to_string())?, r.get::<_, i64>(2).map_err(|e| e.to_string())? as usize, s));
            best.sort_by(|a, b| b.3.partial_cmp(&a.3).unwrap_or(std::cmp::Ordering::Equal));
            best.truncate(k);
        }
    }
    Ok(best)
}

fn store(c: &mut Connection, doc_id: i64, no: i64, hash: &str, vecs: &[Vec<f32>]) -> Result<(), String> {
    let tx = c.transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM vectors WHERE doc_id=?1 AND no=?2", params![doc_id, no]).map_err(|e| e.to_string())?;
    for (i, v) in vecs.iter().enumerate() {
        tx.execute("INSERT INTO vectors (doc_id, no, chunk, hash, vec) VALUES (?1, ?2, ?3, ?4, ?5)", params![doc_id, no, i as i64, hash, to_blob(v)])
            .map_err(|e| e.to_string())?;
    }
    tx.commit().map_err(|e| e.to_string())
}

#[derive(Serialize, Clone)]
struct Progress {
    skill_id: i64,
    /// "download" | "index" | "done" | "error" | "stopped"
    stage: String,
    done: i64,
    total: i64,
    error: String,
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

#[allow(dead_code)]
pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Builds the missing vectors of a skill in the background (`download`: the model may be fetched first).
pub fn start(app: tauri::AppHandle, db_path: PathBuf, data_dir: PathBuf, skill_id: i64) {
    {
        let mut p = PENDING.lock().unwrap();
        if !p.contains(&skill_id) {
            p.push(skill_id);
        }
    }
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    STOP.store(false, Ordering::SeqCst);
    std::thread::spawn(move || {
        loop {
            let next = {
                let mut p = PENDING.lock().unwrap();
                if p.is_empty() {
                    None
                } else {
                    Some(p.remove(0))
                }
            };
            let Some(sk) = next else { break };
            let emit = |stage: &str, done: i64, total: i64, error: &str| {
                let _ = app.emit("vec-progress", Progress { skill_id: sk, stage: stage.into(), done, total, error: error.into() });
            };
            match run(&db_path, &data_dir, sk, &emit) {
                Ok(true) => emit("done", 0, 0, ""),
                Ok(false) => {
                    emit("stopped", 0, 0, "");
                    PENDING.lock().unwrap().clear();
                    break;
                }
                Err(e) => emit("error", 0, 0, &e),
            }
        }
        RUNNING.store(false, Ordering::SeqCst);
        // a skill queued just as the run ended
        if !PENDING.lock().unwrap().is_empty() && !STOP.load(Ordering::SeqCst) {
            if let Some(sk) = PENDING.lock().unwrap().pop() {
                start(app, db_path, data_dir, sk);
            }
        }
    });
}

/// Ok(false) = stopped by the user.
fn run(db_path: &Path, data_dir: &Path, skill_id: i64, emit: &dyn Fn(&str, i64, i64, &str)) -> Result<bool, String> {
    let mut c = db::open(db_path)?;
    let (stale, total) = stale_pages(&c, skill_id)?;
    if stale.is_empty() {
        return Ok(true);
    }
    if !model_present(data_dir) {
        emit("download", 0, 0, "");
        match download(data_dir, &|done, total| emit("download", done as i64, total as i64, "")) {
            Err(e) if e == STOPPED => return Ok(false),
            r => r?,
        }
    }
    engine::load(data_dir)?;
    let mut done = total - stale.len() as i64;
    emit("index", done, total, "");
    // pages are collected into batches of about BATCH chunks
    let mut batch: Vec<(i64, i64, String, usize)> = vec![];
    let mut texts: Vec<String> = vec![];
    let flush = |c: &mut Connection, batch: &mut Vec<(i64, i64, String, usize)>, texts: &mut Vec<String>| -> Result<(), String> {
        let vecs = if texts.is_empty() { vec![] } else { embed(data_dir, std::mem::take(texts))? };
        let mut at = 0;
        for (d, no, hash, n) in batch.drain(..) {
            store(c, d, no, &hash, &vecs[at..at + n])?;
            at += n;
        }
        Ok(())
    };
    for (d, no, text) in stale {
        if STOP.load(Ordering::SeqCst) {
            return Ok(false);
        }
        let cs = chunks(&text);
        let n = cs.len();
        texts.extend(cs.into_iter().map(|t| format!("passage: {t}")));
        batch.push((d, no, page_hash(&text), n));
        if texts.len() >= BATCH {
            let pages = batch.len() as i64;
            flush(&mut c, &mut batch, &mut texts)?;
            done += pages;
            emit("index", done, total, "");
        }
    }
    flush(&mut c, &mut batch, &mut texts)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blobs_and_search() {
        let v = normalize(vec![3.0, 4.0]);
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert_eq!(from_blob(&to_blob(&v)), v);
        let mut c = db::open_memory();
        let s = db::create_skill(&c, "T", "", "hu").unwrap();
        let dir = std::env::temp_dir().join(format!("skillstar-vec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, "x").unwrap();
        let d = db::add_document(&c, s, &f).unwrap().unwrap();
        store(&mut c, d, 1, "h", &[normalize(vec![1.0, 0.0]), normalize(vec![0.7, 0.7])]).unwrap();
        store(&mut c, d, 2, "h", &[normalize(vec![0.0, 1.0])]).unwrap();
        let r = search(&c, s, &normalize(vec![0.1, 1.0]), 2).unwrap();
        assert_eq!((r[0].1, r[0].2), (2, 0));
        assert_eq!((r[1].1, r[1].2), (1, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// candle vs ONNX on the same texts: SKILLSTAR_VEC_TEST=<dir> cargo test --features candle candle_matches -- --ignored
    #[cfg(feature = "candle")]
    #[test]
    #[ignore]
    fn candle_matches() {
        let dir = PathBuf::from(std::env::var("SKILLSTAR_VEC_TEST").unwrap());
        download(&dir, &|_, _| {}).unwrap();
        download_files(&dir, &CANDLE_FILES, &|_, _| {}).unwrap();
        let texts: Vec<String> = vec!["query: Hol van a megszakítás vektor?".into(), "passage: Az IRQ vektor a $0314 címen található, a KERNAL ide ugrik.".into(), "passage: A SID három hangcsatornát tartalmaz.".into()];
        let a: Vec<Vec<f32>> = engine::embed_raw(&dir, texts.clone()).unwrap().into_iter().map(normalize).collect();
        let t = std::time::Instant::now();
        let b: Vec<Vec<f32>> = candle_engine::embed_in(&candle_engine::files_dir_of(&dir), texts).unwrap().into_iter().map(normalize).collect();
        eprintln!("candle: {:?}", t.elapsed());
        let dot = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| p * q).sum::<f32>();
        for i in 0..3 {
            eprintln!("onnx vs candle {i}: {}", dot(&a[i], &b[i]));
            assert!(dot(&a[i], &b[i]) > 0.95);
        }
        assert!(dot(&b[0], &b[1]) > dot(&b[0], &b[2]));
    }

    /// Real model: SKILLSTAR_VEC_TEST=<data dir> cargo test real_model -- --ignored
    #[test]
    #[ignore]
    fn real_model() {
        let dir = PathBuf::from(std::env::var("SKILLSTAR_VEC_TEST").unwrap());
        download(&dir, &|_, _| {}).unwrap();
        let q = embed_queries(&dir, &["Hol van a megszakítás vektor?".into()]).unwrap();
        let p = embed(&dir, vec!["passage: Az IRQ vektor a $0314 címen található.".into(), "passage: A SID három hangcsatornát tartalmaz.".into()]).unwrap();
        assert_eq!(q[0].len(), DIM);
        let s0: f32 = q[0].iter().zip(&p[0]).map(|(a, b)| a * b).sum();
        let s1: f32 = q[0].iter().zip(&p[1]).map(|(a, b)| a * b).sum();
        assert!(s0 > s1, "{s0} {s1}");
        assert!(model_present(&dir));
        let long: Vec<String> = (0..64).map(|i| format!("passage: {} {i}", "A raszter megszakítás a VIC-II $D012 regiszterével állítható be, a sprite-ok mozgatása. ".repeat(14))).collect();
        let t = std::time::Instant::now();
        embed(&dir, long).unwrap();
        eprintln!("64 chunks of ~1200 chars: {:?}", t.elapsed());
    }
}
