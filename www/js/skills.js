// SkillStar – the views of one skill: Documents, Text, Skill (metadata),
// Package (preview + build) and Search; plus the sidebar with the skills.

import { invoke, h, icon, toast, modal, confirmDlg, contextMenu, relTime, fmtDate, debounce } from './util.js';
import { t } from './i18n.js';
import { mdRender } from './markdown.js';

/** Package formats: id, name, what it is for (a function: translated at render time). */
export const FORMATS = () => [
  ['claude', 'Claude', t('Claude Code plugin, .plugin, claude.ai / Desktop skill ZIP')],
  ['agents', 'Agent Skills', t('OpenAI Codex, Gemini CLI, GitHub Copilot, Cursor…')],
  ['chatgpt', 'ChatGPT', t('Custom GPT / Project: utasítás + ≤ 20 tudásfájl')],
  ['gemini', 'Gemini', t('Gem: utasítás + ≤ 10 tudásfájl')],
  ['deepseek', 'DeepSeek', t('rendszerprompt + kontextusnyi részek')],
  ['rag', 'RAG', t('chunks.jsonl bármely LLM-hez és vektoradatbázishoz')],
];
const fmtSet = (sk) => new Set((sk?.formats || 'claude').split(',').map((x) => x.trim()).filter(Boolean));

/** Chips to switch the package formats on and off; `onChange(set)` after every click (at least one stays on). */
function formatChips(sk, onChange, compact = false) {
  const set = fmtSet(sk);
  const wrap = h('div.fmt-chips' + (compact ? '.compact' : ''));
  const draw = () => wrap.replaceChildren(...FORMATS().map(([id, name, what]) => h('button.fmt-chip' + (set.has(id) ? '.on' : ''), {
    type: 'button', title: what,
    on: { click: () => {
      if (set.has(id)) { if (set.size === 1) return; set.delete(id); } else set.add(id);
      draw();
      onChange(set);
    } },
  }, h('i.box', set.has(id) ? icon('check') : null), h('b', name), compact ? null : h('span', t(what)))));
  draw();
  return wrap;
}
const fmtString = (set) => FORMATS().map((f) => f[0]).filter((f) => set.has(f)).join(',');

export const KIND_ICON = { pdf: 'pdf', docx: 'word', odt: 'word', pptx: 'slides', image: 'image', html: 'globe', md: 'note', txt: 'file' };
export const DOC_FILTER = ['pdf', 'docx', 'docm', 'dotx', 'pptx', 'odt', 'html', 'htm', 'md', 'markdown', 'txt', 'text', 'csv', 'rst', 'json', 'xml', 'yaml', 'yml', 'png', 'jpg', 'jpeg', 'tif', 'tiff', 'bmp', 'gif', 'webp'];

const fmtSize = (n) => (n < 1024 ? `${n} B` : n < 1048576 ? `${(n / 1024).toFixed(0)} KB` : `${(n / 1048576).toFixed(1)} MB`);
const docTitle = (d) => (d.title || '').trim() || d.name;
export const err = (e) => toast(String(e), 'err');

// ───────────────────────── sidebar ─────────────────────────

export function renderSidebar(ctx) {
  const { S } = ctx;
  const el = document.getElementById('sidebar');
  const list = h('div.side-tree.skill-list',
    S.skills.length ? S.skills.map((s) => h('div.skill-item' + (s.id === S.skillId ? '.sel' : ''), {
      title: s.name,
      on: {
        click: () => ctx.selectSkill(s.id),
        contextmenu: (e) => contextMenu(e, [
          { label: t('Megnyitás'), icon: 'open', action: () => ctx.selectSkill(s.id) },
          { label: t('Skill adatai…'), icon: 'edit', action: () => { ctx.selectSkill(s.id); ctx.setView('skill'); } },
          { label: t('Csomag építése'), icon: 'build', disabled: !s.done, action: async () => { await ctx.selectSkill(s.id); ctx.build(); } },
          { label: t('Kimeneti mappa megnyitása'), icon: 'folderOpen', action: () => ctx.revealOut(s.id) },
          '-',
          { label: t('Skill törlése…'), icon: 'trash', danger: true, action: () => ctx.deleteSkill(s) },
        ]),
      },
    }, icon('skill'),
    h('div.si-main', h('div.si-title', s.title), h('div.si-sub', t('{0} dokumentum · {1} oldal', s.docs, s.pages))),
    s.built ? h('span.si-built', { title: t('Utoljára építve: {0}', fmtDate(s.built, true)) }, icon('pkg')) : null))
      : h('div.empty-small', t('Még nincs skill. Hozz létre egyet, és húzz bele dokumentumokat.')));
  el.replaceChildren(
    h('div.side-head', h('span.side-title', t('Skillek')), h('span.grow'),
      h('button.icon-btn', { title: t('Új skill (Ctrl+N)'), on: { click: () => ctx.newSkill() } }, icon('plus'))),
    list,
  );
}

/** Compact engine indicator for the status bar: one icon + coloured dot, details in the tooltip. */
export function engineIndicator(ctx) {
  const st = ctx.S.engine;
  if (!st) return null;
  const ocr = st.ocr || {};
  const ocrText = ocr.active === 'windows' ? t('Windows OCR ({0})', (ctx.S.settings.ocr_lang || (ocr.windows_langs || []).map((l) => l.tag).join(', ')) || '?')
    : ocr.active === 'tesseract' ? `Tesseract (${ctx.S.settings.tesseract_lang || 'eng'})` : t('nincs');
  const ok = !!st.pdfium && !!ocr.active;
  const tip = `PDF: ${st.pdfium ? 'pdfium ✓' : t('pdfium hiányzik')}
OCR: ${ocrText}
${st.pdfium ? t('OCR beállítások') : t('Kattints a letöltéshez')}`;
  return h('span.eng-ind' + (ok ? '.ok' : '.warn'), { title: tip, on: { click: () => (st.pdfium ? ctx.settingsDlg('extract') : ctx.downloadPdfium()) } },
    icon('scan'), h('i.dot'));
}

// ───────────────────────── view tabs ─────────────────────────

export const VIEWS = [
  ['docs', 'Dokumentumok', 'pages'],
  ['text', 'Szöveg', 'note'],
  ['skill', 'Skill adatai', 'skill'],
  ['build', 'Csomag', 'pkg'],
  ['search', 'Keresés', 'search'],
];

export function renderViewTabs(ctx) {
  const { S } = ctx;
  const el = document.getElementById('tabs');
  if (!S.skillId) { el.replaceChildren(); el.classList.add('none'); return; }
  el.classList.remove('none');
  el.replaceChildren(...VIEWS.map(([id, label, ic]) => h('div.tab' + (S.view === id ? '.active' : ''), { on: { click: () => ctx.setView(id) } },
    icon(ic), h('span.name', t(label)),
    id === 'docs' && S.docs.length ? h('span.tab-count', String(S.docs.length)) : null)));
}

// ───────────────────────── documents ─────────────────────────

