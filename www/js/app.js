// SkillStar – main UI shell: toolbar, sidebar, views, welcome page, settings,
// AI panel, about, self-update. The views of a skill: skills.js.

import {
  invoke, h, icon, toast, modal, confirmDlg, debounce,
  initUiState, uiGet, uiSet, editMenu, splitter,
} from './util.js';
import { t, setLang, getLang } from './i18n.js';
import { mdRender } from './markdown.js';
import {
  renderSidebar, renderViewTabs, renderDocs, renderText, renderSkillForm, renderBuild, renderSearch,
  editDocDlg, forgetText, DOC_FILTER, err, engineIndicator,
} from './skills.js';

// ───────────────────────── state ─────────────────────────

const S = {
  settings: null,
  skills: [],
  skillId: 0,
  view: 'docs',
  docs: [],
  docSel: new Set(),
  progress: {},     // doc id → {stage, done, total}
  queue: [],        // doc ids waiting or processing
  engine: null,     // engine_status
  textDoc: 0,
  textPage: 1,
  textFind: '',
  searchQ: '',
  pkSel: '',
  pkRaw: false,
  buildResult: null,
  defaultOut: '',
  busy: 0,
  busyLabel: '',
  donationVisible: false,
  update: null,
};

const curSkill = () => S.skills.find((s) => s.id === S.skillId) || null;
const aiOn = () => !!S.settings?.ai_enabled;
const isDeepSeek = () => S.settings?.ai_provider === 'deepseek';
const isClaudeCli = () => S.settings?.ai_provider === 'claude-cli';
const aiKey = () => (isClaudeCli() ? 'cli' : (isDeepSeek() ? S.settings?.deepseek_api_key : S.settings?.anthropic_api_key) || '');
const aiProviderName = () => (isDeepSeek() ? 'DeepSeek' : isClaudeCli() ? 'Claude Code CLI' : 'Anthropic Claude');
const aiModel = () => (isDeepSeek() ? S.settings?.deepseek_model : S.settings?.ai_model) || '';

async function saveSettings() {
  try { await invoke('save_settings', { settings: S.settings }); } catch (e) { toast(String(e), 'err'); }
}
const saveLater = debounce(() => saveSettings(), 400);

// ───────────────────────── services for the views ─────────────────────────

const ctx = {
  S,
  aiOn: () => aiOn(),
  selectSkill: (id) => selectSkill(id),
  setView: (v) => setView(v),
  newSkill: () => newSkill(),
  deleteSkill: (s) => deleteSkill(s),
  refreshSkills: () => refreshSkills(),
  reloadDocs: () => reloadDocs(),
  renderContent: () => renderContent(),
  addFiles: () => addFiles(),
  addFolder: () => addFolder(),
  process: (ids) => processDocs(ids),
  cancel: () => invoke('cancel_processing').catch(err),
  removeDocs: (docs) => removeDocs(docs),
  saveDoc: (d, patch, quiet) => saveDoc(d, patch, quiet),
  editDoc: (d) => editDocDlg(ctx, d),
  aiDescribe: (d) => aiDescribe(d),
  moveDoc: (d, dir) => moveDoc(d, dir),
  openText: (id) => { S.textDoc = id; S.textPage = 1; setView('text'); },
  build: () => build(),
  revealOut: (id) => revealOut(id),
  outRootLabel: () => S.settings.output_dir || S.defaultOut,
  settingsDlg: (tab) => settingsDlg(tab),
  downloadPdfium: () => downloadPdfium(),
  aiAsk: (q) => aiAsk(q),
};

// ───────────────────────── theme / zoom / language ─────────────────────────

function applyLang() {
  setLang(S.settings.ui_language || 'auto');
  invoke('set_ui_language', { lang: getLang() }).catch(() => {});
}

function applyTheme() {
  let th = S.settings.theme;
  if (th === 'system') th = matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark';
  document.documentElement.dataset.theme = th;
  document.documentElement.style.setProperty('--font-size', (S.settings.font_size || 13) + 'px');
}

const ZOOMS = [0.5, 0.67, 0.75, 0.8, 0.9, 1, 1.1, 1.25, 1.5, 1.75, 2, 2.5, 3];
const zoomNow = () => S.settings?.zoom || 1;
function applyZoom() { invoke('set_zoom', { scale: zoomNow() }).catch(() => {}); }
function setZoom(z) {
  z = Math.max(ZOOMS[0], Math.min(ZOOMS[ZOOMS.length - 1], Math.round(z * 100) / 100));
  if (z === zoomNow()) return;
  S.settings.zoom = z;
  applyZoom();
  saveLater();
  toast(t('Nagyítás: {0}%', Math.round(z * 100)), 'info', 1200);
  renderStatusbar();
}
function zoomStep(dir) {
  const z = zoomNow();
  if (dir === 0) return setZoom(1);
  const next = dir > 0 ? ZOOMS.find((x) => x > z + 0.001) : [...ZOOMS].reverse().find((x) => x < z - 0.001);
  if (next) setZoom(next);
}

const THEME_UI = { dark: ['Sötét', 'moon'], light: ['Világos', 'sun'] };
const LIGHT_THEMES = ['light', 'modern'];
const nextBasicTheme = () => (LIGHT_THEMES.includes(document.documentElement.dataset.theme) ? 'dark' : 'light');
function toggleTheme() {
  S.settings.theme = nextBasicTheme();
  applyTheme();
  saveSettings();
  renderAll();
}

let busyTimer = 0;
function setBusy(on, label = '') {
  S.busy = Math.max(0, S.busy + (on ? 1 : -1));
  S.busyLabel = on ? label : S.busy ? S.busyLabel : '';
  clearTimeout(busyTimer);
  const bar = document.getElementById('busybar');
  if (S.busy || S.queue.length) busyTimer = setTimeout(() => bar.classList.add('on'), 150);
  else bar.classList.remove('on');
  renderToolbar();
  renderStatusbar();
}

// ───────────────────────── skills ─────────────────────────

async function refreshSkills() {
  try { S.skills = await invoke('list_skills'); } catch (e) { err(e); }
  if (S.skillId && !curSkill()) S.skillId = 0;
  renderSidebar(ctx);
  renderToolbar();
}

async function reloadDocs() {
  if (!S.skillId) { S.docs = []; return; }
  try { S.docs = await invoke('list_documents', { skillId: S.skillId }); } catch (e) { err(e); S.docs = []; }
  const ids = new Set(S.docs.map((d) => d.id));
  for (const id of [...S.docSel]) if (!ids.has(id)) S.docSel.delete(id);
  renderViewTabs(ctx);
  if (S.view === 'docs') renderContent();
}

async function selectSkill(id) {
  if (S.skillId === id) return;
  S.skillId = id;
  S.docSel.clear();
  S.textDoc = 0;
  S.searchQ = '';
  S.pkSel = '';
  S.settings.active_skill = id;
  saveLater();
  await reloadDocs();
  renderAll();
}

function setView(v) {
  S.view = v;
  uiSet('ss.view', v);
  renderViewTabs(ctx);
  renderContent();
  renderToolbar();
}

async function newSkill() {
  const name = h('input.inp', { placeholder: t('pl. Garanciális szabályzat, C64 könyvek, Termékkézikönyv') });
  const lang = h('select.inp', [['hu', 'Magyar'], ['en', 'English']].map(([v, l]) => h('option', { value: v }, l)));
  lang.value = getLang();
  const res = await modal({
    title: t('Új skill'),
    body: h('div.settings-grid',
      h('label.field.full', h('span', t('Név')), name),
      h('label.field', h('span', t('A generált szövegek nyelve')), lang),
      h('div.note.full', t('A skill egy tudáscsomag: a dokumentumaid teljes szövege, tartalomjegyzéke és kulcsszó-mutatója, plusz egy SKILL.md, amiből a Claude tudja, mikor és hogyan használja.'))),
    buttons: [{ label: t('Mégse'), value: null }, { label: t('Létrehozás'), value: () => ({ title: name.value.trim(), language: lang.value }), primary: true }],
  });
  if (!res?.title) return null;
  try {
    const id = await invoke('create_skill', { title: res.title, language: res.language });
    await refreshSkills();
    S.view = 'docs';
    await selectSkill(id);
    return id;
  } catch (e) { err(e); return null; }
}

