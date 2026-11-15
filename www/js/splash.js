// Splash window: version and texts in the UI language.
const invoke = window.__TAURI__?.core?.invoke;

const TEXT = {
  hu: { tag: 'Dokumentumokból Claude skill – helyi OCR, helyi adatbázis', status: 'Betöltés…' },
  en: { tag: 'Claude skills from your documents – local OCR, local database', status: 'Loading…' },
};

async function init() {
  if (!invoke) return;
  try {
    const s = await invoke('get_settings');
    const want = s.ui_language || 'auto';
    const lang = want === 'auto' ? (navigator.language.toLowerCase().startsWith('hu') ? 'hu' : 'en') : want === 'en' ? 'en' : 'hu';
    document.documentElement.lang = lang;
    document.getElementById('tag').textContent = TEXT[lang].tag;
    document.getElementById('status').textContent = TEXT[lang].status;
  } catch { /* keep the Hungarian defaults */ }
  try {
    const info = await invoke('app_info');
    document.getElementById('ver').textContent = 'v' + info.version;
  } catch { /* no version */ }
}

init();