function statusCell(ctx, d) {
  const p = ctx.S.progress[d.id];
  if (d.status === 'processing' && p) {
    const pct = p.total ? Math.round((p.done * 100) / p.total) : 0;
    const label = p.stage === 'ocr' ? t('OCR {0}/{1}', p.done + 1, p.total) : p.stage === 'resume' ? t('Folytatás {0}/{1}', p.done + 1, p.total) : p.total ? t('{0}/{1} oldal', p.done, p.total) : t('Feldolgozás…');
    return h('div.st.proc', h('div.st-bar', h('i', { style: { width: (p.total ? pct : 30) + '%' }, class: p.total ? '' : 'indet' })), h('span', label));
  }
  const map = {
    new: [t('Új'), ''],
    queued: [t('Sorban áll'), 'q'],
    processing: [t('Feldolgozás…'), 'proc'],
    done: [t('Kész'), 'ok'],
    error: [t('Hiba'), 'err'],
  };
  let [label, cls] = map[d.status] || [d.status, ''];
  // an interrupted run: the finished pages are kept, processing continues from there
  if ((d.status === 'new' || d.status === 'error') && d.partial > 0) [label, cls] = [t('Folytatható ({0} oldal kész)', d.partial), 'q'];
  return h('div.st' + (cls ? '.' + cls : ''), { title: d.error || '' },
    d.status === 'done' ? icon('check') : d.status === 'error' ? icon('warnTri') : d.status === 'processing' ? icon('refresh', 'spin') : null,
    h('span', label),
    d.missing ? h('span.st-warn', { title: t('A fájl nem található: {0}', d.path) }, icon('warnTri')) : null,
    d.stale ? h('span.st-warn', { title: t('A fájl változott a feldolgozás óta – dolgozd fel újra.') }, icon('refresh')) : null);
}

function textSource(d) {
  if (d.status !== 'done') return '';
  if (d.kind === 'image') return 'OCR';
  if (d.kind === 'pdf') return d.ocr_pages === 0 ? t('szövegréteg') : d.ocr_pages >= d.pages ? 'OCR' : t('vegyes ({0} OCR)', d.ocr_pages);
  return d.kind.toUpperCase();
}

export function renderDocs(ctx, el) {
  const { S } = ctx;
  const sel = S.docSel;
  const docs = S.docs;
  const running = S.queue.length > 0;
  const selDocs = docs.filter((d) => sel.has(d.id));
  const toProcess = docs.filter((d) => d.status === 'new' || d.status === 'error' || d.stale);
  const head = h('div.docs-bar',
    h('button.btn', { on: { click: () => ctx.addFiles() } }, icon('addFiles'), t('Fájlok…')),
    h('button.btn', { on: { click: () => ctx.addFolder() } }, icon('folder'), t('Mappa…')),
    h('span.sep'),
    running ? h('button.btn.danger', { on: { click: () => ctx.cancel() } }, icon('stop'), t('Leállítás'))
      : h('button.btn.primary', { disabled: !toProcess.length && !selDocs.length, title: t('Az új, hibás és megváltozott dokumentumok feldolgozása. Ha sorokat jelöltél ki (kattintás, Ctrl/Shift + kattintás), csak azokat dolgozza fel; a pipa nem kijelölés, hanem hogy bekerül-e a skillbe.'),
        on: { click: () => ctx.process(selDocs.length ? selDocs.map((d) => d.id) : toProcess.map((d) => d.id)) } },
      icon('play'), selDocs.length ? t('Kijelölt sorok feldolgozása ({0})', selDocs.length) : toProcess.some((d) => d.partial > 0) ? t('Folytatás ({0})', toProcess.length) : t('Feldolgozás ({0})', toProcess.length)),
    selDocs.length ? h('span.sel-chip', { title: t('Sorok kijelölése: kattintás, Ctrl/Shift + kattintás. A pipa nem kijelölés: azt mutatja, hogy a dokumentum bekerül-e a skillbe.') },
      t('{0} sor kijelölve', selDocs.length),
      h('button.icon-btn', { title: t('Kijelölés megszüntetése (Esc)'), on: { click: () => { sel.clear(); ctx.renderContent(); } } }, icon('close'))) : null,
    h('span.grow'),
    selDocs.length ? h('button.btn', { on: { click: () => ctx.removeDocs(selDocs) } }, icon('trash'), t('Kijelölt sorok eltávolítása ({0})', selDocs.length)) : null,
    h('span.docs-sum', t('{0} dokumentum · {1} oldal · {2} kész', docs.length, docs.reduce((a, d) => a + d.pages, 0), docs.filter((d) => d.status === 'done').length)));

  if (!docs.length) {
    el.replaceChildren(h('div.docs', head, h('div.drop-hint', { on: { click: () => ctx.addFiles() } },
      icon('addFiles'), h('b', t('Húzd ide a dokumentumokat')),
      h('span', t('PDF, Word (DOCX), PowerPoint (PPTX), ODT, HTML, Markdown, szöveg és képek (OCR). Mappát is ráhúzhatsz.')),
      h('button.btn.primary', icon('addFiles'), t('Fájlok kiválasztása…')))));
    return;
  }

  let lastClick = null;
  const rowClick = (e, d, i) => {
    if (e.target.closest('input,button')) return;
    if (e.shiftKey && lastClick != null) {
      const [a, b] = [Math.min(lastClick, i), Math.max(lastClick, i)];
      for (let k = a; k <= b; k++) sel.add(docs[k].id);
    } else if (e.ctrlKey || e.metaKey) {
      if (sel.has(d.id)) sel.delete(d.id); else sel.add(d.id);
    } else {
      sel.clear();
      sel.add(d.id);
    }
    lastClick = i;
    ctx.renderContent();
  };
  const rows = docs.map((d, i) => h('tr' + (sel.has(d.id) ? '.sel' : '') + (d.include ? '' : '.off'), {
    on: {
      click: (e) => rowClick(e, d, i),
      dblclick: () => (d.status === 'done' ? ctx.openText(d.id) : ctx.editDoc(d)),
      contextmenu: (e) => { if (!sel.has(d.id)) { sel.clear(); sel.add(d.id); ctx.renderContent(); } docMenu(ctx, e, d); },
    },
  },
  h('td.c-inc', h('input', { type: 'checkbox', checked: d.include, title: t('Bekerül a skillbe'), on: { change: (e) => ctx.saveDoc(d, { include: e.target.checked }) } })),
  h('td.c-name', h('div.dn', icon(KIND_ICON[d.kind] || 'file'), h('div.dn-t', h('div.dn-title', docTitle(d)), docTitle(d) !== d.name ? h('div.dn-file', d.name) : null)),
    d.use_when ? h('div.dn-use', d.use_when) : d.summary ? h('div.dn-use', d.summary) : null),
  h('td.c-pages', d.status === 'done' ? String(d.pages) : ''),
  h('td.c-src', textSource(d), d.ocr_mode !== 'auto' ? h('span.ocr-mode', { title: t('OCR mód') }, d.ocr_mode === 'force' ? t('mindig OCR') : t('OCR nélkül')) : null,
    d.ocr_lang ? h('span.ocr-mode', { title: t('OCR nyelv') }, ocrLangLabel(ctx, d.ocr_lang)) : null),
  h('td.c-size', fmtSize(d.size)),
  h('td.c-st', statusCell(ctx, d)),
  h('td.c-act', h('button.icon-btn', { title: t('Műveletek'), on: { click: (e) => docMenu(ctx, e, d) } }, icon('chev')))));
  const table = h('table.doc-table',
    h('thead', h('tr', h('th.c-inc', { title: t('A bepipált dokumentumok kerülnek a skill csomagjába. Sorok kijelölése (feldolgozáshoz, törléshez): kattintás a sorra, Ctrl/Shift + kattintás.') }, t('Skillbe')), h('th', t('Dokumentum')), h('th.r', t('Oldal')), h('th', t('Szöveg')), h('th.r', t('Méret')), h('th', t('Állapot')), h('th'))),
    h('tbody', rows));
  el.replaceChildren(h('div.docs', head, h('div.doc-scroll', table),
    h('div.docs-foot', t('Tipp: dupla katt = szöveg megtekintése · jobb klikk = műveletek · Ctrl/Shift = több kijelölése · fájlok ráhúzhatók az ablakra'))));
}

