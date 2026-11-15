// SkillStar – the views of one skill: Documents, Text, Skill (metadata),
// Package (preview + build) and Search; plus the sidebar with the skills.

import { invoke, h, icon, toast, modal, confirmDlg, contextMenu, relTime, fmtDate, debounce } from './util.js';
import { t } from './i18n.js';
import { mdRender } from './markdown.js';

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
      : h('button.btn.primary', { disabled: !toProcess.length && !selDocs.length, title: t('Az új, hibás és megváltozott dokumentumok feldolgozása (kijelölésnél: a kijelöltek)'),
        on: { click: () => ctx.process(selDocs.length ? selDocs.map((d) => d.id) : toProcess.map((d) => d.id)) } },
      icon('play'), selDocs.length ? t('Kijelöltek feldolgozása ({0})', selDocs.length) : toProcess.some((d) => d.partial > 0) ? t('Folytatás ({0})', toProcess.length) : t('Feldolgozás ({0})', toProcess.length)),
    h('span.grow'),
    selDocs.length ? h('button.btn', { on: { click: () => ctx.removeDocs(selDocs) } }, icon('trash'), t('Eltávolítás ({0})', selDocs.length)) : null,
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
  h('td.c-src', textSource(d), d.ocr_mode !== 'auto' ? h('span.ocr-mode', { title: t('OCR mód') }, d.ocr_mode === 'force' ? t('mindig OCR') : t('OCR nélkül')) : null),
  h('td.c-size', fmtSize(d.size)),
  h('td.c-st', statusCell(ctx, d)),
  h('td.c-act', h('button.icon-btn', { title: t('Műveletek'), on: { click: (e) => docMenu(ctx, e, d) } }, icon('chev')))));
  const table = h('table.doc-table',
    h('thead', h('tr', h('th'), h('th', t('Dokumentum')), h('th.r', t('Oldal')), h('th', t('Szöveg')), h('th.r', t('Méret')), h('th', t('Állapot')), h('th'))),
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
    { label: many ? t('Kijelöltek feldolgozása ({0})', sel.length) : t('Újrafeldolgozás'), icon: 'play', disabled: S.queue.includes(d.id), action: () => ctx.process((many ? sel : [d]).map((x) => x.id)) },
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
    h('label.chk', inc, t('Bekerül a skillbe')),
    aiBtn ? h('div.field.full', h('div', { style: { display: 'flex', gap: '8px', alignItems: 'center' } }, aiBtn, aiOut)) : null);
  const ok = await modal({ title: t('Dokumentum – {0}', d.name), body, wide: true, buttons: [{ label: t('Mégse'), value: false }, { label: t('Mentés'), value: true, primary: true }] });
  if (!ok) return;
  const modeChanged = mode.value !== d.ocr_mode;
  await ctx.saveDoc(d, { title: title.value, summary: summary.value, use_when: useWhen.value, ocr_mode: mode.value, include: inc.checked });
  if (modeChanged && d.status === 'done' && await confirmDlg(t('OCR mód'), t('Feldolgozod újra a dokumentumot az új OCR móddal?'), t('Feldolgozás'))) ctx.process([d.id]);
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
      h('button.icon-btn', { title: t('Eredeti fájl megnyitása'), on: { click: () => invoke('open_file', { path: data.doc.path }).catch(err) } }, icon('open'))),
    h('div.tx-body', outline, ta));
  main.onkeydown = (e) => {
    if (e.target === pageInp) return;
    if (e.key === 'PageDown' && e.ctrlKey) { e.preventDefault(); go(S.textPage + 1); }
    if (e.key === 'PageUp' && e.ctrlKey) { e.preventDefault(); go(S.textPage - 1); }
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
  const save = async () => {
    const next = { ...sk };
    for (const k of Object.keys(f)) next[k] = f[k].value;
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
    h('div.pk-paths',
      pathRow(t('Plugin mappa (Claude Code)'), res.dir, true),
      pathRow(t('Plugin csomag (.plugin)'), res.plugin),
      pathRow(t('Skill ZIP (claude.ai / Claude Desktop)'), res.skill_zip)),
    res.warnings.length ? h('div.pk-warn', res.warnings.map((w) => h('div', icon('warnTri'), w))) : null) : null;
  el.replaceChildren(h('div.pk',
    h('div.pk-head',
      h('div', h('h2', icon('pkg'), t('Skill csomag')),
        h('div.sub', sk?.built ? t('Utoljára építve: {0}', fmtDate(sk.built, true)) : t('Még nem építetted meg.'))),
      h('span.grow'),
      h('button.btn', { on: { click: () => ctx.revealOut(S.skillId) } }, icon('folderOpen'), t('Kimeneti mappa')),
      h('button.btn.primary', { disabled: !sk?.done || null, on: { click: () => ctx.build() } }, icon('build'), t('Csomag építése'))),
    result,
    h('div.pk-body', fileList, viewer)));
  invoke('preview_skill', { id: S.skillId }).then((files) => {
    let selPath = S.pkSel && files.some((f) => f.path === S.pkSel) ? S.pkSel : (files.find((f) => f.path.endsWith('SKILL.md')) || files[0])?.path;
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
    const tree = files.map((f) => {
      const depth = f.path.split('/').length - 1;
      return h('div.pk-file', { 'data-p': f.path, style: { paddingLeft: (8 + depth * 12) + 'px' }, title: f.path, on: { click: () => { selPath = f.path; show(); } } },
        icon(f.path.endsWith('.json') ? 'file' : f.path.endsWith('SKILL.md') ? 'skill' : 'note'), h('span', f.path.split('/').pop()), h('span.cnt', fmtSize(f.size)));
    });
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
