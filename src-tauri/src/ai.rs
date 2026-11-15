//! AI features through the Claude Messages API (raw HTTP, API key auth),
//! DeepSeek, or the local Claude Code CLI.

use crate::settings::tr;
use serde_json::{json, Value};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2026-10-21";
const FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

const DEEPSEEK_URL: &str = "https://api.deepseek.com/chat/completions";

pub struct AiRequest<'a> {
    /// "anthropic" | "deepseek" | "claude-cli"
    pub provider: &'a str,
    /// API key; for "claude-cli" the CLI path from Settings (empty = find it)
    pub api_key: &'a str,
    pub model: &'a str,
    pub system: String,
    pub user: String,
    pub effort: &'a str,
    pub max_tokens: u32,
}

/// Models that accept the `output_config.effort` parameter.
fn supports_effort(model: &str) -> bool {
    !(model.starts_with("claude-haiku-4") || model.contains("sonnet-4-5") || model.contains("3-"))
}

/// Models that accept server-side `fallbacks: "default"`.
fn supports_fallback(model: &str) -> bool {
    matches!(model, "claude-opus-5-5" | "claude-opus-5" | "claude-fable-5-1" | "claude-sonnet-5-5")
}

pub async fn complete(req: AiRequest<'_>) -> Result<String, String> {
    if req.provider == "deepseek" {
        return complete_deepseek(req).await;
    }
    if req.provider == "claude-cli" {
        let (path, model, system, user) = (req.api_key.to_string(), req.model.to_string(), req.system, req.user);
        let effort = supports_effort(&model).then(|| req.effort.to_string());
        return tauri::async_runtime::spawn_blocking(move || crate::claude_cli::complete(&path, &model, effort.as_deref(), &system, &user))
            .await
            .map_err(|e| e.to_string())?;
    }
    let body = anthropic_body(&req, json!([{ "role": "user", "content": req.user }]));
    let v = anthropic_post(req.api_key, req.model, &body).await?;
    let text: String = v["content"]
        .as_array()
        .map(|blocks| {
            blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("")
        })
        .unwrap_or_default();
    finish_text(text, &v)
}

fn anthropic_body(req: &AiRequest<'_>, messages: Value) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "system": req.system,
        "messages": messages,
    });
    if supports_effort(req.model) {
        body["output_config"] = json!({ "effort": req.effort });
    }
    if supports_fallback(req.model) {
        body["fallbacks"] = json!("default");
    }
    body
}

fn finish_text(text: String, v: &Value) -> Result<String, String> {
    if text.trim().is_empty() {
        return Err(tr("Üres válasz érkezett a modelltől.", "The model returned an empty response."));
    }
    let mut text = text.trim().to_string();
    if v["stop_reason"].as_str() == Some("max_tokens") {
        text.push_str(&tr("\n\n_(A válasz a maximális hossznál megszakadt.)_", "\n\n_(The answer was cut at the maximum length.)_"));
    }
    Ok(text)
}

/// One Messages API call; HTTP errors and refusals become readable errors.
async fn anthropic_post(api_key: &str, model: &str, body: &Value) -> Result<Value, String> {
    if api_key.trim().is_empty() {
        return Err(tr("Nincs megadva Anthropic API kulcs (Beállítások → AI).", "No Anthropic API key is set (Settings → AI)."));
    }
    let fallback = supports_fallback(model);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let mut http = client
        .post(API_URL)
        .header("x-api-key", api_key.trim())
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json");
    if fallback {
        http = http.header("anthropic-beta", FALLBACK_BETA);
    }
    let resp = http.json(body).send().await.map_err(|e| format!("{}: {e}", tr("Hálózati hiba", "Network error")))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|e| format!("{}: {e}", tr("Hibás válasz", "Invalid response")))?;
    if !status.is_success() {
        let unknown = tr("ismeretlen hiba", "unknown error");
        let msg = v["error"]["message"].as_str().unwrap_or(&unknown);
        let kind = v["error"]["type"].as_str().unwrap_or("");
        return Err(match status.as_u16() {
            401 => format!("{} ({msg})", tr("Érvénytelen API kulcs", "Invalid API key")),
            429 => format!("{} ({msg})", tr("Túl sok kérés / kvóta elfogyott, próbáld később", "Too many requests / quota exceeded, try again later")),
            529 => format!("{} ({msg})", tr("Az API túlterhelt, próbáld később", "The API is overloaded, try again later")),
            c => format!("{} {c} {kind}: {msg}", tr("API hiba", "API error")),
        });
    }
    if v["stop_reason"].as_str() == Some("refusal") {
        let expl = v["stop_details"]["explanation"].as_str().unwrap_or("");
        return Err(format!("{} {expl}", tr("A modell elutasította a kérést.", "The model declined the request.")));
    }
    Ok(v)
}

/// A web page the answer used (cited as [Wn]).
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Default)]
pub struct WebSource {
    pub n: usize,
    pub url: String,
    pub title: String,
    /// the cited text (empty if the page was found but not cited)
    pub text: String,
    pub cited: bool,
}