function docMenu(ctx, e, d) {
  const { S } = ctx;
  const sel = S.docs.filter((x) => S.docSel.has(x.id));
  const many = sel.length > 1;
  const idx = S.docs.indexOf(d);
  contextMenu(e, [
    { label: t('Szöveg megtekintése'), icon: 'eye', disabled: d.status !== 'done', action: () => ctx.openText(d.id) },
    { label: t('Leírás szerkesztése…'), icon: 'edit', action: () => ctx.editDoc(d) },
    ctx.aiOn() ? { label: t('Leírás AI-val'), icon: 'sparkle', disabled: d.status !== 'done', action: () => ctx.aiDescribe(d) } : null,
    '-',
    { label: many ? t('Kijelölt sorok feldolgozása ({0})', sel.length) : t('Újrafeldolgozás'), icon: 'play', disabled: S.queue.includes(d.id), action: () => ctx.process((many ? sel : [d]).map((x) => x.id)) },
    { label: many ? t('Újra OCR más nyelvvel ({0})…', sel.length) : t('Újra OCR más nyelvvel…'), icon: 'scan', disabled: !(many ? sel : [d]).some((x) => x.kind === 'pdf' || x.kind === 'image') || S.queue.includes(d.id), action: () => reOcrDlg(ctx, (many ? sel : [d]).filter((x) => x.kind === 'pdf' || x.kind === 'image')) },
    { header: t('OCR mód') },
    ...[['auto', t('Automatikus (csak a szöveg nélküli oldalak)')], ['force', t('Mindig OCR (minden oldal)')], ['off', t('OCR nélkül')]].map(([m, l]) => (
      { label: l, icon: d.ocr_mode === m ? 'check' : null, action: async () => { for (const x of many ? sel : [d]) await ctx.saveDoc(x, { ocr_mode: m }, true); ctx.reloadDocs(); } })),
    '-',
    { label: t('Feljebb'), icon: 'up', disabled: idx <= 0, action: () => ctx.moveDoc(d, -1) },
    { label: t('Lejjebb'), icon: 'down', disabled: idx >= S.docs.length - 1, action: () => ctx.moveDoc(d, 1) },
    '-',
    { label: t('Megnyitás'), icon: 'open', disabled: d.missing, action: () => invoke('open_file', { path: d.path }).catch(err) },
    { label: t('Megjelenítés a mappában'), icon: 'folderOpen', disabled: d.missing, action: () => invoke('reveal_in_explorer', { path: d.path }).catch(err) },
    { label: t('Útvonal másolása'), icon: 'copy', action: () => navigator.clipboard.writeText(d.path).then(() => toast(t('Vágólapra másolva'), 'ok', 1500)) },
    '-',
    { label: many ? t('Eltávolítás ({0})', sel.length) : t('Eltávolítás a skillből'), icon: 'trash', danger: true, action: () => ctx.removeDocs(many ? sel : [d]) },
  ]);
}

/** Edit dialog of one document: title, summary, "use when", OCR mode, include. */
export async function editDocDlg(ctx, d) {
  const title = h('input.inp', { value: d.title || '', placeholder: d.name });
  const summary = h('textarea.inp', { rows: 4, placeholder: t('Miről szól a dokumentum? (bekerül a DOCUMENTS.md-be)') });
  summary.value = d.summary || '';
  const useWhen = h('textarea.inp', { rows: 3, placeholder: t('Mikor nyúljon ehhez a dokumentumhoz a modell? Pl. „ha a kérdés a garanciális feltételekről szól…”') });
  useWhen.value = d.use_when || '';
  const mode = h('select.inp', [['auto', t('Automatikus (csak a szöveg nélküli oldalak)')], ['force', t('Mindig OCR (minden oldal)')], ['off', t('OCR nélkül')]].map(([v, l]) => h('option', { value: v }, l)));
  mode.value = d.ocr_mode;
  const lang = ocrLangSelect(ctx, d.ocr_lang || '');
  const inc = h('input', { type: 'checkbox', checked: d.include });
  const aiOut = h('span.note');
  const aiBtn = ctx.aiOn() ? h('button.btn.ai', { type: 'button', disabled: d.status !== 'done' || null, on: { click: async () => {
    aiBtn.disabled = true;
    aiOut.className = 'note';
    aiOut.textContent = t('Az AI dolgozik…');
    try {
      const r = await invoke('ai_describe', { id: d.id });
      if (r.title && !title.value.trim()) title.value = r.title;
      summary.value = r.summary;
      useWhen.value = r.use_when;
      aiOut.textContent = '✓ ' + t('Kitöltve – ellenőrizd, majd Mentés.');
      aiOut.className = 'note ok';
    } catch (e) { aiOut.textContent = '✗ ' + e; aiOut.className = 'note err'; }
    finally { aiBtn.disabled = false; }
  } } }, icon('sparkle'), t('Kitöltés AI-val')) : null;
  const body = h('div.settings-grid',
    h('div.field.full.doc-file', icon(KIND_ICON[d.kind] || 'file'), h('span', d.path)),
    h('label.field.full', h('span', t('Cím')), title),
    h('label.field.full', h('span', t('Összefoglaló')), summary),
    h('label.field.full', h('span', t('Használd ha…')), useWhen, h('span.note', t('A SKILL.md dokumentumlistájába kerül: ez segít a modellnek eldönteni, melyik forrást nyissa meg.'))),
    h('label.field', h('span', t('OCR mód')), mode, h('span.note', t('Módosítás után dolgozd fel újra.'))),
    h('label.field', h('span', t('OCR nyelv')), lang, h('span.note', t('Csak ennél a dokumentumnál; az alapértelmezés a Beállítások → Szövegkinyerés.'))),
    h('label.chk.full', inc, t('Bekerül a skillbe')),
    aiBtn ? h('div.field.full', h('div', { style: { display: 'flex', gap: '8px', alignItems: 'center' } }, aiBtn, aiOut)) : null);
  const ok = await modal({ title: t('Dokumentum – {0}', d.name), body, wide: true, buttons: [{ label: t('Mégse'), value: false }, { label: t('Mentés'), value: true, primary: true }] });
  if (!ok) return;
  const modeChanged = mode.value !== d.ocr_mode || lang.value !== (d.ocr_lang || '');
  await ctx.saveDoc(d, { title: title.value, summary: summary.value, use_when: useWhen.value, ocr_mode: mode.value, ocr_lang: lang.value, include: inc.checked });
  if (modeChanged && d.status === 'done' && d.ocr_mode !== 'off' && await confirmDlg(t('OCR'), t('Feldolgozod újra a dokumentumot az új OCR beállítással?'), t('Feldolgozás'))) ctx.process([d.id]);
}

