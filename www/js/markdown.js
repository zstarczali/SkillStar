// SkillStar – Markdown → safe HTML for the preview.
// Full CommonMark + GitHub Flavored Markdown (tables, task lists, strikethrough,
// autolinks, nested lists, HTML blocks …) by `marked`, plus ==highlight== and
// heading anchors. The result always goes through DOMPurify: notes can come
// from anywhere (sync), so no script, handler or iframe may survive.

import { Marked } from '../vendor/marked.esm.js';
import DOMPurify from '../vendor/purify.es.mjs';

let slugs = new Map();

/** "Első lépések!" → "elso-lepesek", unique within one render. */
export function slugify(text) {
  const base = String(text).toLowerCase().normalize('NFD').replace(/[̀-ͯ]/g, '')
    .replace(/<[^>]*>/g, '').replace(/[^\p{L}\p{N}\s-]/gu, '').trim().replace(/\s+/g, '-') || 'section';
  const n = slugs.get(base) || 0;
  slugs.set(base, n + 1);
  return n ? `${base}-${n}` : base;
}

const md = new Marked({ gfm: true, breaks: true });
/**
 * Footnotes: `text[^1]` + a `[^1]: note` line anywhere. The definitions are
 * taken out and listed at the end; the references become numbered links.
 */
function footnotes(src) {
  const lines = src.replace(/\r/g, '').split('\n');
  const defs = new Map();
  let fence = null;
  const body = [];
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    const f = l.match(/^\s*(```+|~~~+)/);
    if (f) { if (!fence) fence = f[1][0]; else if (f[1][0] === fence) fence = null; body.push(l); continue; }
    const m = !fence && l.match(/^\[\^([^\]\s]+)\]:\s?(.*)$/);
    if (!m) { body.push(l); continue; }
    const text = [m[2]];
    // indented lines continue the note
    while (i + 1 < lines.length && /^( {2,}|\t)\S/.test(lines[i + 1])) text.push(lines[++i].trim());
    defs.set(m[1], text.join('\n'));
  }
  if (!defs.size) return src;
  const order = [];
  fence = null;
  const out = body.map((l) => {
    const f = l.match(/^\s*(```+|~~~+)/);
    if (f) { if (!fence) fence = f[1][0]; else if (f[1][0] === fence) fence = null; return l; }
    if (fence) return l;
    // not inside `code`
    return l.split(/(`+[^`]*`+)/).map((part, k) => (k % 2 ? part : part.replace(/\[\^([^\]\s]+)\]/g, (all, id) => {
      if (!defs.has(id)) return all;
      if (!order.includes(id)) order.push(id);
      const n = order.indexOf(id) + 1;
      return `<sup class="fn-ref"><a href="#fn-${fnId(id)}" id="fnref-${fnId(id)}">${n}</a></sup>`;
    }))).join('');
  });
  if (!order.length) return out.join('\n');
  out.push('', '<section class="footnotes">', '', '<hr>', '');
  order.forEach((id, i) => {
    out.push(`<div class="fn" id="fn-${fnId(id)}"><span class="fn-n">${i + 1}.</span><div>`, '', defs.get(id), '', `</div><a class="fn-back" href="#fnref-${fnId(id)}">↩</a></div>`, '');
  });
  out.push('</section>');
  return out.join('\n');
}
const fnId = (id) => String(id).replace(/[^\w-]/g, '_');

const attr = (s) => String(s ?? '').replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
const SIZE = /^\d{1,5}%?$/;
function imgHtml(href, alt, title, w, hgt) {
  return `<img src="${attr(href)}" alt="${attr(alt)}"${title ? ` title="${attr(title)}"` : ''}`
    + `${w && SIZE.test(w) ? ` width="${w}"` : ''}${hgt && SIZE.test(hgt) ? ` height="${hgt}"` : ''}>`;
}
/** "leírás|300" / "leírás|300x200" / "|50%" → [alt, width, height] */
export function splitAlt(text) {
  const m = /^(.*?)\s*\|\s*(\d{1,5}%?)?(?:\s*x\s*(\d{1,5}%?))?\s*$/.exec(text || '');
  return m && (m[2] || m[3]) ? [m[1], m[2] || '', m[3] || ''] : [text || '', '', ''];
}
// ![alt](src =300x) / =300x200 / =x200 / =50%x
const SIZED_IMG = /^!\[([^\]]*)\]\(\s*(<[^>\n]*>|[^\s)]+)\s+=(\d{0,5}%?)x?(\d{0,5}%?)\s*\)/;

md.use({
  hooks: { preprocess(src) { slugs = new Map(); return footnotes(src); } },
  renderer: {
    heading({ tokens, depth, text }) {
      return `<h${depth} id="h-${slugify(text)}">${this.parser.parseInline(tokens)}</h${depth}>\n`;
    },
    // ![alt|300](src): Obsidian style size
    image({ href, title, text }) {
      const [alt, w, hgt] = splitAlt(text);
      return imgHtml(href, alt, title, w, hgt);
    },
  },
  extensions: [{
    name: 'sizedImage',
    level: 'inline',
    start(src) { const i = src.indexOf('!['); return i < 0 ? undefined : i; },
    tokenizer(src) {
      const m = SIZED_IMG.exec(src);
      if (m) return { type: 'sizedImage', raw: m[0], alt: m[1], href: m[2].replace(/^<|>$/g, ''), w: m[3], h: m[4] };
      return undefined;
    },
    renderer(tok) { return imgHtml(tok.href, tok.alt, '', tok.w, tok.h); },
  }, {
    // ==highlight==
    name: 'mark',
    level: 'inline',
    start(src) { const i = src.indexOf('=='); return i < 0 ? undefined : i; },
    tokenizer(src) {
      const m = /^==(?=\S)([\s\S]*?\S)==(?!=)/.exec(src);
      if (m) return { type: 'mark', raw: m[0], tokens: this.lexer.inlineTokens(m[1]) };
      return undefined;
    },
    renderer(tok) { return `<mark>${this.parser.parseInline(tok.tokens)}</mark>`; },
  }],
});

const PURIFY = {
  FORBID_TAGS: ['style', 'form', 'button', 'textarea', 'select', 'iframe', 'object', 'embed'],
  FORBID_ATTR: ['style'],
};

export function mdRender(src) {
  const html = md.parse(String(src ?? ''), { async: false });
  return DOMPurify.sanitize(html, PURIFY);
}

/**
 * Source line of every task checkbox, in document order (fenced code skipped),
 * so a click in the preview can tick the right line.
 */
export function taskLines(src) {
  const out = [];
  let fence = null;
  String(src).replace(/\r/g, '').split('\n').forEach((l, i) => {
    const f = l.match(/^\s*(```+|~~~+)/);
    if (f) { if (!fence) fence = f[1][0]; else if (f[1][0] === fence) fence = null; return; }
    if (!fence && /^\s*(?:>\s*)*(?:[-*+]|\d+[.)])\s+\[[ xX]\](?:\s|$)/.test(l)) out.push(i);
  });
  return out;
}