async function deleteSkill(s) {
  if (!(await confirmDlg(t('Skill törlése'), t('Törlöd a(z) „{0}” skillt? A dokumentumok kinyert szövege is törlődik az adatbázisból (az eredeti fájlok és a már megépített csomagok megmaradnak).', s.title), t('Törlés'), true))) return;
  try {
    await invoke('delete_skill', { id: s.id });
    if (S.skillId === s.id) { S.skillId = 0; S.docs = []; }
    await refreshSkills();
    renderAll();
  } catch (e) { err(e); }
}

async function revealOut(id) {
  try {
    const p = await invoke('skill_out_dir', { id });
    await invoke('reveal_in_explorer', { path: p }).catch(() => invoke('reveal_in_explorer', { path: p.replace(/[\\/][^\\/]+$/, '') }));
  } catch (e) { err(e); }
}

// ───────────────────────── documents ─────────────────────────

async function addPaths(paths) {
  if (!paths?.length) return;
  if (!S.skillId) {
    const id = await newSkill();
    if (!id) return;
  }
  try {
    const r = await invoke('add_documents', { skillId: S.skillId, paths });
    if (r.skipped.length) toast(t('Nem támogatott fájlok kimaradtak: {0}', r.skipped.join(', ')), 'err');
    if (!r.added.length && !r.skipped.length) toast(t('Nem került be új dokumentum (már a skillben vannak, vagy nincs támogatott fájl).'));
    S.view = 'docs';
    await reloadDocs();
    await refreshSkills();
    renderAll();
    if (r.added.length) {
      toast(t('{0} dokumentum hozzáadva.', r.added.length), 'ok', 2000);
      if (S.settings.auto_process !== false) processDocs(r.added);
    }
  } catch (e) { err(e); }
}

async function addFiles() {
  const r = await invoke('plugin:dialog|open', { options: { multiple: true, title: t('Dokumentumok hozzáadása'), filters: [{ name: t('Dokumentumok'), extensions: DOC_FILTER }, { name: t('Minden fájl'), extensions: ['*'] }] } }).catch(() => null);
  if (r) addPaths(Array.isArray(r) ? r : [r]);
}

async function addFolder() {
  const r = await invoke('plugin:dialog|open', { options: { directory: true, multiple: false, title: t('Mappa hozzáadása (almappákkal)') } }).catch(() => null);
  if (r) addPaths(Array.isArray(r) ? r : [r]);
}

async function processDocs(ids) {
  if (!ids?.length) return;
  try {
    await invoke('process_documents', { ids });
    S.queue = await invoke('processing_queue');
  } catch (e) {
    if (String(e) === '\u0001pdfium') {
      const ok = await confirmDlg(t('pdfium szükséges'),
        t('A PDF-ek feldolgozásához a pdfium könyvtár kell (a Chromium PDF-motorja, kb. 6 MB). Letöltöm most a github.com/bblanchon/pdfium-binaries oldalról a SkillStar adatmappájába?'),
        t('Letöltés'));
      if (ok && (await downloadPdfium())) return processDocs(ids);
      return;
    }
    err(e);
  }
  await reloadDocs();
  setBusy(false);
}

async function removeDocs(docs) {
  const text = docs.length === 1 ? t('Eltávolítod a(z) „{0}” dokumentumot a skillből? (Az eredeti fájl megmarad.)', docs[0].name)
    : t('Eltávolítasz {0} dokumentumot a skillből? (Az eredeti fájlok megmaradnak.)', docs.length);
  if (!(await confirmDlg(t('Eltávolítás'), text, t('Eltávolítás'), true))) return;
  try {
    await invoke('remove_documents', { ids: docs.map((d) => d.id) });
    for (const d of docs) { S.docSel.delete(d.id); forgetText(d.id); }
    await reloadDocs();
    await refreshSkills();
  } catch (e) { err(e); }
}

async function saveDoc(d, patch, quiet = false) {
  const n = { title: d.title, summary: d.summary, use_when: d.use_when, ocr_mode: d.ocr_mode, ocr_lang: d.ocr_lang || '', include: d.include, ...patch };
  try {
    const saved = await invoke('update_document', { id: d.id, title: n.title, summary: n.summary, useWhen: n.use_when, ocrMode: n.ocr_mode, ocrLang: n.ocr_lang, include: n.include });
    Object.assign(d, saved);
    if (!quiet) { renderContent(); renderViewTabs(ctx); }
  } catch (e) { err(e); }
}

async function aiDescribe(d) {
  setBusy(true, t('AI: {0}', d.name));
  try {
    const r = await invoke('ai_describe', { id: d.id });
    await saveDoc(d, { title: d.title || r.title, summary: r.summary, use_when: r.use_when });
    toast(t('Leírás kész: {0}', d.name), 'ok', 2500);
  } catch (e) { err(e); }
  finally { setBusy(false); }
}

async function moveDoc(d, dir) {
  const i = S.docs.indexOf(d);
  const j = i + dir;
  if (i < 0 || j < 0 || j >= S.docs.length) return;
  [S.docs[i], S.docs[j]] = [S.docs[j], S.docs[i]];
  renderContent();
  await invoke('reorder_documents', { ids: S.docs.map((x) => x.id) }).catch(err);
}

/** Progress events of the background queue. */
const docRefresh = debounce(async () => { await reloadDocs(); await refreshSkills(); }, 250);
function onProgress(p) {
  const d = S.docs.find((x) => x.id === p.id);
  if (p.status === 'processing') S.progress[p.id] = { stage: p.stage, done: p.done, total: p.total };
  else delete S.progress[p.id];
  if (!S.queue.includes(p.id) && (p.status === 'queued' || p.status === 'processing')) S.queue.push(p.id);
  if (p.status === 'done' || p.status === 'error' || p.status === 'new') {
    S.queue = S.queue.filter((x) => x !== p.id);
    forgetText(p.id);
    if (p.status === 'error' && d) toast(`${d.name}: ${p.error}`, 'err');
  }
  if (d && d.status !== p.status) { d.status = p.status; d.error = p.error; }
  if (p.skill_id === S.skillId) {
    if (S.view === 'docs') renderContent();
    if (p.status !== 'processing') docRefresh();
  }
  const bar = document.getElementById('busybar');
  bar.classList.toggle('on', S.queue.length > 0 || S.busy > 0);
  renderStatusbar();
  renderToolbar();
}

// ───────────────────────── build ─────────────────────────

async function build() {
  const sk = curSkill();
  if (!sk) return;
  if (S.queue.length) { toast(t('Várd meg, amíg a feldolgozás befejeződik.'), 'err'); return; }
  setBusy(true, t('Csomag építése…'));
  try {
    const res = await invoke('build_skill', { id: sk.id });
    S.buildResult = { skill: sk.id, res };
    await refreshSkills();
    S.view = 'build';
    renderAll();
    toast(t('A skill csomag elkészült: {0}', res.dir), 'ok', 4000);
  } catch (e) { err(e); }
  finally { setBusy(false); }
}

// ───────────────────────── engines ─────────────────────────

async function loadEngine() {
  try { S.engine = await invoke('engine_status'); } catch { S.engine = null; }
  renderSidebar(ctx);
  renderStatusbar();
  if (!S.skillId) renderContent();
}

async function downloadPdfium() {
  setBusy(true, t('pdfium letöltése…'));
  let un = null;
  try {
    un = await window.__TAURI__.event.listen('pdfium-progress', ({ payload: { received, total } }) => {
      S.busyLabel = total ? t('pdfium letöltése… {0}%', Math.round((received * 100) / total)) : t('pdfium letöltése… {0} MB', (received / 1048576).toFixed(1));
      renderStatusbar();
    });
    const p = await invoke('pdfium_download');
    toast(t('pdfium telepítve: {0}', p), 'ok', 3500);
    await loadEngine();
    return true;
  } catch (e) { err(e); return false; }
  finally { un?.(); setBusy(false); }
}

// ───────────────────────── rendering ─────────────────────────

function renderAll() {
  renderSidebar(ctx);
  renderViewTabs(ctx);
  renderToolbar();
  renderContent();
  renderStatusbar();
}