/** Re-OCR with another language / mode: saves the choice on the documents and processes them again. */
async function reOcrDlg(ctx, docs) {
  if (!docs.length) return;
  const d = docs[0];
  const lang = ocrLangSelect(ctx, d.ocr_lang || '');
  const mode = h('select.inp', [['auto', t('Automatikus (csak a szöveg nélküli oldalak)')], ['force', t('Mindig OCR (minden oldal)')]].map(([v, l]) => h('option', { value: v }, l)));
  mode.value = d.ocr_mode === 'force' ? 'force' : 'auto';
  const ocr = ctx.S.engine?.ocr || {};
  const hasHu = (ocr.windows_langs || []).some((l) => /^hu/i.test(l.tag)) || (ocr.tesseract_langs || []).includes('hun');
  const cmd = 'Add-WindowsCapability -Online -Name "Language.OCR~~~hu-HU~0.0.1.0"';
  const huHelp = hasHu ? null : h('div.field.full.ocr-help',
    h('b', icon('warnTri'), t('A magyar OCR nyelv nincs telepítve ezen a gépen.')),
    h('span', t('Az angol OCR az ékezeteket rosszul olvassa (á → å, ő → ö, ű → ü). Telepítsd a magyar Windows OCR-t: Gépház → Idő és nyelv → Nyelv és régió → Nyelv hozzáadása → Magyar (az „Optikai karakterfelismerés” legyen bejelölve), vagy rendszergazdai PowerShellben:')),
    h('div.ocr-cmd', h('code', cmd), h('button.icon-btn', { type: 'button', title: t('Másolás'), on: { click: () => navigator.clipboard.writeText(cmd).then(() => toast(t('Vágólapra másolva'), 'ok', 1200)) } }, icon('copy'))),
    h('div', { style: { display: 'flex', gap: '8px', alignItems: 'center' } },
      h('button.btn.sm', { type: 'button', on: { click: () => invoke('open_url', { url: 'ms-settings:regionlanguage' }).catch(err) } }, icon('open'), t('Nyelvi beállítások megnyitása')),
      h('span.note', t('Telepítés után indítsd újra a SkillStart, és a nyelv megjelenik a listában. Tesseract esetén a hun nyelvi csomag kell.'))));
  const body = h('div.settings-grid',
    h('div.field.full.note', docs.length > 1 ? t('{0} dokumentum OCR-je készül újra.', docs.length) : d.title || d.name),
    h('label.field', h('span', t('OCR nyelv')), lang),
    h('label.field', h('span', t('OCR mód')), mode, h('span.note', t('Szkennelt könyvnél mindegy; a „Mindig OCR” a rossz szövegréteget is felülírja.'))),
    huHelp,
    h('div.field.full.note', t('A szerkesztett oldalszövegek (javítások) elvesznek; a leírás és a cím megmarad.')));
  const ok = await modal({ title: t('Újra OCR'), body, wide: true, buttons: [{ label: t('Mégse'), value: false }, { label: t('Újra OCR'), value: true, primary: true }] });
  if (!ok) return;
  for (const x of docs) await ctx.saveDoc(x, { ocr_lang: lang.value, ocr_mode: mode.value }, true);
  ctx.process(docs.map((x) => x.id));
}

/** Readable name of a document OCR language value ("win:hu-HU", "tess:hun+eng"). */
export function ocrLangLabel(ctx, v) {
  if (!v) return '';
  if (v.startsWith('win:')) {
    const tag = v.slice(4);
    const l = (ctx.S.engine?.ocr?.windows_langs || []).find((x) => x.tag === tag);
    return l ? l.name : tag;
  }
  return 'Tesseract ' + v.slice(5);
}

/** OCR language of one document: settings default, an installed Windows OCR language, or Tesseract languages. */
function ocrLangSelect(ctx, value) {
  const ocr = ctx.S.engine?.ocr || {};
  const opts = [['', t('Alapértelmezett (Beállítások)')]];
  const win = (ocr.windows_langs || []).map((l) => ['win:' + l.tag, `Windows OCR – ${l.name} (${l.tag})`]);
  const tl = ocr.tesseract_langs || [];
  const tess = ocr.tesseract ? [...new Set(['hun', 'eng', 'hun+eng', 'deu', 'eng+deu', ...tl.filter((x) => x !== 'osd')])]
    .filter((x) => !tl.length || x.split('+').every((p) => tl.includes(p))).map((x) => ['tess:' + x, `Tesseract – ${x}`]) : [];
  const all = [...opts, ...win, ...tess];
  if (value && !all.some((o) => o[0] === value)) all.push([value, `${ocrLangLabel(ctx, value)} (${t('nincs telepítve')})`]);
  const s = h('select.inp', all.map(([v, l]) => h('option', { value: v }, l)));
  s.value = value;
  return s;
}

// ───────────────────────── page viewer (original look of a PDF page / image) ─────────────────────────

export const hasPageImage = (kind) => kind === 'pdf' || kind === 'image';

/**
 * Shows the original page: PDF pages rendered by pdfium (page by page, zoom), images as they are.
 * src: { doc_id, doc (title), page, kind }. "Text" jumps to the extracted text of the same page.
 */
/** Lower case without accents, punctuation trimmed: how words are compared on the page. */
export const foldWord = (w) => String(w).normalize('NFD').replace(/\p{M}/gu, '').toLowerCase().replace(/^[^\p{L}\p{N}$]+|[^\p{L}\p{N}]+$/gu, '');

/**
 * What to highlight on a page image. `words`: [{t, x, y, w, h}] in page fractions;
 * `passage`: the RAG passage of this page (its parts joined with "\n…\n");
 * `terms`: folded search stems. The passage is found by aligning its first and last
 * words with the page; if it is (nearly) the whole page, the sentences with the most
 * search words are marked instead.
 */
