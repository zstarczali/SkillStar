<p align="center">
  <img src="assets/icon.svg" alt="SkillStar logo" width="200" height="200">
</p>

<h1 align="center">SkillStar</h1>

<!-- badges:start -->
<p align="center">
  <a href="https://www.c64va.pro/skillstar.html"><img src="https://img.shields.io/badge/website-c64va.pro-e05d44?style=flat" alt="Website"></a>
  <a href="https://github.com/zstarczali/SkillStar/blob/main/package.json"><img src="https://img.shields.io/github/package-json/v/zstarczali/SkillStar/main?label=version&color=4c1" alt="Version"></a>
  <a href="https://github.com/zstarczali/SkillStar/actions/workflows/build.yml"><img src="https://img.shields.io/github/actions/workflow/status/zstarczali/SkillStar/build.yml?branch=main&label=build" alt="Build"></a>
  <a href="https://github.com/zstarczali/SkillStar/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-freeware-blue" alt="License: Freeware"></a>
  <a href="https://www.c64va.pro/register/donate.php"><img src="https://img.shields.io/badge/donate-PayPal-00457C?logo=paypal&logoColor=white" alt="Donate"></a>
</p>
<p align="center">
  <a href="https://v2.tauri.app/"><img src="https://img.shields.io/badge/Tauri-2-24C8D8?logo=tauri&logoColor=white" alt="Tauri 2"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-backend-B7410E?logo=rust&logoColor=white" alt="Rust"></a>
  <a href="https://www.c64va.pro/skillstar.html"><img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-555" alt="Platforms"></a>
  <a href="https://github.com/zstarczali/SkillStar/commits/main"><img src="https://img.shields.io/github/last-commit/zstarczali/SkillStar/main" alt="Last commit"></a>
  <a href="https://github.com/zstarczali/SkillStar/graphs/commit-activity"><img src="https://img.shields.io/github/commit-activity/m/zstarczali/SkillStar" alt="Commit activity"></a>
  <a href="https://github.com/zstarczali/SkillStar/issues"><img src="https://img.shields.io/github/issues/zstarczali/SkillStar" alt="Issues"></a>
  <a href="https://github.com/zstarczali/SkillStar/stargazers"><img src="https://img.shields.io/github/stars/zstarczali/SkillStar?style=flat" alt="Stars"></a>
</p>
<p align="center">
  <a href="https://github.com/zstarczali/SkillStar#readme"><img src="https://img.shields.io/badge/English-ddd" alt="English"></a>
  <a href="https://github.com/zstarczali/SkillStar#readme"><img src="https://img.shields.io/badge/Magyar-ddd" alt="Magyar"></a>
  <a href="https://github.com/zstarczali/GitStar"><img src="https://img.shields.io/badge/sibling-GitStar-8a2be2" alt="GitStar"></a>
  <a href="https://github.com/zstarczali/FtpStar"><img src="https://img.shields.io/badge/sibling-FtpStar-8a2be2" alt="FtpStar"></a>
  <a href="https://github.com/zstarczali/DiffStar"><img src="https://img.shields.io/badge/sibling-DiffStar-8a2be2" alt="DiffStar"></a>
  <a href="https://github.com/zstarczali/NoteStar"><img src="https://img.shields.io/badge/sibling-NoteStar-8a2be2" alt="NoteStar"></a>
</p>
<!-- badges:end -->