function tb(label, ic, action, { disabled, title, cls = '', badge } = {}) {
  return h('button.tb' + cls, { disabled: disabled || null, title: title || label, on: { click: action } }, icon(ic), h('span', label), badge ? h('span.badge', badge) : null);
}

function renderToolbar() {
  const el = document.getElementById('toolbar');
  const sk = curSkill();
  const nextTheme = nextBasicTheme();
  const running = S.queue.length > 0;
  const toProcess = S.docs.filter((d) => d.status === 'new' || d.status === 'error' || d.stale);
  el.replaceChildren(
    h('div.repo-title', { title: sk ? `${sk.title}
${sk.name}` : '' }, h('span.rn', sk ? sk.title : 'SkillStar'),
      h('span.rb', icon(sk ? 'skill' : 'star'), h('span.rbt', sk ? sk.name : t('Dokumentumokból skill')))),
    h('div.tb-sep'),
    tb(t('Új skill'), 'newSkill', () => newSkill(), { title: t('Új skill (Ctrl+N)') }),
    tb(t('Fájlok'), 'addFiles', () => addFiles(), { title: t('Dokumentumok hozzáadása (Ctrl+O)') }),
    running ? tb(t('Leállítás'), 'stop', () => invoke('cancel_processing').catch(err), { title: t('A feldolgozás leállítása'), badge: String(S.queue.length) })
      : tb(t('Feldolgozás'), 'play', () => processDocs(toProcess.map((d) => d.id)), { disabled: !sk || !toProcess.length, title: t('Szövegkinyerés és OCR az új / hibás / megváltozott dokumentumokon (F5)'), badge: toProcess.length ? String(toProcess.length) : null }),
    tb(t('Építés'), 'build', () => build(), { disabled: !sk || !sk.done || running, title: t('A skill csomag megépítése (Ctrl+B)') }),
    h('div.tb-sep'),
    tb(t('Keresés'), 'search', () => setView('search'), { disabled: !sk, title: t('Keresés a skill szövegében (Ctrl+F)') }),
    aiOn() ? tb('AI', 'sparkle', () => aiAsk(''), { disabled: !sk || !sk.done, title: t('Kérdezz a skill dokumentumaiból (Ctrl+Shift+A)') }) : null,
    h('div.tb-spacer'),
    tb(t(THEME_UI[nextTheme][0]), THEME_UI[nextTheme][1], toggleTheme, { title: t('Váltás sötét és világos téma között (a többi téma a Beállításokban)') }),
    ...(S.donationVisible ? [h('span.tb-donate', tb(t('Adomány'), 'heart', () => openDonate(), { title: t('A SkillStar ingyenes. Ha hasznos neked, egy kis adománnyal támogathatod a fejlesztést.') }))] : []),
    tb(t('Beállítások'), 'settings', () => settingsDlg()),
    tb(t('Névjegy'), 'info', () => aboutDlg(), { title: t('A SkillStar névjegye (F1)') }),
  );
}

function renderStatusbar() {
  const el = document.getElementById('statusbar');
  const sk = curSkill();
  let work = null;
  if (S.queue.length) {
    const id = S.queue.find((x) => S.progress[x]) ?? S.queue[0];
    const d = S.docs.find((x) => x.id === id);
    const p = S.progress[id];
    const what = p?.total ? (p.stage === 'ocr' ? t('OCR {0}/{1}', p.done + 1, p.total) : p.stage === 'resume' ? t('Folytatás {0}/{1}', p.done + 1, p.total) : t('{0}/{1} oldal', p.done, p.total)) : '';
    work = h('span.work-st', icon('refresh', 'spin'), t('Feldolgozás: {0}', d ? d.name : '…'), what ? ` – ${what}` : '', S.queue.length > 1 ? t(' (még {0} a sorban)', S.queue.length - 1) : '');
  } else if (S.busy) work = h('span.work-st', icon('refresh', 'spin'), S.busyLabel || t('Folyamatban…'));
  el.replaceChildren(
    work || h('a.notes-path', { title: t('Kimeneti mappa megnyitása'), on: { click: () => (sk ? revealOut(sk.id) : invoke('reveal_in_explorer', { path: ctx.outRootLabel() }).catch(() => {})) } }, icon('folder'), ctx.outRootLabel()),
    h('span.grow'),
    sk ? h('span', t('{0} dokumentum · {1} oldal', sk.docs, sk.pages)) : null,
    engineIndicator(ctx),
    zoomNow() !== 1 ? h('span.zoom-status', { title: t('Nagyítás visszaállítása 100%-ra (Ctrl+0)'), on: { click: () => setZoom(1) } }, Math.round(zoomNow() * 100) + '%') : null,
    aiOn() ? h('span', t('AI: {0}', aiKey() ? aiModel() : t('nincs beállítva'))) : null,
    S.update?.updateAvailable ? h('span.update-status', { title: t('Kattints a telepítéshez'), on: { click: () => updateShowAvailable() } }, icon('fetch'), t('Frissítés elérhető: v{0}', S.update.latestVersion)) : null,
  );
}

function renderContent() {
  const el = document.getElementById('content');
  if (!S.skillId) { renderWelcome(el); return; }
  const view = { docs: renderDocs, text: renderText, skill: renderSkillForm, build: renderBuild, search: renderSearch }[S.view] || renderDocs;
  view(ctx, el);
}

function renderWelcome(el) {
  const card = (ic, title, desc, key, action) => h('button.wl-card', { on: { click: action } }, icon(ic), h('b', title), h('span', desc), key ? h('kbd', key) : null);
  const st = S.engine;
  const step = (n, title, text) => h('div.wl-step', h('span.n', String(n)), h('div', h('b', title), h('span', text)));
  el.replaceChildren(h('div.welcome', h('div.welcome-box',
    h('h1', h('img', { src: 'img/icon.svg', alt: '' }), 'SkillStar'),
    h('div.lead', t('DOCX-ekből, PDF-ekből és más dokumentumokból Claude skill csomagot készít – helyi OCR-rel és helyi adatbázissal, a fájljaid nem hagyják el a gépet.')),
    h('div.wl-steps',
      step(1, t('Dokumentumok'), t('Húzd be a fájlokat vagy egy egész mappát: PDF, DOCX, PPTX, ODT, HTML, Markdown, TXT, képek.')),
      step(2, t('Feldolgozás'), t('Szövegkinyerés, a szkennelt oldalakon OCR; tartalomjegyzék és kulcsszó-mutató a helyi adatbázisba.')),
      step(3, t('Skill csomag'), t('SKILL.md + tudásbázis oldalszámokkal: Claude Code plugin mappa, .plugin fájl és claude.ai-ra feltölthető ZIP.'))),
    h('div.wl-cards',
      card('newSkill', t('Új skill'), t('Név, nyelv – és már húzhatod is bele a fájlokat'), 'Ctrl+N', () => newSkill()),
      card('addFiles', t('Fájlok hozzáadása'), t('Új skill a kiválasztott fájlokból'), 'Ctrl+O', () => addFiles()),
      st && !st.pdfium ? card('fetch', t('pdfium letöltése'), t('A PDF-ekhez szükséges (kb. 6 MB, egyszer)'), null, () => downloadPdfium())
        : card('scan', t('OCR és szövegkinyerés'), st?.ocr?.active === 'windows' ? t('Windows beépített OCR: {0}', (st.ocr.windows_langs || []).map((l) => l.tag).join(', ')) : st?.ocr?.active === 'tesseract' ? 'Tesseract' : t('Nincs OCR motor – nézd meg a beállításokat'), null, () => settingsDlg('extract')),
      aiOn() ? card('sparkle', t('AI segítség'), t('Dokumentum-összefoglalók, skill-leírás, kérdések a tudásbázisból'), null, () => settingsDlg('ai'))
        : card('settings', t('Beállítások'), t('Kimeneti mappa, szerző, OCR nyelv, AI'), 'Ctrl+,', () => settingsDlg())),
    S.skills.length ? h('div.wl-h', t('Skillek')) : null,
    S.skills.length ? h('div.recent', S.skills.slice(0, 8).map((s) => h('div.recent-item', { on: { click: () => selectSkill(s.id) } },
      icon('skill'), h('div.ri', h('div', s.title), h('div.p', t('{0} dokumentum · {1} oldal', s.docs, s.pages) + (s.built ? ' · ' + t('építve') : '')))))) : null,
  )));
}