export function pageMarks(words, passage, terms) {
  const toks = words.map((w) => foldWord(w.t));
  const isTerm = (tk) => tk.length >= 2 && terms.some((t) => (t.length <= 3 ? tk === t : tk.startsWith(t)));
  const termIdx = toks.map((tk, i) => (isTerm(tk) ? i : -1)).filter((i) => i >= 0);
  const span = new Set();
  const K = 6;
  // where a passage part starts (or ends) on the page: a page word that is one of the part's
  // first (last) words, followed (preceded) by most of the others – a missing or misread word is fine
  const align = (part, fromEnd) => {
    const p = fromEnd ? part.slice(-K) : part.slice(0, K);
    let best = -1;
    let bestScore = 0;
    for (let i = 0; i < toks.length; i++) {
      if (toks[i] !== (fromEnd ? p[p.length - 1] : p[0]) && toks[i] !== (fromEnd ? p[p.length - 2] : p[1])) continue;
      const win = fromEnd ? toks.slice(Math.max(0, i - K - 1), i + 1) : toks.slice(i, i + K + 2);
      let sc = 0;
      let at = 0;
      for (const w of p) {
        const k = win.indexOf(w, at);
        if (k >= 0) { sc++; at = k + 1; }
      }
      if (sc > bestScore) { bestScore = sc; best = i; }
    }
    return bestScore >= Math.min(3, p.length) ? best : -1;
  };
  if (passage) {
    for (const part of passage.split(/\n…\n/)) {
      const pt = part.split(/\s+/).map(foldWord).filter(Boolean);
      if (pt.length < 3) continue;
      const a = align(pt, false);
      const b = align(pt, true);
      if (a >= 0 && b >= a) for (let i = a; i <= b; i++) span.add(i);
      else if (a >= 0) for (let i = a; i < Math.min(toks.length, a + pt.length); i++) span.add(i);
    }
  }
  // the passage is the whole page: the sentences richest in search words instead
  if (span.size > toks.length * 0.85 || (!span.size && passage)) {
    span.clear();
    const sents = [];
    let start = 0;
    toks.forEach((_, i) => { if (/[.!?:;]$/.test(words[i].t) || i === toks.length - 1) { sents.push([start, i]); start = i + 1; } });
    const scored = sents.map(([a, b]) => {
      const hit = new Set();
      for (let i = a; i <= b; i++) if (isTerm(toks[i])) terms.forEach((t) => { if (toks[i].startsWith(t)) hit.add(t); });
      return { a, b, n: hit.size };
    }).filter((x) => x.n >= Math.min(2, terms.length) && x.n > 0).sort((x, y) => y.n - x.n || x.a - y.a).slice(0, 3);
    for (const sc of scored) for (let i = sc.a; i <= sc.b; i++) span.add(i);
  }
  // span words → one box per line
  const spans = [];
  let cur = null;
  [...span].sort((a, b) => a - b).forEach((i) => {
    const w = words[i];
    if (cur && cur.last === i - 1 && Math.abs(w.y - cur.y) < Math.max(w.h, cur.h) * 0.6 && w.x >= cur.x - 0.01) {
      const r = Math.max(cur.x + cur.w, w.x + w.w);
      const bot = Math.max(cur.y + cur.h, w.y + w.h);
      cur.y = Math.min(cur.y, w.y);
      cur.w = r - cur.x;
      cur.h = bot - cur.y;
      cur.last = i;
    } else {
      cur = { x: w.x, y: w.y, w: w.w, h: w.h, last: i };
      spans.push(cur);
    }
  });
  const pad = (r) => ({ x: r.x - 0.003, y: r.y - 0.002, w: r.w + 0.006, h: r.h + 0.004 });
  return { spans: spans.map(pad), terms: termIdx.map((i) => pad(words[i])) };
}

export function pageViewer(ctx, src) {
  document.querySelector('.pv-ov')?.remove();
  let page = Math.max(1, src.page || 1);
  let pages = 0;
  let zoom = 0; // 0 = whole page, 1 = fit width, other = zoom of the width
  const cache = new Map();
  const img = h('img.pv-img', { alt: '' });
  const marks = h('div.pv-marks');
  const sheet = h('div.pv-page', img, marks);
  const status = h('div.pv-status');
  const scroller = h('div.pv-body', sheet, status);
  // highlighting: the RAG passage on its page, the search words on every page
  const terms = (src.terms || []).map(foldWord).filter((x) => x.length >= 2);
  const canMark = !!(src.text || terms.length);
  let showMarks = canMark;
  const words = new Map();
  const pageInp = h('input.inp.pg-inp', { type: 'number', min: 1, value: page, on: { change: (e) => go(+e.target.value || 1) } });
  const count = h('span.sub');
  const zoomLbl = h('span.sub.pv-zoom');
  const want = () => Math.min(3000, Math.max(400, Math.round((scroller.clientWidth - 40) * (zoom || 1) * (window.devicePixelRatio || 1) / 200) * 200));
  let seq = 0;
  async function load() {
    const my = ++seq;
    const w = want();
    const key = `${page}@${w}`;
    status.textContent = t('Betöltés…');
    status.className = 'pv-status';
    try {
      let r = cache.get(key);
      if (!r) {
        r = await invoke('page_image', { id: src.doc_id, page, width: w });
        cache.set(key, r);
      }
      if (my !== seq) return;
      pages = r.pages;
      page = r.page;
      img.src = r.url;
      const fitPage = zoom === 0;
      img.style.width = fitPage ? 'auto' : zoom === 1 ? 'min(100%, 1400px)' : `${Math.round((scroller.clientWidth - 40) * zoom)}px`;
      img.style.maxWidth = fitPage ? '100%' : 'none';
      img.style.maxHeight = fitPage ? `${scroller.clientHeight - 40}px` : 'none';
      status.textContent = '';
      pageInp.value = page;
      pageInp.max = pages;
      count.textContent = `/ ${pages}`;
      zoomLbl.textContent = zoom === 0 || zoom === 1 ? '' : `${Math.round(zoom * 100)}%`;
      fitBtn.classList.toggle('on', zoom === 0);
      widthBtn.classList.toggle('on', zoom === 1);
      prev.disabled = page <= 1;
      next.disabled = page >= pages;
      drawMarks(my);
    } catch (e) {
      if (my !== seq) return;
      status.textContent = String(e);
      status.className = 'pv-status err';
    }
  }
  async function drawMarks(my) {
    marks.replaceChildren();
    markBtn?.classList.toggle('on', showMarks);
    if (!canMark || !showMarks) return;
    let ws = words.get(page);
    if (!ws) {
      status.textContent = t('Kiemelés…');
      try { ws = (await invoke('page_words', { id: src.doc_id, page })).words; } catch { ws = []; }
      words.set(page, ws);
      if (my !== seq) return;
      status.textContent = '';
    }
    const m = pageMarks(ws, page === src.page ? src.text : '', terms);
    for (const r of m.spans) marks.append(h('i.pv-mk.span', { style: box(r) }));
    for (const r of m.terms) marks.append(h('i.pv-mk.term', { style: box(r) }));
    if (!m.spans.length && !m.terms.length && page === src.page) status.textContent = t('Ezen az oldalon nem találtam kiemelhető részt.');
    const first = marks.querySelector('.pv-mk.span') || marks.querySelector('.pv-mk.term');
    if (first && zoom !== 0) first.scrollIntoView({ block: 'center' });
  }
  const box = (r) => ({ left: `${r.x * 100}%`, top: `${r.y * 100}%`, width: `${r.w * 100}%`, height: `${r.h * 100}%` });
  const go = (n) => { page = Math.max(1, pages ? Math.min(pages, n) : n); scroller.scrollTop = 0; load(); };
  const setZoom = (z) => { zoom = z === 0 ? 0 : Math.max(0.5, Math.min(3, z)); load(); };
  const step = (d) => setZoom((zoom || 1) + d);
  const markBtn = canMark ? h('button.btn.sm.pv-fit', { title: t('A releváns részek kiemelése (H)'), on: { click: () => { showMarks = !showMarks; drawMarks(seq); } } }, icon('fmtMark'), t('Kiemelés')) : null;
  const fitBtn = h('button.btn.sm.pv-fit', { title: t('Egész oldal (0)'), on: { click: () => setZoom(0) } }, t('Egész oldal'));
  const widthBtn = h('button.btn.sm.pv-fit', { title: t('Oldalszélesség (1)'), on: { click: () => setZoom(1) } }, t('Szélesség'));
  const prev = h('button.icon-btn', { title: t('Előző oldal (←)'), on: { click: () => go(page - 1) } }, icon('dragUp'));
  const next = h('button.icon-btn', { title: t('Következő oldal (→)'), on: { click: () => go(page + 1) } }, icon('chev'));
  const close = () => { ov.remove(); document.removeEventListener('keydown', onKey, true); };
  const toText = () => { close(); ctx.S.textDoc = src.doc_id; ctx.S.textPage = page; ctx.S.textFind = ''; ctx.setView('text'); };
  const onKey = (e) => {
    if (e.target.tagName === 'INPUT') return;
    if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
    else if (e.key === 'ArrowRight' || e.key === 'PageDown') { e.preventDefault(); go(page + 1); }
    else if (e.key === 'ArrowLeft' || e.key === 'PageUp') { e.preventDefault(); go(page - 1); }
    else if (e.key === '+' || e.key === '=') { e.preventDefault(); step(0.25); }
    else if (e.key === '-') { e.preventDefault(); step(-0.25); }
    else if (e.key === '0') { e.preventDefault(); setZoom(0); }
    else if (e.key === '1') { e.preventDefault(); setZoom(1); }
    else if ((e.key === 'h' || e.key === 'H') && canMark) { e.preventDefault(); showMarks = !showMarks; drawMarks(seq); }
  };
  const ov = h('div.pv-ov', { on: { mousedown: (e) => { if (e.target === ov) close(); } } },
    h('div.pv',
      h('div.pane-h',
        icon(src.kind === 'pdf' ? 'pdf' : 'image'), h('span.pv-title', { title: src.doc }, src.doc), h('span.grow'),
        src.kind === 'pdf' ? [prev, pageInp, count, next, h('span.pv-sep')] : null,
        h('button.icon-btn', { title: t('Kicsinyítés (−)'), on: { click: () => step(-0.25) } }, h('b', '−')), zoomLbl,
        h('button.icon-btn', { title: t('Nagyítás (+)'), on: { click: () => step(0.25) } }, h('b', '+')),
        fitBtn, widthBtn,
        markBtn ? [h('span.pv-sep'), markBtn] : null,
        h('span.pv-sep'),
        h('button.btn.sm', { title: t('A kinyert szöveg ezen az oldalon'), on: { click: toText } }, icon('note'), t('Szöveg')),
        h('button.icon-btn', { title: t('Bezárás (Esc)'), on: { click: close } }, icon('close'))),
      scroller));
  document.body.append(ov);
  document.addEventListener('keydown', onKey, true);
  requestAnimationFrame(load);
}