/// Web search tool version the model accepts (dynamic filtering on newer models).
fn web_tool(model: &str) -> &'static str {
    let new = ["claude-fable", "claude-mythos", "claude-opus-5", "claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6", "claude-sonnet-5", "claude-sonnet-4-6"];
    if new.iter().any(|p| model.starts_with(p)) {
        "web_search_20260209"
    } else {
        "web_search_20250305"
    }
}

/// Anthropic call with the server-side web search tool: the answer text with
/// [Wn] marks after every web-cited part, and the pages it found.
pub async fn complete_web(req: AiRequest<'_>, max_uses: u32) -> Result<(String, Vec<WebSource>), String> {
    if req.provider != "anthropic" && !req.provider.is_empty() {
        return Err(tr("A webes keresés csak az Anthropic API-val működik (Beállítások → AI).", "Web search works only with the Anthropic API (Settings → AI)."));
    }
    let mut messages = vec![json!({ "role": "user", "content": req.user })];
    let mut blocks: Vec<Value> = vec![];
    let mut last = Value::Null;
    // the server pauses long searching turns; the content is sent back to continue
    for _ in 0..4 {
        let mut body = anthropic_body(&req, Value::Array(messages.clone()));
        body["tools"] = json!([{ "type": web_tool(req.model), "name": "web_search", "max_uses": max_uses }]);
        let v = anthropic_post(req.api_key, req.model, &body).await?;
        let content = v["content"].as_array().cloned().unwrap_or_default();
        blocks.extend(content.iter().cloned());
        let paused = v["stop_reason"].as_str() == Some("pause_turn");
        last = v;
        if !paused {
            break;
        }
        messages.push(json!({ "role": "assistant", "content": content }));
    }
    let mut web: Vec<WebSource> = vec![];
    let mut text = String::new();
    for b in &blocks {
        match b["type"].as_str() {
            Some("web_search_tool_result") => {
                // a list on success, an error object otherwise
                for r in b["content"].as_array().into_iter().flatten() {
                    let url = r["url"].as_str().unwrap_or("").to_string();
                    if !url.is_empty() && !web.iter().any(|w| w.url == url) {
                        web.push(WebSource { url, title: r["title"].as_str().unwrap_or("").to_string(), ..Default::default() });
                    }
                }
            }
            Some("text") => {
                text.push_str(b["text"].as_str().unwrap_or(""));
                let mut marks: Vec<usize> = vec![];
                for c in b["citations"].as_array().into_iter().flatten() {
                    if c["type"] != "web_search_result_location" {
                        continue;
                    }
                    let url = c["url"].as_str().unwrap_or("").to_string();
                    let i = match web.iter().position(|w| w.url == url) {
                        Some(i) => i,
                        None => {
                            web.push(WebSource { url: url.clone(), title: c["title"].as_str().unwrap_or("").to_string(), ..Default::default() });
                            web.len() - 1
                        }
                    };
                    let w = &mut web[i];
                    w.cited = true;
                    let quote = c["cited_text"].as_str().unwrap_or("");
                    if w.text.chars().count() < 600 && !quote.is_empty() && !w.text.contains(quote) {
                        if !w.text.is_empty() {
                            w.text.push_str(" … ");
                        }
                        w.text.push_str(quote);
                    }
                    if !marks.contains(&i) {
                        marks.push(i);
                    }
                }
                // cited pages first get numbers in the order the answer cites them
                for i in marks {
                    if web[i].n == 0 {
                        web[i].n = web.iter().filter(|w| w.n > 0).count() + 1;
                    }
                    let n = web[i].n;
                    let mark = format!("[W{n}]");
                    if !text.ends_with(&mark) {
                        // before the trailing punctuation of the cited part
                        let trimmed = text.trim_end();
                        text.truncate(trimmed.len());
                        text.push_str(&mark);
                    }
                }
            }
            _ => {}
        }
    }
    let mut next = web.iter().filter(|w| w.n > 0).count();
    for w in web.iter_mut().filter(|w| w.n == 0) {
        next += 1;
        w.n = next;
    }
    web.sort_by_key(|w| w.n);
    let text = finish_text(text, &last)?;
    Ok((text, web))
}

#[cfg(test)]
mod tests {
    #[test]
    fn web_tool_versions() {
        assert_eq!(super::web_tool("claude-opus-5-5"), "web_search_20260209");
        assert_eq!(super::web_tool("claude-haiku-4-5"), "web_search_20250305");
    }
}