// ───────────────────────── AI panel ─────────────────────────

function aiPanel(title, run, presets = [], extra = null) {
  document.querySelector('.ai-panel')?.remove();
  if (!aiOn()) { toast(t('Az AI funkciók ki vannak kapcsolva (Beállítások → AI).'), 'err'); return null; }
  if (!aiKey()) {
    toast(t('Az AI funkciókhoz add meg az API kulcsot (vagy válaszd a Claude Code CLI-t) a Beállításokban.'), 'err');
    settingsDlg('ai');
    return null;
  }
  const history = [];
  const body = h('div.ai-body');
  const input = h('textarea.inp.ai-ask', { rows: 2, placeholder: t('Kérdezz a dokumentumokból… (Enter: küldés, Shift+Enter: új sor)') });
  const send = h('button.btn.sm.ai', { on: { click: () => ask(input.value.trim()) } }, icon('sparkle'), t('Küldés'));
  let busy = false;
  const ask = async (q) => {
    if (busy || !q) return;
    busy = true;
    send.disabled = true;
    body.append(h('div.ai-q', q));
    input.value = '';
    const wait = h('div.ai-thinking', icon('sparkle', 'spin'), t('Keresés és válasz…'));
    body.append(wait);
    wait.scrollIntoView({ block: 'end' });
    try {
      const res = await run(q, history.slice());
      wait.remove();
      history.push({ q, a: res.text });
      const out = h('div.ai-ans', { html: mdRender(res.text) });
      if (res.sources) citeLinks(out, res.sources);
      out.addEventListener('click', (e) => {
        const c = e.target.closest('a.cite');
        if (c) { e.preventDefault(); openSource(res.sources[Number(c.dataset.n) - 1]); return; }
        const a = e.target.closest('a[href]');
        if (!a) return;
        e.preventDefault();
        if (/^(https?|mailto):/i.test(a.getAttribute('href'))) invoke('open_url', { url: a.getAttribute('href') }).catch(err);
      });
      if (res.queries?.length > 1) body.append(h('div.rag-q', icon('search'), t('Keresések:'), ...res.queries.map((x) => h('span', x))));
      body.append(out);
      if (res.sources?.length) body.append(sourceList(res));
      out.scrollIntoView({ block: 'start' });
    } catch (e) {
      wait.replaceWith(h('div', { style: { color: 'var(--danger)' } }, String(e)));
    } finally { busy = false; send.disabled = false; input.focus(); }
  };
  input.addEventListener('keydown', (e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); ask(input.value.trim()); } });
  const panel = h('div.ai-panel',
    h('div.pane-h', h('span.spark', icon('sparkle')), title, h('span.sub', ` · ${aiModel() || aiProviderName()}`), h('span.grow'),
      h('button.icon-btn', { title: t('Szöveg másolása'), on: { click: () => navigator.clipboard.writeText(body.innerText).then(() => toast(t('Vágólapra másolva'), 'ok', 1500)) } }, icon('copy')),
      h('button.icon-btn', { title: t('Bezárás (Esc)'), on: { click: () => panel.remove() } }, icon('close'))),
    body,
    h('div.ai-foot', extra,
      presets.length ? h('div.ai-presets', presets.map((p) => h('button.btn.sm', { on: { click: () => ask(p) } }, p))) : null,
      h('div.ai-askrow', input, send)));
  document.body.append(panel);
  return { ask, input };
}

// ───────────────────────── RAG answers: [n] citations → sources ─────────────────────────

/** [1], [2][4], [1, 3] in the answer → clickable citation marks (outside code). */
function citeLinks(root, sources) {
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT, { acceptNode: (n) => (n.parentElement.closest('code, pre, a') ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_ACCEPT) });
  const nodes = [];
  while (walker.nextNode()) if (/\[\d+(?:\s*[,;]\s*\d+)*\]/.test(walker.currentNode.nodeValue)) nodes.push(walker.currentNode);
  for (const n of nodes) {
    const frag = document.createDocumentFragment();
    let last = 0;
    const txt = n.nodeValue;
    txt.replace(/\[(\d+(?:\s*[,;]\s*\d+)*)\]/g, (m, list, at) => {
      frag.append(txt.slice(last, at));
      for (const num of list.split(/[,;]/).map((x) => Number(x.trim()))) {
        const s = sources[num - 1];
        frag.append(s ? h('a.cite', { href: '#', 'data-n': String(num), title: `${s.doc} – ${t('{0}. oldal', s.page)}` }, String(num)) : `[${num}]`);
      }
      last = at + m.length;
      return m;
    });
    frag.append(txt.slice(last));
    n.replaceWith(frag);
  }
}

function sourceList(res) {
  const cited = new Set([...res.text.matchAll(/\[(\d+(?:\s*[,;]\s*\d+)*)\]/g)].flatMap((m) => m[1].split(/[,;]/).map((x) => Number(x.trim()))));
  const used = res.sources.filter((s) => cited.has(s.n));
  const rest = res.sources.filter((s) => !cited.has(s.n));
  const row = (s) => h('div.src', { title: s.text.slice(0, 600), on: { click: () => openSource(s) } },
    h('span.n', String(s.n)), h('span.d', s.doc), h('span.p', t('{0}. oldal', s.page)), h('span.x', s.text.replace(/\s+/g, ' ').slice(0, 160)));
  const more = rest.length ? h('details.src-more', h('summary', t('További {0} keresett részlet', rest.length)), rest.map(row)) : null;
  return h('div.srcs', h('div.srcs-h', icon('pages'), t('Források')), used.length ? used.map(row) : null, used.length ? more : rest.map(row));
}

/** Opens a cited page in the text view (switching skill if the source is in another one). */
async function openSource(s) {
  if (!s) return;
  if (s.skill_id && s.skill_id !== S.skillId) await selectSkill(s.skill_id);
  S.textDoc = s.doc_id;
  S.textPage = s.page;
  S.textFind = '';
  setView('text');
}

function aiAsk(q) {
  const sk = curSkill();
  if (!sk) return;
  const opt = { all: !!uiGet('rag.all'), expand: uiGet('rag.expand') !== false };
  const sw = (key, label, title) => {
    const e = h('input', { type: 'checkbox', checked: opt[key] });
    e.addEventListener('change', () => { opt[key] = e.checked; uiSet('rag.' + key, e.checked); });
    return h('label.chk.rag-opt', { title }, e, label);
  };
  const extra = h('div.rag-opts',
    h('span.rag-badge', 'RAG'),
    sw('expand', t('Keresés bővítése AI-val'), t('Az AI előbb keresőkifejezéseket ír (szinonimák, ragozott és idegen nyelvű alakok), így több releváns oldalt talál; +1 rövid AI-hívás')),
    S.skills.length > 1 ? sw('all', t('Minden skillben'), t('A keresés az összes skill dokumentumaiban')) : null);
  const p = aiPanel(t('AI – {0}', sk.title), (question, history) => invoke('ai_ask', { id: sk.id, question, history, all: opt.all, expand: opt.expand }),
    [t('Miről szólnak ezek a dokumentumok?'), t('Milyen kérdésekre tud válaszolni ez a skill?')], extra);
  if (!p) return;
  if (q) p.ask(q); else p.input.focus();
}

// ───────────────────────── about / donation ─────────────────────────

const DONATE_URL = 'https://www.c64va.pro/register/donate.php';
const SITE_URL = 'https://www.c64va.pro/skillstar.html';
function startDonationTimer() {
  let remaining = 10 * 60 * 1000;
  let started = null;
  let timer;
  const update = () => {
    clearTimeout(timer);
    if (started !== null) remaining -= performance.now() - started;
    started = null;
    if (remaining <= 0) {
      S.donationVisible = true;
      window.removeEventListener('focus', update);
      window.removeEventListener('blur', update);
      document.removeEventListener('visibilitychange', update);
      renderToolbar();
    } else if (!document.hidden && document.hasFocus()) {
      started = performance.now();
      timer = setTimeout(update, remaining);
    }
  };
  window.addEventListener('focus', update);
  window.addEventListener('blur', update);
  document.addEventListener('visibilitychange', update);
  update();
}