// ───────────────────────── text ─────────────────────────

export function renderText(ctx, el) {
  const { S } = ctx;
  const done = S.docs.filter((d) => d.status === 'done');
  if (!done.length) {
    el.replaceChildren(h('div.empty', icon('note'), t('Még nincs feldolgozott dokumentum.'), h('button.btn.primary', { on: { click: () => ctx.setView('docs') } }, t('Dokumentumok'))));
    return;
  }
  if (!done.some((d) => d.id === S.textDoc)) { S.textDoc = done[0].id; S.textPage = 1; }
  const docList = h('div.tx-docs', done.map((d) => h('div.tx-doc' + (d.id === S.textDoc ? '.sel' : ''), {
    title: d.name, on: { click: () => { S.textDoc = d.id; S.textPage = 1; ctx.renderContent(); } },
  }, icon(KIND_ICON[d.kind] || 'file'), h('span', docTitle(d)), h('span.cnt', String(d.pages)))));
  const main = h('div.tx-main', h('div.empty', icon('refresh', 'spin')));
  el.replaceChildren(h('div.tx', docList, main));
  loadText(ctx, main);
}

const textCache = new Map();
export const forgetText = (id) => (id == null ? textCache.clear() : textCache.delete(id));

async function loadText(ctx, main) {
  const { S } = ctx;
  const id = S.textDoc;
  let data = textCache.get(id);
  if (!data) {
    try { data = await invoke('document_text', { id }); } catch (e) { main.replaceChildren(h('div.empty', String(e))); return; }
    textCache.set(id, data);
  }
  if (S.textDoc !== id) return;
  const pages = data.pages;
  const total = pages.length;
  S.textPage = Math.max(1, Math.min(total, S.textPage || 1));
  const page = pages[S.textPage - 1];
  const ta = h('textarea.ed-text.tx-text', { spellcheck: 'false' });
  ta.value = page?.text || '';
  let dirty = false;
  const saveBtn = h('button.btn.sm', { disabled: true, on: { click: () => save() } }, icon('save'), t('Mentés'));
  const save = async () => {
    if (!dirty) return true;
    try {
      await invoke('save_page', { id, no: page.no, text: ta.value });
      page.text = ta.value;
      dirty = false;
      saveBtn.disabled = true;
      toast(t('Oldal mentve'), 'ok', 1200);
      return true;
    } catch (e) { err(e); return false; }
  };
  ta.addEventListener('input', () => { dirty = ta.value !== page.text; saveBtn.disabled = !dirty; });
  ta.addEventListener('keydown', (e) => { if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 's') { e.preventDefault(); e.stopPropagation(); save(); } });
  const go = async (n) => {
    if (dirty && !(await save())) return;
    S.textPage = Math.max(1, Math.min(total, n));
    loadText(ctx, main);
  };
  const pageInp = h('input.inp.pg-inp', { type: 'number', min: 1, max: total, value: S.textPage, on: { change: (e) => go(+e.target.value || 1) } });
  const outline = data.outline.length ? h('div.tx-outline', h('div.pane-h', icon('outline'), t('Tartalom')),
    h('div.tx-ol', data.outline.map((o) => h('div.ol-item' + (o.page === S.textPage ? '.sel' : ''), {
      style: { paddingLeft: (6 + Math.min(4, o.level - 1) * 12) + 'px' }, title: o.title,
      on: { click: () => o.page > 0 && go(o.page) },
    }, h('span', o.title), o.page > 0 ? h('span.cnt', String(o.page)) : null)))) : null;
  main.replaceChildren(
    h('div.pane-h', h('span', docTitle(data.doc)), h('span.sub', `· ${data.doc.name}`), h('span.grow'),
      page?.ocr ? h('span.ocr-badge', { title: t('Ezt az oldalt OCR olvasta: javíthatod a szöveget, a skill a javított változatot kapja.') }, icon('scan'), 'OCR') : null,
      h('button.icon-btn', { title: t('Előző oldal (PgUp)'), disabled: S.textPage <= 1 || null, on: { click: () => go(S.textPage - 1) } }, icon('dragUp')),
      pageInp, h('span.sub', `/ ${total}`),
      h('button.icon-btn', { title: t('Következő oldal (PgDn)'), disabled: S.textPage >= total || null, on: { click: () => go(S.textPage + 1) } }, icon('chev')),
      saveBtn,
      hasPageImage(data.doc.kind) ? h('button.btn.sm', { title: t('Az oldal eredeti képe (Ctrl+P)'), on: { click: () => pageViewer(ctx, { doc_id: data.doc.id, doc: docTitle(data.doc), page: S.textPage, kind: data.doc.kind }) } }, icon(data.doc.kind === 'pdf' ? 'pdf' : 'image'), data.doc.kind === 'pdf' ? 'PDF' : t('Kép')) : null,
      h('button.icon-btn', { title: t('Eredeti fájl megnyitása'), on: { click: () => invoke('open_file', { path: data.doc.path }).catch(err) } }, icon('open'))),
    h('div.tx-body', outline, ta));
  main.onkeydown = (e) => {
    if (e.target === pageInp) return;
    if (e.key === 'PageDown' && e.ctrlKey) { e.preventDefault(); go(S.textPage + 1); }
    if (e.key === 'PageUp' && e.ctrlKey) { e.preventDefault(); go(S.textPage - 1); }
    if (e.key.toLowerCase() === 'p' && e.ctrlKey && hasPageImage(data.doc.kind)) { e.preventDefault(); pageViewer(ctx, { doc_id: data.doc.id, doc: docTitle(data.doc), page: S.textPage, kind: data.doc.kind }); }
  };
  if (S.textFind) {
    const i = ta.value.toLowerCase().indexOf(S.textFind.toLowerCase());
    S.textFind = '';
    if (i >= 0) setTimeout(() => { ta.focus(); ta.setSelectionRange(i, i + 1); const lines = ta.value.slice(0, i).split('\n').length; ta.scrollTop = Math.max(0, (lines - 3) * 18); }, 30);
  }
}