SkillStar turns a pile of **DOCX, PDF, PPTX, ODT, HTML, Markdown and text files** — scanned PDFs too, through **OCR** — into a **Claude skill package**: the full text of every document, page by page, with an index, tables of contents and a keyword index, plus a `SKILL.md` that tells Claude how to search it and cite the sources. Tauri 2 and Rust backend, local SQLite database, no server. It is the sibling of [GitStar](https://github.com/zstarczali/GitStar), [FtpStar](https://github.com/zstarczali/FtpStar), [DiffStar](https://github.com/zstarczali/DiffStar) and [NoteStar](https://github.com/zstarczali/NoteStar) and shares their themes, dialogs and AI setup.

Current version **1.1.1**. See [What's new](whatsnew.txt) for the release history.

## Features

- **Skills** — Any number of skills, each with its own documents, name, description, keywords, version, author, language and extra instructions for the model.
- **Documents** — Add files or whole folders, or drop them onto the window. DOCX (headings, lists, tables, page breaks), PDF, PPTX (one page per slide), ODT, HTML, Markdown and text (UTF-8 / UTF-16 / Windows-1250). Changed or missing source files are detected.
- **PDF** through [PDFium](https://pdfium.googlesource.com/pdfium/): text layer and bookmarks (as the table of contents). PDFium is bundled with the installers; when missing, SkillStar downloads it.
- **OCR** for pages without a usable text layer — the built-in **Windows OCR** (no install) or **Tesseract**. Per document: automatic, always or never. Every finished page is saved at once, so **a stopped or interrupted run continues where it left off**.
- **Local database** — SQLite with **FTS5 full-text search** over every page (accent-insensitive), snippets, a hit opens the page.
- **Text viewer** — Page by page with the outline; the page text can be edited to fix OCR errors.
- **Package formats** — chosen per skill, each into its own folder of the output (`<out>/<name>/`), with a preview of every file:
  - **Claude** — Claude Code plugin folder (`.claude-plugin/plugin.json`, `skills/<name>/SKILL.md`, `knowledge/index.md`, `knowledge/DOCUMENTS.md`, `knowledge/sources/*.md` with `--- PAGE n ---` markers), the same as one **`.plugin`** file, and **`<name>-skill.zip`** for **claude.ai → Settings → Capabilities → Skills**.
  - **Agent Skills** — the open `SKILL.md` format for other coding agents (OpenAI Codex, Gemini CLI, GitHub Copilot, Cursor…), folder + ZIP.
  - **ChatGPT** — Custom GPT / Project: `instructions.md` (≤ 8000 characters) and at most 20 knowledge files (documents merged when needed) with a catalog.
  - **Gemini** — Gem: instructions and at most 10 knowledge files.
  - **DeepSeek** — system prompt and the text cut into context-sized parts, with a catalog of which pages are in which part.
  - **RAG** — `chunks.jsonl` (page-aligned chunks with document, page and OCR flag), `documents.json` and a system prompt, for any LLM and vector store.
- **Ask the documents (RAG)** (with AI on) — the question, plus search queries the AI writes (synonyms, inflected and other-language forms), go through the local full-text index; the best passages are fused, numbered and sent to the model, and the answer cites them as **[n]**: click a citation or a source to open the page. Search one skill or every skill.
- **Optional AI** (off by default, **Settings → AI**) — titles and "when to use" descriptions of the documents, the skill description, and the RAG questions above. Only the needed text is sent, and only when you ask.
- **Themes** — Dark, light, neon, retro, modern, modern dark, or system, as in GitStar. Zoom with Ctrl + / Ctrl − / Ctrl 0.
- **Self-update** — Looks for a newer installer in `downloads/SkillStar/` on the update server (default `https://www.c64va.pro/`).
- **English and Hungarian** interface.

## Running from source

Install stable Rust and Node.js. On Windows, WebView2 is required and comes with Windows 11. On Linux, install the Tauri system dependencies, including WebKitGTK 4.1.

```bash
npm install
npm run dev
```

PDF support needs the PDFium library: put `pdfium.dll` / `libpdfium.dylib` / `libpdfium.so` from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases) into `src-tauri/resources/pdfium/`, or let the app download it (**Settings → Extraction**).

To build an NSIS installer locally on Windows (written to `src-tauri/target/release/bundle`):

```bash
npm run build -- --bundles nsis
```

Tests (database, DOCX, text extraction, package building, Windows OCR): `cd src-tauri && cargo test`. A real-PDF test runs when `SKILLSTAR_PDFIUM` (path of the PDFium library) and `SKILLSTAR_TEST_PDF` are set: `cargo test pdfium_text_layer -- --nocapture`.

### Building installers with GitHub Actions

The `.github/workflows/build.yml` workflow runs **only when manually triggered** (**Actions → Build installers → Run workflow**). It downloads PDFium for each target and builds Windows x64 / ARM64 (NSIS), macOS ARM64 / x64 (`.dmg`) and Linux x64 (`.deb`, `.rpm`, `.AppImage`) installers. **Actions → Publish** uploads them, plus `readme.txt`, `whatsnew.txt` and `LICENSE`, to `/www/downloads/SkillStar/` over FTPS; it needs the **FTP_PASSWORD** Actions secret.

## OCR languages

Windows OCR uses the OCR languages installed in Windows: **Settings → Time & language → Language & region → add a language** (with *optical character recognition*). Choose the language in **Settings → Extraction**; *automatic* uses the user profile languages. For Tesseract, install it (with the `hun`, `eng`… traineddata), then set its path and languages (e.g. `hun+eng`) there.

## Using the packages

Every format folder has its own `README.md` with the steps. In short:

- **Claude Code** — `claude --plugin-dir <out>/<name>/claude/<name>`, or install `claude/<name>.plugin`.
- **claude.ai / Claude Desktop** — upload `claude/<name>-skill.zip` at **Settings → Capabilities → Skills**.
- **Agent Skills** — copy `agents/<name>/` into the skills folder of the tool.
- **ChatGPT / Gemini** — paste `instructions.md` into the GPT / Gem and upload the files of `knowledge/`.
- **DeepSeek** — paste `system-prompt.md`, attach `00-catalog.md` and the parts you need.
- **RAG** — index `rag/chunks.jsonl` and use `rag/system-prompt.md`.

## AI setup

Turn on **Settings → AI → Turn on the AI features**, then choose **Anthropic Claude (API key)**, **Claude Code CLI (your own login)** or **DeepSeek** — the same options as in GitStar. When off, SkillStar contacts no AI provider and shows no AI buttons.

## Keyboard shortcuts

| Shortcut | Action |
|---|---|
| Ctrl+N | New skill |
| Ctrl+O | Add files |
| F5 | Process the documents |
| Ctrl+B | Build the package |
| Ctrl+F | Search in the documents |
| Ctrl+A / Del | Select all / remove the selected documents |
| Ctrl+Shift+A | AI assistant (when on) |
| Ctrl+, / F1 | Settings / About |

## Project structure

```text
src-tauri/src/
  db.rs         SQLite: skills, documents, pages, outline, FTS5 search, resumable pages
  extract.rs    Text extraction dispatch, text/HTML/Markdown, page chunks
  docx.rs       DOCX / PPTX / ODT → Markdown pages
  pdf.rs        PDFium: text layer, bookmarks, page rendering for OCR, download
  ocr.rs        Windows.Media.Ocr and Tesseract
  process.rs    Background processing queue with progress events
  skill.rs      Package formats: Claude, Agent Skills, ChatGPT, Gemini, DeepSeek, RAG
  rag.rs        RAG retrieval: query terms, FTS5, rank fusion, best passage per page
  ai.rs         AI API calls and prompts (claude_cli.rs: Claude Code CLI)
  commands.rs   Tauri commands
  update.rs     Self-update
www/js/
  app.js        Toolbar, welcome page, settings, AI panel, about, update, drag and drop
  skills.js     Skill list, document table, text viewer, skill form, package preview, search
  markdown.js   Markdown → safe HTML (marked + DOMPurify)
  i18n.js, i18n-en.js  Hungarian → English translations
www/vendor/     marked 18.2.0 (MIT) and DOMPurify 3.4.18 (Apache-2.0 / MPL-2.0)
```

## License

SkillStar is freeware: free to download and use, including at work, but it may not be copied, redistributed, published, sold, or modified without the author's written permission. See [LICENSE](LICENSE). The bundled third-party libraries in `www/vendor/` keep their own licenses (`LICENSE-marked.md`, `LICENSE-dompurify.txt`); PDFium is BSD-3-Clause / Apache-2.0.

If SkillStar is useful to you, you can support its development with a small [donation](https://www.c64va.pro/register/donate.php).

© 2026 Zsolt Tarczali