function openDonate() {
  invoke('open_url', { url: `${DONATE_URL}?lang=${getLang() === 'hu' ? 'hu' : 'en'}` }).catch((e) => toast(String(e), 'err'));
}


async function aboutDlg() {
  let info = {};
  try { info = await invoke('app_info'); } catch (e) { info = {}; }
  const ai = !aiOn() ? t('kikapcsolva') : aiKey() ? `${aiProviderName()} · ${aiModel()}` : t('nincs beállítva');
  const ocr = S.engine?.ocr?.active === 'windows' ? 'Windows OCR' : S.engine?.ocr?.active === 'tesseract' ? 'Tesseract' : t('nincs');
  const rows = [
    [t('Verzió'), info.version || '?'],
    ['Tauri', info.tauri || '?'],
    [t('Rendszer'), info.os || '?'],
    ['AI', ai],
    ['OCR', ocr],
    ['pdfium', S.engine?.pdfium || t('nincs')],
    [t('Adatbázis'), info.db_path || '?'],
    [t('Kimeneti mappa'), info.out_dir || '?'],
    [t('Beállítások fájl'), info.settings_path || '?'],
  ];
  const text = 'SkillStar ' + rows.map(([k, v]) => `${k}: ${v}`).join('\n');
  const features = [
    ['pdf', 'PDF'], ['word', 'DOCX / ODT'], ['slides', 'PPTX'], ['image', t('Képek')], ['scan', t('Helyi OCR')],
    ['search', t('Teljes szöveges keresés (SQLite FTS5)')], ['pkg', t('Claude skill / plugin')], ['sparkle', t('Kapcsolható AI')], ['globe', t('Magyar / English')],
  ];
  const body = h('div.about',
    h('div.about-hero',
      h('div.about-logo', h('img', { src: 'img/icon.svg', alt: 'SkillStar' })),
      h('div.about-name', 'SkillStar'),
      h('div.about-ver', `v${info.version || '?'}`),
      h('div.about-tag', t('Dokumentumokból Claude skill – helyi OCR, helyi adatbázis.'))),
    h('div.about-feats', features.map(([ic, l]) => h('span.about-feat', icon(ic), l))),
    h('div.about-info', rows.map(([k, v]) => [h('span.k', k), h('span.v', { title: v }, v)]),
      h('span.k', t('Weboldal')), h('span.v', h('a.about-link', { title: SITE_URL, on: { click: () => invoke('open_url', { url: SITE_URL }).catch((e) => toast(String(e), 'err')) } }, SITE_URL.replace(/^https:\/\//, '')))),
    h('div.about-foot', t('Rust + Tauri 2. A GitStar testvérprogramja (GitStar, FtpStar, DiffStar, NoteStar). PDF: pdfium (BSD). OCR: Windows.Media.Ocr vagy Tesseract. AI: Anthropic Claude (API vagy Claude Code CLI) vagy DeepSeek.')),
    h('div.about-free', t('A SkillStar ingyenes (freeware). Ha hasznos neked, egy kis adománnyal támogathatod a fejlesztést. Köszönöm!')),
    h('div.about-copy', '© 2026 Zsolt Tarczali'));
  const res = await modal({ title: t('Névjegy'), body, buttons: [
    { label: t('Adomány'), value: 'donate' },
    { label: t('Frissítések keresése'), value: 'update' },
    { label: t('Beállítások mappa'), value: 'folder' },
    { label: t('Másolás'), value: 'copy' },
    { label: t('Bezárás'), value: false, primary: true },
  ] });
  if (res === 'update') checkForAppUpdate();
  if (res === 'donate') openDonate();
  if (res === 'copy') { await navigator.clipboard.writeText(text).catch(() => {}); toast(t('Vágólapra másolva')); }
  if (res === 'folder' && info.settings_path) invoke('reveal_in_explorer', { path: info.settings_path }).catch((e) => toast(String(e), 'err'));
}

// ───────────────────────── settings ─────────────────────────

async function settingsDlg(startTab) {
  const s = S.settings;
  const els = {};
  const fld = (name, label, el, note, full) => { els[name] = el; return h('label.field' + (full ? '.full' : ''), h('span', label), el, note ? h('span.note', note) : null); };
  const sel = (opts, val) => { const e = h('select.inp', opts.map(([v, l]) => h('option', { value: v }, l))); e.value = String(val); return e; };
  const chk = (name, label, val) => { const e = h('input', { type: 'checkbox', checked: !!val }); els[name] = e; return h('label.chk.full', e, label); };
  const browseBtn = (inp, opts) => h('button.btn', { type: 'button', on: { click: async () => {
    const d = await invoke('plugin:dialog|open', { options: { multiple: false, ...opts } }).catch(() => null);
    if (d) inp.value = Array.isArray(d) ? d[0] : d;
  } } }, t('Tallózás…'));

  // ── skills ──
  const outDir = h('input.inp', { value: s.output_dir || '', placeholder: S.defaultOut, spellcheck: 'false' });

  // ── extraction ──
  let st = S.engine;
  try { st = await invoke('engine_status'); S.engine = st; } catch { /* keep */ }
  const ocr = st?.ocr || { windows_langs: [], tesseract_langs: [] };
  const engineSel = sel([['auto', t('Automatikus')], ['windows', t('Windows beépített OCR')], ['tesseract', 'Tesseract'], ['off', t('Kikapcsolva')]], s.ocr_engine || 'auto');
  const langOpts = [['', t('A felhasználói profil nyelvei')], ...(ocr.windows_langs || []).map((l) => [l.tag, `${l.name} (${l.tag})`])];
  if (s.ocr_lang && !langOpts.some((o) => o[0] === s.ocr_lang)) langOpts.push([s.ocr_lang, `${s.ocr_lang} (${t('nincs telepítve')})`]);
  const winLang = sel(langOpts, s.ocr_lang || '');
  const tessPath = h('input.inp', { value: s.tesseract_path || '', placeholder: ocr.tesseract || t('üres = megkeresi (PATH, Program Files)'), spellcheck: 'false' });
  const tessLang = h('input.inp', { value: s.tesseract_lang || 'hun+eng', spellcheck: 'false' });
  const pdfiumPath = h('input.inp', { value: s.pdfium_path || '', placeholder: st?.pdfium || st?.pdfium_target || '', spellcheck: 'false' });
  const pdfiumOut = h('span.note' + (st?.pdfium ? '.ok' : '.err'), st?.pdfium ? '✓ ' + st.pdfium : '✗ ' + t('nincs pdfium – PDF-ekhez töltsd le'));
  const pdfiumDl = h('button.btn', { type: 'button', on: { click: async () => {
    pdfiumDl.disabled = true;
    if (await downloadPdfium()) { pdfiumOut.className = 'note ok'; pdfiumOut.textContent = '✓ ' + (S.engine?.pdfium || ''); }
    pdfiumDl.disabled = false;
  } } }, icon('fetch'), t('Letöltés'));
  const pdfiumTest = h('button.btn', { type: 'button', on: { click: async () => {
    const p = pdfiumPath.value.trim() || st?.pdfium || '';
    if (!p) return;
    try { pdfiumOut.textContent = '✓ ' + (await invoke('pdfium_test', { path: p })); pdfiumOut.className = 'note ok'; }
    catch (e) { pdfiumOut.textContent = '✗ ' + e; pdfiumOut.className = 'note err'; }
  } } }, t('Teszt'));
  const ocrState = h('div.note.full', ocr.windows ? t('Windows OCR nyelvek ezen a gépen: {0}.', (ocr.windows_langs || []).map((l) => l.tag).join(', ')) : t('A Windows OCR nem érhető el.'),
    ' ', ocr.tesseract ? t('Tesseract: {0} (nyelvek: {1}).', ocr.tesseract, (ocr.tesseract_langs || []).join(', ') || '?') : t('Tesseract nincs telepítve.'),
    ' ', t('Magyar Windows OCR-hez: Gépház → Idő és nyelv → Nyelv és régió → Magyar → Nyelvi beállítások → „Optikai karakterfelismerés” letöltése.'));

  // ── AI ──
  const aiEnabled = h('input', { type: 'checkbox', checked: !!s.ai_enabled });
  const models = ['claude-opus-5-5', 'claude-sonnet-5-5', 'claude-haiku-5-5', 'claude-fable-5-2'];
  const modelSel = sel([...models.map((m) => [m, m]), ['custom', t('Egyéb…')]], models.includes(s.ai_model) ? s.ai_model : 'custom');
  const customModel = h('input.inp', { value: models.includes(s.ai_model) ? '' : s.ai_model, placeholder: 'model id', style: { display: models.includes(s.ai_model) ? 'none' : '' } });
  modelSel.addEventListener('change', () => (customModel.style.display = modelSel.value === 'custom' ? '' : 'none'));
  const key = h('input.inp', { type: 'password', value: s.anthropic_api_key, placeholder: 'sk-ant-…', spellcheck: 'false' });
  const dsModels = ['deepseek-flash', 'deepseek-v4-pro'];
  const dsModel = s.deepseek_model || 'deepseek-flash';
  const dsKey = h('input.inp', { type: 'password', value: s.deepseek_api_key || '', placeholder: 'sk-…', spellcheck: 'false' });
  const dsModelSel = sel([...dsModels.map((m) => [m, m]), ['custom', t('Egyéb…')]], dsModels.includes(dsModel) ? dsModel : 'custom');
  const dsCustom = h('input.inp', { value: dsModels.includes(dsModel) ? '' : dsModel, placeholder: 'model id', style: { display: dsModels.includes(dsModel) ? 'none' : '' } });
  dsModelSel.addEventListener('change', () => (dsCustom.style.display = dsModelSel.value === 'custom' ? '' : 'none'));
  const providerSel = sel([['anthropic', 'Anthropic Claude (API key)'], ['claude-cli', t('Claude Code CLI (saját bejelentkezés)')], ['deepseek', 'DeepSeek']], s.ai_provider || 'anthropic');
  const cliPath = h('input.inp', { value: s.claude_cli_path || '', placeholder: t('üres = megkeresi (PATH, ~/.local/bin, Homebrew, npm)'), spellcheck: 'false' });
  const cliField = h('label.field.full', h('span', t('Claude CLI útvonala')), cliPath,
    h('span.note', t('A telepített Claude Code CLI-t (claude) futtatja a saját bejelentkezéseddel, így itt nem kell API kulcs. Bejelentkezés: futtasd egyszer a „claude” parancsot egy terminálban.')));
  const keyField = h('label.field.full', h('span', t('Anthropic API kulcs')), key, h('span.note', t('A kulcs a SkillStar beállításfájljában tárolódik ezen a gépen. Kulcs: console.anthropic.com')));
  const claudeBox = h('div.settings-sub.full', keyField, cliField,
    h('label.field', h('span', t('Modell')), modelSel, customModel, h('span.note', t('Alapértelmezés: claude-opus-5-5. Gyorsabb/olcsóbb: claude-haiku-5-5.'))));
  const dsBox = h('div.settings-sub.full',
    h('label.field.full', h('span', t('DeepSeek API kulcs')), dsKey, h('span.note', t('A kulcs a SkillStar beállításfájljában tárolódik ezen a gépen. Kulcs: platform.deepseek.com'))),
    h('label.field', h('span', t('Modell')), dsModelSel, dsCustom, h('span.note', t('Alapértelmezés: deepseek-flash. Erősebb: deepseek-v4-pro.'))));
  const syncProvider = () => {
    const ds = providerSel.value === 'deepseek', cli = providerSel.value === 'claude-cli';
    claudeBox.style.display = ds ? 'none' : ''; dsBox.style.display = ds ? '' : 'none';
    keyField.style.display = cli ? 'none' : ''; cliField.style.display = cli ? '' : 'none';
  };
  providerSel.addEventListener('change', syncProvider);
  syncProvider();
  const aiTestOut = h('span.note', '');
  const aiTestBtn = h('button.btn', { on: { click: async () => {
    const ds = providerSel.value === 'deepseek', cli = providerSel.value === 'claude-cli';
    const apiKey = (ds ? dsKey.value : cli ? cliPath.value : key.value).trim();
    const model = ds ? (dsModelSel.value === 'custom' ? dsCustom.value.trim() || 'deepseek-flash' : dsModelSel.value)
      : (modelSel.value === 'custom' ? customModel.value.trim() || 'claude-opus-5-5' : modelSel.value);
    if (!apiKey && !cli) { aiTestOut.className = 'note err'; aiTestOut.textContent = '✗ ' + t('Nincs megadva API kulcs.'); return; }
    aiTestBtn.disabled = true;
    aiTestOut.className = 'note';
    aiTestOut.textContent = t('Tesztelés…');
    try { aiTestOut.textContent = '✓ ' + (await invoke('ai_test', { provider: providerSel.value, apiKey, model })); aiTestOut.className = 'note ok'; }
    catch (e) { aiTestOut.textContent = '✗ ' + e; aiTestOut.className = 'note err'; }
    finally { aiTestBtn.disabled = false; }
  } } }, t('Teszt'));
  const aiBox = h('div.settings-grid.full.ai-box',
    h('label.field.full', h('span', t('AI szolgáltató')), h('div', { style: { display: 'flex', gap: '6px' } }, providerSel, aiTestBtn), aiTestOut),
    claudeBox, dsBox,
    fld('ai_language', t('AI válaszok nyelve'), sel([['hu', t('Magyar')], ['en', 'English']], s.ai_language)),
    h('div.note.full', t('Az AI csak akkor kap adatot, ha te kéred: egy dokumentum elejét és néhány oldalát (összefoglalóhoz), a dokumentumlistát (skill-leíráshoz), vagy a keresés találatait (kérdésnél). A feldolgozás, az OCR és a csomagépítés AI nélkül, helyben fut.')));
  const syncAi = () => aiBox.classList.toggle('off', !aiEnabled.checked);
  aiEnabled.addEventListener('change', syncAi);
  syncAi();

  // ── update ──
  const updCheck = h('input', { type: 'checkbox', checked: s.update_check !== false });
  const updServer = h('input.inp', { value: s.update_server || '', placeholder: 'https://www.c64va.pro/', spellcheck: 'false' });
  const saveUpdServer = async () => {
    try { await invoke('update_server_check', { url: updServer.value }); } catch (e) { toast(String(e), 'err'); return; }
    S.settings.update_server = updServer.value.trim();
    await saveSettings();
    checkForAppUpdate();
  };

  const tabs = [
    ['skills', t('Skillek'), h('div.settings-grid',
      h('label.field.full', h('span', t('Kimeneti mappa')), h('div', { style: { display: 'flex', gap: '6px' } }, outDir, browseBtn(outDir, { directory: true, title: t('Kimeneti mappa') })),
        h('span.note', t('Ide kerülnek a megépített csomagok: <skill>/ mappa (Claude Code plugin), <skill>.plugin és <skill>-skill.zip. Üres = {0}', S.defaultOut))),
      fld('author', t('Szerző (plugin.json)'), h('input.inp', { value: s.author || '', placeholder: t('a neved vagy a csapatod') })),
      h('div'),
      chk('auto_process', t('Hozzáadás után automatikus feldolgozás'), s.auto_process !== false),
      chk('restore_session', t('Induláskor a legutóbbi skill megnyitása'), s.restore_session !== false))],
    ['extract', t('Szövegkinyerés'), h('div.settings-grid',
      h('div.settings-sec', 'PDF (pdfium)'),
      h('label.field.full', h('span', t('pdfium könyvtár')), h('div', { style: { display: 'flex', gap: '6px' } }, pdfiumPath, browseBtn(pdfiumPath, { title: 'pdfium' }), pdfiumTest, pdfiumDl), pdfiumOut,
        h('span.note', t('A PDF-ek szövegrétegét és az OCR-hez az oldalképeket a pdfium (a Chromium PDF-motorja) adja. A telepítő tartalmazza; ha mégis hiányzik, egy kattintással letölthető. Üres = a beépített / letöltött példány.'))),
      fld('ocr_min_chars', t('OCR, ha egy PDF-oldalon kevesebb a karakter, mint'), h('input.inp', { type: 'number', min: 0, max: 2000, value: s.ocr_min_chars ?? 40 }), t('Szkennelt oldal: nincs vagy alig van szövegréteg.')),
      fld('ocr_dpi', t('Felbontás OCR-hez (DPI)'), sel([['150', '150'], ['200', '200'], ['220', '220'], ['300', '300'], ['400', '400']], String(s.ocr_dpi || 220)), t('Nagyobb = pontosabb, de lassabb.')),
      h('div.settings-sec', 'OCR'),
      h('label.field', h('span', t('OCR motor')), engineSel, h('span.note', t('Automatikus: Windowson a beépített OCR, máshol a Tesseract.'))),
      h('label.field', h('span', t('Windows OCR nyelv')), winLang),
      h('label.field', h('span', t('Tesseract útvonala')), h('div', { style: { display: 'flex', gap: '6px' } }, tessPath, browseBtn(tessPath, { title: 'Tesseract' }))),
      h('label.field', h('span', t('Tesseract nyelvek')), tessLang, h('span.note', t('pl. hun+eng (a nyelvi csomagoknak telepítve kell lenniük)'))),
      ocrState)],
    ['look', t('Megjelenés'), h('div.settings-grid',
      fld('ui_language', t('Felület nyelve'), sel([['auto', t('Automatikus (rendszer)')], ['hu', 'Magyar'], ['en', 'English']], s.ui_language || 'auto')),
      h('div'),
      fld('theme', t('Téma'), sel([['dark', t('Sötét')], ['light', t('Világos')], ['ai', 'Neon'], ['retro', t('Retró (8 bites)')], ['modern', t('Modern (üveg)')], ['modern-dark', t('Modern sötét (üveg)')], ['system', t('Rendszer szerint')]], s.theme)),
      fld('font_size', t('Felület betűmérete'), h('input.inp', { type: 'number', min: 10, max: 20, value: s.font_size })),
      fld('zoom', t('Nagyítás'), sel(ZOOMS.map((z) => [String(z), Math.round(z * 100) + '%']), String(ZOOMS.includes(zoomNow()) ? zoomNow() : 1)),
        t('Gyorsan: Ctrl + / Ctrl − / Ctrl 0, vagy Ctrl + egérgörgő.')))],
    ['ai', 'AI', h('div.settings-grid',
      h('label.chk.full.ai-switch', aiEnabled, h('b', t('AI funkciók bekapcsolása'))),
      h('div.note.full', t('Kikapcsolva a SkillStar semmilyen AI szolgáltatóhoz nem fordul, és az AI gombok sem látszanak. A skill csomag AI nélkül is teljes.')),
      aiBox)],
    ['update', t('Frissítés'), h('div.settings-grid',
      h('label.chk.full', updCheck, t('Új verzió keresése induláskor')),
      h('label.field.full', h('span', t('Frissítési szerver')), h('div', { style: { display: 'flex', gap: '6px' } }, updServer,
        h('button.btn', { on: { click: () => saveUpdServer() } }, t('Keresés most'))),
        h('span.note', t('Elég a szerver címe (pl. https://www.c64va.pro/), a SkillStar a downloads/SkillStar/ mappában keresi a telepítőket. Más mappához add meg a teljes címét. Üres = alapértelmezett.'))))],
  ];
  const tabBtns = tabs.map(([id, label]) => h('button.stab', { type: 'button', 'data-label': label, on: { click: () => showTab(id) } }, label));
  const panes = h('div.settings-panes', ...tabs.map(([, , el]) => el));
  const showTab = (id) => {
    if (!tabs.some((x) => x[0] === id)) id = 'skills';
    tabs.forEach(([tid, , el], i) => { el.classList.toggle('on', tid === id); tabBtns[i].classList.toggle('on', tid === id); });
    uiSet('ss.settingsTab', id);
  };
  showTab(startTab || uiGet('ss.settingsTab') || 'skills');
  const body = h('div.settings', h('div.settings-tabs', ...tabBtns), panes);
  const ok = await modal({ title: t('Beállítások'), body, wide: true, buttons: [{ label: t('Mégse'), value: false }, { label: t('Mentés'), value: true, primary: true }] });
  if (!ok) return;
  Object.assign(S.settings, {
    output_dir: outDir.value.trim(),
    author: els.author.value.trim(),
    auto_process: els.auto_process.checked,
    restore_session: els.restore_session.checked,
    pdfium_path: pdfiumPath.value.trim(),
    ocr_min_chars: Math.max(0, Math.min(2000, +els.ocr_min_chars.value || 0)),
    ocr_dpi: +els.ocr_dpi.value || 220,
    ocr_engine: engineSel.value,
    ocr_lang: winLang.value,
    tesseract_path: tessPath.value.trim(),
    tesseract_lang: tessLang.value.trim() || 'eng',
    theme: els.theme.value,
    ui_language: els.ui_language.value,
    font_size: Math.max(10, Math.min(20, +els.font_size.value || 13)),
    zoom: +els.zoom.value || 1,
    ai_enabled: aiEnabled.checked,
    ai_provider: providerSel.value,
    anthropic_api_key: key.value.trim(),
    claude_cli_path: cliPath.value.trim(),
    deepseek_api_key: dsKey.value.trim(),
    deepseek_model: dsModelSel.value === 'custom' ? dsCustom.value.trim() || 'deepseek-flash' : dsModelSel.value,
    ai_model: modelSel.value === 'custom' ? customModel.value.trim() || 'claude-opus-5-5' : modelSel.value,
    ai_language: els.ai_language.value,
    update_check: updCheck.checked,
  });
  applyZoom();
  try { await invoke('update_server_check', { url: updServer.value }); S.settings.update_server = updServer.value.trim(); }
  catch (e) { toast(String(e), 'err'); }
  if (!aiOn()) document.querySelector('.ai-panel')?.remove();
  await saveSettings();
  applyLang();
  applyTheme();
  await loadEngine();
  renderAll();
}

// ───────────────────────── self-update ─────────────────────────

let updBusy = false;

function updateDlgEls() {
  let ov = document.getElementById('update-ov');
  if (ov) return ov.els;
  const els = {
    title: h('div.modal-h'), msg: h('div.desc.update-msg'),
    bar: h('div.update-bar-fill'), text: h('div.update-progress-text'),
    skip: h('button.btn', t('Kihagyom ezt a verziót')),
    close: h('button.btn', t('Bezárás')),
    install: h('button.btn.primary', icon('fetch'), t('Telepítés most')),
  };
  els.prog = h('div.update-progress', h('div.update-bar', els.bar), els.text);
  ov = h('div.overlay#update-ov', h('div.modal.update-dlg', els.title, h('div.modal-b', els.msg, els.prog), h('div.modal-f', els.skip, els.close, els.install)));
  ov.els = els;
  const shut = () => { if (!updBusy) { ov.remove(); document.removeEventListener('keydown', onKey, true); } };
  function onKey(e) { if (e.key === 'Escape') { e.stopPropagation(); e.preventDefault(); } }
  els.close.addEventListener('click', shut);
  els.skip.addEventListener('click', () => { S.settings.update_skip = S.update?.latestVersion || ''; saveSettings(); shut(); });
  els.install.addEventListener('click', () => installUpdate());
  document.addEventListener('keydown', onKey, true);
  document.body.append(ov);
  return els;
}

function updateProgress(state, label = '') {
  const els = updateDlgEls();
  els.prog.style.display = state === null ? 'none' : '';
  els.bar.classList.toggle('indet', state === -1);
  els.bar.style.width = state >= 0 ? state + '%' : '';
  els.text.textContent = label;
}

function updateShow({ title, msg, install = false, skip = false, closeLabel = t('Bezárás') }) {
  const els = updateDlgEls();
  els.title.textContent = title;
  els.msg.textContent = msg;
  els.install.style.display = install ? '' : 'none';
  els.skip.style.display = skip ? '' : 'none';
  els.close.textContent = closeLabel;
  els.close.disabled = false;
  if (!install) updateProgress(null);
}

function updateShowAvailable() {
  const u = S.update;
  if (!u?.updateAvailable) return;
  updateShow({ title: t('Új verzió érhető el'),
    msg: t('Elérhető a SkillStar {0} (most: {1}). Letöltöm és telepítem? A SkillStar a telepítés után újraindul.', u.latestVersion, u.currentVersion),
    install: true, skip: true, closeLabel: t('Később') });
  updateProgress(null);
}

async function checkForAppUpdate({ silent = false } = {}) {
  if (updBusy) return;
  if (!silent) { updateShow({ title: t('Frissítések keresése'), msg: t('Új verzió keresése…') }); updateProgress(-1); }
  updBusy = true;
  let info;
  try { info = await invoke('check_for_update'); }
  catch (e) {
    updBusy = false;
    if (!silent) updateShow({ title: t('Frissítések keresése'), msg: t('A frissítés ellenőrzése nem sikerült: {0}', String(e)) });
    return;
  }
  updBusy = false;
  S.update = info;
  renderStatusbar();
  if (!info?.supported) { if (!silent) updateShow({ title: t('Frissítések keresése'), msg: t('Ezen a platformon nincs automatikus frissítés.') }); return; }
  if (!info.updateAvailable) { if (!silent) updateShow({ title: t('Frissítések keresése'), msg: t('A SkillStar naprakész (v{0}).', info.currentVersion) }); return; }
  if (silent && (S.settings.update_skip === info.latestVersion || document.querySelector('.overlay'))) return;
  updateShowAvailable();
}

async function installUpdate() {
  const u = S.update;
  if (updBusy || !u?.updateAvailable) return;
  const els = updateDlgEls();
  updBusy = true;
  els.install.style.display = 'none';
  els.skip.style.display = 'none';
  els.close.disabled = true;
  els.msg.textContent = t('A SkillStar {0} letöltése…', u.latestVersion);
  updateProgress(-1);
  let unlisten = null;
  try {
    const mb = (n) => (n / 1048576).toFixed(1);
    unlisten = await window.__TAURI__.event.listen('update-download-progress', ({ payload: { received, total } }) => {
      if (total > 0) updateProgress(Math.min(100, Math.round((received * 100) / total)), `${mb(received)} / ${mb(total)} MB`);
      else updateProgress(-1, `${mb(received)} MB`);
    });
    const res = await invoke('download_and_install_update', { version: u.latestVersion });
    updateProgress(100);
    if (res.restarting) els.msg.textContent = t('A telepítő elindult, a SkillStar most bezárul és újraindul.');
    else {
      updBusy = false;
      updateShow({ title: t('Új verzió érhető el'), msg: t('A telepítő megnyílt ({0}). Fejezd be a telepítést, majd indítsd újra a SkillStart.', res.path) });
    }
  } catch (e) {
    updBusy = false;
    updateShow({ title: t('Új verzió érhető el'), msg: t('A frissítés nem sikerült: {0}', String(e)), install: true });
    updateProgress(null);
  } finally { unlisten?.(); }
}


// ───────────────────────── boot ─────────────────────────

/** Files dragged onto the window (native drop: full paths). */
async function initDrop() {
  const ev = window.__TAURI__?.event;
  if (!ev) return;
  const overlay = h('div.drop-ov', h('div.drop-box', icon('addFiles'), h('b'), h('span')));
  const show = (on) => {
    if (on) {
      const sk = curSkill();
      overlay.querySelector('b').textContent = sk ? t('Engedd el: hozzáadás ehhez: {0}', sk.title) : t('Engedd el: új skill ezekből a fájlokból');
      overlay.querySelector('span').textContent = t('PDF, DOCX, PPTX, ODT, HTML, MD, TXT, képek – mappák is');
      if (!overlay.isConnected) document.body.append(overlay);
    } else overlay.remove();
  };
  await ev.listen('tauri://drag-enter', () => { if (!document.querySelector('.overlay')) show(true); });
  await ev.listen('tauri://drag-leave', () => show(false));
  await ev.listen('tauri://drag-drop', ({ payload }) => {
    show(false);
    if (document.querySelector('.overlay')) return;
    addPaths(payload?.paths || []);
  });
}

async function boot() {
  try { S.settings = await invoke('get_settings'); }
  catch (e) { S.settings = { theme: 'dark' }; toast(String(e), 'err'); }
  S.settings.ui_state ||= {};
  initUiState(S.settings.ui_state, debounce(() => saveSettings(), 300));
  applyLang();
  applyTheme();
  if (zoomNow() !== 1) applyZoom();
  matchMedia('(prefers-color-scheme: light)').addEventListener('change', () => { if (S.settings.theme === 'system') { applyTheme(); renderAll(); } });
  S.defaultOut = await invoke('default_out_dir').catch(() => '');
  S.view = uiGet('ss.view') || 'docs';
  splitter(document.getElementById('side-split'), document.getElementById('sidebar'), { min: 180, max: 520, key: 'ss.sideW' });
  await refreshSkills();
  if (S.settings.restore_session !== false && S.skills.some((s) => s.id === S.settings.active_skill)) {
    S.skillId = S.settings.active_skill;
    await reloadDocs();
  }
  renderAll();
  startDonationTimer();
  loadEngine();
  initDrop();
  window.__TAURI__?.event?.listen('doc-progress', ({ payload }) => onProgress(payload));
  S.queue = await invoke('processing_queue').catch(() => []);

  // files changed outside: refresh the "changed" markers on focus
  let lastFocus = 0;
  window.addEventListener('focus', async () => {
    if (Date.now() - lastFocus < 1500) return;
    lastFocus = Date.now();
    if (S.skillId && S.view === 'docs' && !document.querySelector('.overlay')) await reloadDocs();
  });

  document.addEventListener('keydown', (e) => {
    const mod = e.ctrlKey || e.metaKey;
    if (mod && !e.altKey) {
      const k = e.key, c = e.code;
      const dir = (k === '+' || k === '=' || c === 'NumpadAdd') ? 1 : (k === '-' || c === 'NumpadSubtract') ? -1 : (k === '0' || c === 'Numpad0') ? 0 : null;
      if (dir !== null) { e.preventDefault(); zoomStep(dir); return; }
    }
    if (document.querySelector('.overlay')) return;
    const key = e.key.toLowerCase();
    const sk = curSkill();
    if (e.key === 'F1') { e.preventDefault(); aboutDlg(); }
    else if (e.key === 'F5') { e.preventDefault(); const ids = S.docs.filter((d) => d.status === 'new' || d.status === 'error' || d.stale).map((d) => d.id); if (ids.length) processDocs(ids); }
    else if (mod && key === 'n') { e.preventDefault(); newSkill(); }
    else if (mod && key === 'o') { e.preventDefault(); addFiles(); }
    else if (mod && key === 'b') { e.preventDefault(); if (sk?.done) build(); }
    else if (mod && e.shiftKey && key === 'a') { e.preventDefault(); if (aiOn() && sk) aiAsk(''); }
    else if (mod && key === 'f') { e.preventDefault(); if (sk) setView('search'); }
    else if (mod && key === 'a' && S.view === 'docs' && !e.target.closest?.('input,textarea')) { e.preventDefault(); S.docs.forEach((d) => S.docSel.add(d.id)); renderContent(); }
    else if (e.key === 'Delete' && S.view === 'docs' && S.docSel.size && !e.target.closest?.('input,textarea')) { e.preventDefault(); removeDocs(S.docs.filter((d) => S.docSel.has(d.id))); }
    else if (mod && key === ',') { e.preventDefault(); settingsDlg(); }
    else if (e.key === 'Escape') document.querySelector('.ai-panel')?.remove();
  });
  let wheelSum = 0, wheelLast = 0, wheelAt = 0;
  addEventListener('wheel', (e) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const now = Date.now();
    if (now - wheelLast > 400) wheelSum = 0;
    wheelLast = now;
    wheelSum += e.deltaY;
    if (Math.abs(wheelSum) < 40 || now - wheelAt < 120) return;
    wheelAt = now;
    zoomStep(wheelSum < 0 ? 1 : -1);
    wheelSum = 0;
  }, { passive: false });
  document.addEventListener('contextmenu', (e) => {
    if (e.defaultPrevented) return;
    e.preventDefault();
    editMenu(e);
  });
  if (S.settings.update_check !== false) setTimeout(() => checkForAppUpdate({ silent: true }), 4000);
}

boot();