/** `href` of a link / image in note `noteRel` → path in the notes folder, or null (web, outside). */
export function resolveRel(noteRel, href) {
  let p = String(href || '').trim();
  if (!p || /^[a-z][a-z0-9+.-]*:/i.test(p) || p.startsWith('//') || p.startsWith('#')) return null;
  p = p.replace(/^<|>$/g, '').replace(/[?#].*$/, '');
  try { p = decodeURIComponent(p); } catch { /* keep as is */ }
  const parts = p.startsWith('/') ? [] : noteRel.split('/').slice(0, -1);
  for (const seg of p.replace(/\\/g, '/').split('/')) {
    if (!seg || seg === '.') continue;
    if (seg === '..') { if (!parts.length) return null; parts.pop(); } else parts.push(seg);
  }
  return parts.join('/') || null;
}

const normSrc = (s) => { s = String(s || '').trim().replace(/^<|>$/g, ''); try { return decodeURIComponent(s); } catch { return s; } };

/**
 * Sets the size of the `nth` image (0-based, counted among images with the same
 * `src`) in `text`. `size` = '300' | '50%' | '' (original). Markdown images get
 * the `![alt|300](src)` form, HTML <img> tags a width attribute.
 * Returns the new text, or null when the image is not found.
 */
export function setImageSize(text, src, nth, size) {
  const want = normSrc(src);
  const re = /!\[([^\]]*)\]\(\s*(<[^>\n]*>|[^\s)]+)((?:\s+=[^\s)]*)|(?:\s+"[^"]*"))?\s*\)|<img\b[^>]*>/gi;
  let n = 0;
  for (const m of text.matchAll(re)) {
    const html = m[0].startsWith('<');
    const s = html ? (m[0].match(/\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/i)?.slice(1).find((x) => x != null) ?? '') : m[2];
    if (normSrc(s) !== want) continue;
    if (n++ !== nth) continue;
    let out;
    if (html) {
      out = m[0].replace(/\s+(width|height)\s*=\s*("[^"]*"|'[^']*'|[^\s>]+)/gi, '');
      if (size) out = out.replace(/^<img\b/i, `<img width="${size}"`);
    } else {
      const [alt] = splitAlt(m[1]);
      const title = m[3] && !/^\s+=/.test(m[3]) ? m[3] : '';
      out = `![${alt}${size ? `|${size}` : ''}](${m[2]}${title})`;
    }
    return text.slice(0, m.index) + out + text.slice(m.index + m[0].length);
  }
  return null;
}

/** Path of `rel` as written in note `noteRel` (relative, Markdown-safe). */
export function relLink(noteRel, rel) {
  const from = noteRel.split('/').slice(0, -1);
  const to = rel.split('/');
  let i = 0;
  while (i < from.length && i < to.length - 1 && from[i] === to[i]) i++;
  const path = [...from.slice(i).map(() => '..'), ...to.slice(i)].join('/');
  return path.replace(/%/g, '%25').replace(/ /g, '%20').replace(/\(/g, '%28').replace(/\)/g, '%29');
}