/// DeepSeek: OpenAI-compatible chat completions API, Bearer key.
async fn complete_deepseek(req: AiRequest<'_>) -> Result<String, String> {
    if req.api_key.trim().is_empty() {
        return Err(tr("Nincs megadva DeepSeek API kulcs (Beállítások → AI).", "No DeepSeek API key is set (Settings → AI)."));
    }
    // max_tokens is left to the server default: the per-model limits are not published.
    let body = json!({
        "model": req.model,
        "messages": [
            { "role": "system", "content": req.system },
            { "role": "user", "content": req.user },
        ],
        "stream": false,
    });
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(DEEPSEEK_URL)
        .bearer_auth(req.api_key.trim())
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("{}: {e}", tr("Hálózati hiba", "Network error")))?;
    let status = resp.status();
    let raw = resp.text().await.map_err(|e| format!("{}: {e}", tr("Hibás válasz", "Invalid response")))?;
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if !status.is_success() {
        let msg = v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| raw.chars().take(300).collect());
        return Err(match status.as_u16() {
            401 => format!("{} ({msg})", tr("Érvénytelen DeepSeek API kulcs", "Invalid DeepSeek API key")),
            402 => format!("{} ({msg})", tr("Elfogyott a DeepSeek egyenleg", "DeepSeek balance is insufficient")),
            429 => format!("{} ({msg})", tr("Túl sok kérés, próbáld később", "Too many requests, try again later")),
            500 | 503 => format!("{} ({msg})", tr("A DeepSeek szerver túlterhelt vagy hibás, próbáld később", "The DeepSeek server is busy or failing, try again later")),
            c => format!("DeepSeek {} {c}: {msg}", tr("hiba", "error")),
        });
    }
    let choice = &v["choices"][0];
    let text = choice["message"]["content"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Err(tr("Üres válasz érkezett a modelltől.", "The model returned an empty response."));
    }
    let mut text = text;
    if choice["finish_reason"].as_str() == Some("length") {
        text.push_str(&tr("\n\n_(A válasz a maximális hossznál megszakadt.)_", "\n\n_(The answer was cut at the maximum length.)_"));
    }
    Ok(text)
}

fn lang_name(lang: &str) -> &'static str {
    if lang == "en" { "English" } else { "Hungarian" }
}

/// Describes one document for the skill: summary + "use when".
pub fn describe_system(lang: &str) -> String {
    format!(
        "You help build a knowledge skill for an AI assistant from documents. You get the beginning (and, for long          documents, samples) of ONE document inside <document>, with `--- PAGE n ---` markers, plus its outline if any.
         Answer in {lang} with EXACTLY this structure and nothing else:
         TITLE: <the real title of the document, short>
         SUMMARY: <3–5 sentences: what the document is, who it is for, what it covers>
         USE_WHEN: <one sentence starting with a verb-less condition, listing the concrete questions/topics this document answers, with key terms>
         Never invent content that is not in the text. Text read with OCR may contain character errors: ignore them.",
        lang = lang_name(lang)
    )
}

/// Writes the description (trigger text) of the whole skill.
pub fn skill_desc_system(lang: &str) -> String {
    format!(
        "You write the `description` field of an Agent Skill (Claude). The description decides when the skill is          activated, so it must say what knowledge the skill holds and list the concrete topics, terms and typical user          phrases (in {lang} and, where natural, English) that should trigger it. You get the skill title, the documents          with their summaries and the most characteristic keywords.
         Answer with the description text only: one paragraph, at most 900 characters, no quotes, no Markdown, written          in {lang}. Start with what the skill is, then \"Use when…\" / \"Használd, ha…\" with the triggers.",
        lang = lang_name(lang)
    )
}

/// RAG answer: numbered passages in, Markdown answer with [n] citations out.
pub fn ask_system(lang: &str, web: bool) -> String {
    let extra = if web {
        " You can also search the web: use it when the passages do not answer the question fully or the question needs \
         current information. Web results are cited automatically; never give them passage numbers."
    } else {
        ""
    };
    format!(
        "You answer questions from a document collection (retrieval-augmented generation). The user message holds \
         numbered passages retrieved from the documents inside <passages> (<passage n=\"…\" document=\"…\" page=\"…\">), \
         possibly the earlier conversation, and the question. Answer in {lang}, as Markdown. Base the answer on the \
         passages first, and put the passage number in square brackets after every claim a passage supports: [1], \
         [2][4]. Do not cite documents in any other way and never invent passage numbers or put them on claims the \
         passages do not support. If the passages do not cover something, say so; you may add what you know \
         yourself, as separate sentences without a passage number.{extra} Quote verbatim when the exact wording \
         matters. Text read with OCR may contain character errors.",
        lang = lang_name(lang)
    )
}

/// Query expansion for the RAG search: the question → a few full-text search queries.
pub fn rag_queries_system() -> String {
    "You write search queries for a full-text (keyword, prefix-matching) search over a document collection, to find \
     the passages that answer the user's question. Write 3 to 5 queries, one per line, each 2–6 words, no operators, \
     no quotes, no numbering, nothing else. Use the important terms, their base forms and synonyms, technical terms, \
     register names or numbers from the question, and translations (English ↔ Hungarian, German) where documents \
     might use another language. If the question refers to the earlier conversation, resolve the reference. \
     Read the question in the domain of the collection: questions are often typed without accents and use the \
     jargon of the field, so take a word in its meaning in that field (in a computer collection \"hires\" is the \
     high-resolution graphics mode, not \"híres\"), and keep its original spelling in one of the queries."
        .to_string()
}