// ───────────────────────── skill metadata ─────────────────────────

export function renderSkillForm(ctx, el) {
  const { S } = ctx;
  const sk = S.skills.find((s) => s.id === S.skillId);
  if (!sk) return;
  const f = {};
  const inp = (k, attrs = {}) => (f[k] = h('input.inp', { value: sk[k] ?? '', spellcheck: 'false', ...attrs }));
  const area = (k, rows, ph) => { const a = h('textarea.inp', { rows, placeholder: ph || '' }); a.value = sk[k] ?? ''; f[k] = a; return a; };
  const lang = h('select.inp', [['hu', 'Magyar'], ['en', 'English']].map(([v, l]) => h('option', { value: v }, l)));
  lang.value = sk.language || 'hu';
  f.language = lang;
  const descOut = h('span.note');
  const aiDesc = ctx.aiOn() ? h('button.btn.ai', { type: 'button', disabled: !sk.done || null, on: { click: async () => {
    aiDesc.disabled = true;
    descOut.className = 'note';
    descOut.textContent = t('Az AI dolgozik…');
    try {
      f.description.value = await invoke('ai_skill_description', { id: sk.id });
      descOut.textContent = '✓ ' + t('Kész – ellenőrizd, majd Mentés.');
      descOut.className = 'note ok';
    } catch (e) { descOut.textContent = '✗ ' + e; descOut.className = 'note err'; }
    finally { aiDesc.disabled = false; }
  } } }, icon('sparkle'), t('Leírás AI-val')) : null;
  const outDir = inp('out_dir', { placeholder: ctx.outRootLabel() });
  const browse = h('button.btn', { type: 'button', on: { click: async () => {
    const d = await invoke('plugin:dialog|open', { options: { directory: true, multiple: false, title: t('Kimeneti mappa') } }).catch(() => null);
    if (d) outDir.value = Array.isArray(d) ? d[0] : d;
  } } }, t('Tallózás…'));
  let formats = fmtSet(sk);
  const save = async () => {
    const next = { ...sk };
    for (const k of Object.keys(f)) next[k] = f[k].value;
    next.formats = fmtString(formats);
    try {
      const saved = await invoke('update_skill', { skill: next });
      Object.assign(sk, saved);
      toast(t('Mentve'), 'ok', 1200);
      ctx.refreshSkills();
    } catch (e) { err(e); }
  };
  el.replaceChildren(h('div.form-page', h('div.form-box',
    h('h2', icon('skill'), t('Skill adatai')),
    h('div.settings-grid',
      h('label.field', h('span', t('Név')), inp('title')),
      h('label.field', h('span', t('Azonosító (mappa- és skill-név)')), inp('name'), h('span.note', t('kisbetű, szám, kötőjel; a „claude” és „anthropic” szó nem lehet benne'))),
      h('label.field.full', h('span', t('Leírás (mikor használja a modell)')), area('description', 5, t('Üresen hagyva a SkillStar állítja össze a dokumentumok címéből és kulcsszavaiból.')),
        h('div', { style: { display: 'flex', gap: '8px', alignItems: 'center', marginTop: '4px' } }, aiDesc, descOut,
          h('span.note', t('A Claude ez alapján dönti el, mikor töltse be a skillt: írd bele a témákat és a jellemző kérdéseket. Legfeljebb 1024 karakter.')))),
      h('label.field.full', h('span', t('Kulcsszavak')), inp('keywords', { placeholder: t('vesszővel elválasztva – a leírásba és a plugin.json-ba kerülnek') })),
      h('label.field', h('span', t('Verzió')), inp('version')),
      h('label.field', h('span', t('Szerző')), inp('author', { placeholder: S.settings.author || '' })),
      h('label.field', h('span', t('A generált szövegek nyelve')), lang),
      h('div'),
      h('div.field.full', h('span', t('Csomagformátumok')), formatChips(sk, (set) => { formats = set; }),
        h('span.note', t('Minden bekapcsolt formátum a kimeneti mappa saját almappájába kerül (claude/, agents/, chatgpt/, gemini/, deepseek/, rag/).'))),
      h('label.field.full', h('span', t('További utasítások a modellnek')), area('instructions', 6, t('Pl. „Mindig a legfrissebb szabályzatot vedd alapul.” vagy „Kódot Kick Assembler szintaxissal adj.” – Markdown, a SKILL.md végére kerül.'))),
      h('label.field.full', h('span', t('Kimeneti mappa (csak ennél a skillnél)')), h('div', { style: { display: 'flex', gap: '6px' } }, outDir, browse))),
    h('div.form-foot', h('span.note', t('Létrehozva: {0} · módosítva: {1}', fmtDate(sk.created, true), relTime(sk.updated))), h('span.grow'),
      h('button.btn.primary', { on: { click: save } }, icon('save'), t('Mentés'))))));
}

// ───────────────────────── package: preview + build ─────────────────────────

export function renderBuild(ctx, el) {
  const { S } = ctx;
  const sk = S.skills.find((s) => s.id === S.skillId);
  const res = S.buildResult?.skill === S.skillId ? S.buildResult.res : null;
  const fileList = h('div.pk-files', h('div.empty-small', t('Betöltés…')));
  const viewer = h('div.pk-view');
  const raw = { on: S.pkRaw };
  const result = res ? h('div.pk-result',
    h('div.pk-ok', icon('check'), h('b', t('Kész a csomag')), h('span.sub', t('{0} dokumentum · {1} oldal · {2} kulcsszó', res.documents, res.pages, res.terms))),
    h('div.pk-paths', pathRow(t('Kimeneti mappa'), res.dir, true), ...(res.outputs || []).map((o) => pathRow(o.label, o.path, o.dir))),
    res.warnings.length ? h('div.pk-warn', res.warnings.map((w) => h('div', icon('warnTri'), w))) : null) : null;
  el.replaceChildren(h('div.pk',
    h('div.pk-head',
      h('div', h('h2', icon('pkg'), t('Skill csomag')),
        h('div.sub', sk?.built ? t('Utoljára építve: {0}', fmtDate(sk.built, true)) : t('Még nem építetted meg.'))),
      sk ? formatChips(sk, async (set) => {
        try {
          const saved = await invoke('update_skill', { skill: { ...sk, formats: fmtString(set) } });
          Object.assign(sk, saved);
          S.pkSel = '';
          renderBuild(ctx, el);
        } catch (e) { err(e); }
      }, true) : null,
      h('span.grow'),
      h('button.btn', { on: { click: () => ctx.revealOut(S.skillId) } }, icon('folderOpen'), t('Kimeneti mappa')),
      h('button.btn.primary', { disabled: !sk?.done || null, on: { click: () => ctx.build() } }, icon('build'), t('Csomag építése'))),
    result,
    h('div.pk-body', fileList, viewer)));
  invoke('preview_skill', { id: S.skillId }).then((files) => {
    let selPath = S.pkSel && files.some((f) => f.path === S.pkSel) ? S.pkSel
      : (files.find((f) => f.path.endsWith('SKILL.md')) || files.find((f) => /instructions\.md$|system-prompt\.md$/.test(f.path)) || files[0])?.path;
    const show = () => {
      const f = files.find((x) => x.path === selPath);
      fileList.querySelectorAll('.pk-file').forEach((x) => x.classList.toggle('sel', x.dataset.p === selPath));
      if (!f) return;
      S.pkSel = selPath;
      const md = /\.md$/i.test(f.path) && !raw.on;
      viewer.replaceChildren(
        h('div.pane-h', icon(md ? 'eye' : 'file'), h('span', f.path), h('span.sub', `· ${fmtSize(f.size)}`), h('span.grow'),
          /\.md$/i.test(f.path) ? h('button.btn.sm', { on: { click: () => { raw.on = !raw.on; S.pkRaw = raw.on; show(); } } }, raw.on ? t('Előnézet') : t('Forrás')) : null,
          h('button.icon-btn', { title: t('Másolás'), on: { click: () => navigator.clipboard.writeText(f.text).then(() => toast(t('Vágólapra másolva'), 'ok', 1200)) } }, icon('copy'))),
        md ? h('div.md-view.pk-md', { html: mdRender(f.text.length > 120000 ? f.text.slice(0, 120000) + '\n\n…' : f.text) }) : h('pre.pk-raw', f.text));
    };
    // folders as their own rows: claude/, chatgpt/knowledge/ …
    const tree = [];
    let open = [];
    for (const f of files) {
      const parts = f.path.split('/');
      const dirs = parts.slice(0, -1);
      let same = 0;
      while (same < open.length && same < dirs.length && open[same] === dirs[same]) same++;
      for (let i = same; i < dirs.length; i++) {
        tree.push(h('div.pk-dir', { style: { paddingLeft: (8 + i * 12) + 'px' }, title: dirs.slice(0, i + 1).join('/') }, icon('folder'), h('span', dirs[i] + '/')));
      }
      open = dirs;
      tree.push(h('div.pk-file', { 'data-p': f.path, style: { paddingLeft: (8 + dirs.length * 12) + 'px' }, title: f.path, on: { click: () => { selPath = f.path; show(); } } },
        icon(/\.jsonl?$/.test(f.path) ? 'file' : f.path.endsWith('SKILL.md') ? 'skill' : 'note'), h('span', parts[parts.length - 1]), h('span.cnt', fmtSize(f.size))));
    }
    fileList.replaceChildren(h('div.pane-h', icon('folder'), sk?.name || ''), h('div.pk-tree', tree));
    show();
  }).catch((e) => {
    fileList.replaceChildren(h('div.empty-small', String(e)));
    viewer.replaceChildren(h('div.empty', icon('pkg'), t('A csomag előnézete itt jelenik meg, ha van feldolgozott dokumentum.')));
  });
}

function pathRow(label, path, dir = false) {
  return h('div.pk-path', h('span.k', label), h('code', { title: path }, path),
    h('button.icon-btn', { title: t('Másolás'), on: { click: () => navigator.clipboard.writeText(path).then(() => toast(t('Vágólapra másolva'), 'ok', 1200)) } }, icon('copy')),
    h('button.icon-btn', { title: t('Megjelenítés a mappában'), on: { click: () => invoke('reveal_in_explorer', { path }).catch(err) } }, icon(dir ? 'folderOpen' : 'open')));
}

// ───────────────────────── search ─────────────────────────

export function renderSearch(ctx, el) {
  const { S } = ctx;
  const q = h('input.inp.search-inp', { value: S.searchQ || '', placeholder: t('Keresés a skill teljes szövegében… (ékezet nélkül is)'), spellcheck: 'false' });
  const out = h('div.hits');
  const run = async () => {
    const query = q.value.trim();
    S.searchQ = query;
    if (!query) { out.replaceChildren(h('div.empty-small', t('Írj be egy szót vagy kifejezést. A szóelejekre is talál (pl. „megszak” → megszakítás).'))); return; }
    try {
      const hits = await invoke('search', { skillId: S.skillId, query, limit: 100 });
      if (!hits.length) { out.replaceChildren(h('div.empty-small', t('Nincs találat.'))); return; }
      out.replaceChildren(h('div.hit-count', t('{0} találat', hits.length)), ...hits.map((x) => h('div.hit', {
        on: { click: () => { S.textDoc = x.doc_id; S.textPage = x.page; S.textFind = (x.snippet.match(/\u0001([^\u0002]*)\u0002/) || [])[1] || ''; ctx.setView('text'); } },
      }, h('div.hit-name', icon('note'), h('span', x.doc), h('span.hit-cnt', t('{0}. oldal', x.page))),
      h('div.hit-snip', ...snippet(x.snippet)))));
    } catch (e) { out.replaceChildren(h('div.empty-small', String(e))); }
  };
  const later = debounce(run, 250);
  q.addEventListener('input', later);
  q.addEventListener('keydown', (e) => { if (e.key === 'Enter') run(); });
  el.replaceChildren(h('div.search-page',
    h('div.search-head', icon('search'), q,
      ctx.aiOn() ? h('button.btn.ai', { title: t('Kérdés a skill dokumentumaiból: a SkillStar keres, az AI válaszol forrásmegjelöléssel'), on: { click: () => ctx.aiAsk(q.value.trim()) } }, icon('sparkle'), t('Kérdezd az AI-t')) : null),
    out));
  run();
  setTimeout(() => q.focus(), 20);
}

function snippet(s) {
  const parts = [];
  const re = /\u0001([^\u0002]*)\u0002/g;
  let last = 0, m;
  while ((m = re.exec(s))) {
    if (m.index > last) parts.push(s.slice(last, m.index));
    parts.push(h('mark', m[1]));
    last = re.lastIndex;
  }
  parts.push(s.slice(last));
  return parts;
}
