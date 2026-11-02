'use strict';
// All data reaches the DOM through textContent / createTextNode only. No innerHTML, no inline handlers (CSP).
let session = null, setup = false, epoch = 0, platform = '', backendHealthy = null, localTried = false;
let docCount = null, limits = {}, currentView = 'gate', openedReportId = null;
const $ = s => document.querySelector(s);
const $$ = s => [...document.querySelectorAll(s)];
const VIEWS = ['gate', 'search', 'reports', 'documents', 'settings', 'ai'];
const SCOPE_LABEL = { public: '公開Web', private: '社内資料', combined: '両方' };
// The small label under each scope says whether the query leaves this machine; picking a scope that sends is the consent.
const SCOPE_SEND = { public: '外部へ送信', private: '外部へ送らない', combined: '公開分を送信' };
const SCOPE_ICON = { public: 'i-globe', private: 'i-lock', combined: 'i-both' };
const PROFILE_LABEL = { general: '一般調査', development: '開発・公式仕様', sales: '営業・企業調査', sns: 'SNS・情報確認' };
const ENGINE_REASON = { 'Too many requests': 'アクセス制限', 'Timeout': '時間切れ' };
const SVG_NS = 'http://www.w3.org/2000/svg';

function node(tag, text, cls) { const n = document.createElement(tag); if (text !== undefined && text !== null) n.textContent = text; if (cls) n.className = cls; return n; }
function icon(id) { const s = document.createElementNS(SVG_NS, 'svg'); s.setAttribute('class', 'ic'); s.setAttribute('aria-hidden', 'true'); const u = document.createElementNS(SVG_NS, 'use'); u.setAttribute('href', '#' + id); s.append(u); return s; }
function button(text, cls, onClick) { const b = node('button', text, cls); b.type = 'button'; if (onClick) b.addEventListener('click', onClick); return b; }
function notice(text) { $('#notice').textContent = text || ''; }
function stored(key, value) {
  try { if (value === undefined) return localStorage.getItem(key); localStorage.setItem(key, value); } catch { return null; }
  return null;
}

async function api(path, body, headers) {
  const ticket = epoch;
  const response = await fetch(path, {
    method: body === undefined ? 'GET' : 'POST',
    headers: body === undefined ? {} : { 'Content-Type': 'application/json', 'X-CSRF-Token': session?.csrf || '', ...headers },
    // a Blob (the backup file) is sent as it is: the browser streams it, nothing is parsed or copied on this side
    body: body === undefined ? undefined : body instanceof Blob ? body : JSON.stringify(body)
  });
  let data;
  try { data = await response.json(); } catch { throw Error('応答を読み取れません。画面を更新してください。'); }
  if (ticket !== epoch) throw Error('利用者が切り替わったため、前の操作結果を破棄しました。');
  if (!response.ok) {
    if (response.status === 401 && path !== '/api/login') clearPrivate();
    const id = data.error?.request_id ? '(処理ID ' + String(data.error.request_id).slice(0, 8) + ')' : '';
    const error = Error((data.error?.message || '処理に失敗しました。') + id); error.status = response.status;
    throw error;
  }
  return data;
}
const pause = ms => new Promise(resolve => setTimeout(resolve, ms));

/* ---------- theme (boot.js applies it before the first paint) ---------- */
const THEMES = ['auto', 'light', 'dark'];
const darkQuery = window.matchMedia ? window.matchMedia('(prefers-color-scheme: dark)') : null;
function themePref() { const t = stored('talmudo_theme'); return THEMES.includes(t) ? t : 'auto'; }
function applyTheme(pref) {
  const dark = pref === 'dark' || (pref !== 'light' && !!darkQuery?.matches);
  document.documentElement.dataset.theme = dark ? 'dark' : 'light'; document.documentElement.dataset.themePref = pref;
  for (const b of $$('[data-theme-set]')) b.setAttribute('aria-pressed', String(b.dataset.themeSet === pref));
}
for (const b of $$('[data-theme-set]')) b.addEventListener('click', () => { stored('talmudo_theme', b.dataset.themeSet); applyTheme(b.dataset.themeSet); });
darkQuery?.addEventListener?.('change', () => applyTheme(themePref()));
applyTheme(themePref());

/* ---------- views ---------- */
function showOnly(view) {
  currentView = view;
  for (const name of VIEWS) $('#view-' + name).hidden = name !== view;
  $('#app').classList.toggle('gate-mode', view === 'gate');
  $('#app').dataset.view = view;
  const tab = view === 'ai' ? 'settings' : view;
  for (const b of $$('#nav button')) b.setAttribute('aria-current', b.dataset.view === tab ? 'page' : 'false');
  // Phone: away from 調べる, or while a result is on screen, the box folds to one line (scope chip + query + 調べる).
  setCompact(view !== 'search' || !!$('#result').firstChild);
  updatePrivateEmpty();
  window.scrollTo(0, 0);
}
async function showView(view) {
  if (!session && view !== 'ai') view = 'gate';
  showOnly(view);
  if (view === 'documents') await loadDocs();
  if (view === 'reports') { $('#report-detail').hidden = true; $('#report-list').hidden = false; openedReportId = null; await loadReports(); }
  if (view === 'settings') await health();
  if (view === 'ai') $('#mcp-download').hidden = !session;
  if (view === 'search') { $('#query').focus({ preventScroll: true }); if (!$('#result').firstChild) await loadRecent(); }
}

function clearPrivate() {
  epoch++; session = null; docCount = null; openedReportId = null;
  for (const s of ['#result', '#recent', '#saved-result', '#document-list', '#report-list', '#diagnostics', '#health']) $(s).replaceChildren();
  $('#document-detail').textContent = ''; $('#document-detail').hidden = true;
  $('#nav').hidden = true; $('#logout').hidden = true; $('#identity').textContent = ''; $('#gate-ai-link').hidden = false;
  $('#side-foot').hidden = true; $('#search-form').hidden = true; for (const n of $$('[data-identity], [data-avatar]')) n.textContent = '';
  $('#health-dot').className = 'dot none'; setCompact(false);
  $('#phone-link').hidden = true; $('#phone-qr').removeAttribute('src'); $('#phone-url').textContent = '';
  $('#delete-all-confirm').hidden = true; $('#delete-all').hidden = true; $('#report-count').textContent = '';
  for (const f of ['#search-form', '#document-form', '#account-form', '#restore-form']) $(f).reset();
  setDocCount(null);
}

function randomPassword() {
  const alphabet = 'abcdefghijkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789';
  const bytes = new Uint8Array(24); crypto.getRandomValues(bytes);
  return [...bytes].map(b => alphabet[b % alphabet.length]).join('');
}

// 社内資料 = the documents this person registered. The count sits on the scope tile and on the 社内資料 page; 0 shows the way to add one.
function setDocCount(n) {
  docCount = typeof n === 'number' ? n : null;
  for (const el of $$('[data-doc-count]')) { el.textContent = docCount === null ? '' : docCount.toLocaleString('ja-JP') + '件'; el.hidden = docCount === null; }
  if (session) scopeChanged(); else updatePrivateEmpty();
}
function updatePrivateEmpty() {
  const box = $('#private-empty');
  box.hidden = !(session && currentView === 'search' && currentScope() === 'private' && docCount === 0 && !$('#result').firstChild);
}

async function init() {
  const data = await api('/api/session');
  setup = data.setup_needed; platform = data.platform || ''; limits = data.limits || {};
  session = data.user ? data : null;
  // On this PC the server signs in the first admin by itself (not through the tunnel); once per page load, so logout sticks.
  if (!session && data.this_pc && !localTried) {
    localTried = true;
    try { await api('/api/local-login', {}); return await init(); } catch { /* fall back to the password screen */ }
  }
  const gate = $('#view-gate');
  if (limits.document_chars) $('#document-form [name=body]').maxLength = limits.document_chars;
  if (!session) {
    $('#auth-form').hidden = !setup; $('#pw-open').hidden = setup;
    $('#gate-title').textContent = setup ? 'はじめる' : 'ログイン';
    $('#auth-submit').textContent = setup ? 'はじめる' : 'ログイン';
    const form = $('#auth-form');
    form.elements.password.autocomplete = setup ? 'new-password' : 'current-password';
    if (setup) { form.elements.name.value = form.elements.name.value || '自分'; if (!form.elements.password.value) form.elements.password.value = randomPassword(); }
    $('#nav').hidden = true; $('#logout').hidden = true; $('#gate-ai-link').hidden = false; $('#side-foot').hidden = true; $('#search-form').hidden = true;
    if (!$('#view-ai').hidden) return;
    showOnly('gate'); gate.hidden = false;
    return;
  }
  $('#identity').textContent = data.user.name; $('#logout').hidden = false; $('#nav').hidden = false; $('#gate-ai-link').hidden = true;
  $('#side-foot').hidden = false; $('#search-form').hidden = false;
  for (const n of $$('[data-identity]')) n.textContent = data.user.name;
  for (const n of $$('[data-avatar]')) n.textContent = [...data.user.name][0] || '';
  const admin = data.user.role === 'admin';
  for (const s of ['#account-controls', '#restart', '#stop']) $(s).hidden = !admin;
  $('#phone-open').hidden = !(admin && data.phone_link);
  setDocCount(data.document_count);
  const pref = await api('/api/preferences'); $('#profile').value = pref.profile;
  const savedScope = stored('scope');
  if (SCOPE_LABEL[savedScope]) setScope(savedScope); else scopeChanged();
  await showView('search');
}

async function action(form, fn) {
  const b = form.querySelector('button.primary') || form.querySelector('button');
  const text = b.textContent; b.disabled = true; b.textContent = '処理中…'; notice('');
  try { await fn(); } catch (e) { notice(e.message); } finally { b.disabled = false; b.textContent = text; }
}

/* ---------- login / first run ---------- */
$('#auth-form').addEventListener('submit', e => {
  e.preventDefault();
  action(e.target, async () => {
    const f = new FormData(e.target);
    await api(setup ? '/api/setup' : '/api/login', { name: f.get('name'), password: f.get('password') });
    e.target.reset(); $('#view-gate').hidden = true; await init(); notice('');
  });
});
$('#pw-open').addEventListener('click', () => { $('#auth-form').hidden = false; $('#pw-open').hidden = true; $('#auth-form').elements.name.focus(); });
$('#phone-open').addEventListener('click', async () => {
  try {
    const d = await api('/api/phone-link', {});
    $('#phone-qr').src = 'data:image/svg+xml;charset=utf-8,' + encodeURIComponent(d.qr_svg);
    $('#phone-url').textContent = d.url; $('#phone-link').hidden = false;
  } catch (e) { notice(e.message); }
});
$('#pw-toggle').addEventListener('click', () => {
  const field = $('#auth-password'); const show = field.type === 'password';
  field.type = show ? 'text' : 'password'; $('#pw-toggle').textContent = show ? '隠す' : '表示';
});
async function logout() {
  try { await api('/api/logout', {}); clearPrivate(); localTried = true; await init(); notice('ログアウトしました。'); } catch (e) { notice(e.message); }
}
for (const b of [$('#logout'), ...$$('[data-logout]')]) b.addEventListener('click', logout);

/* ---------- search form (lives in the band, usable from every screen) ---------- */
function currentScope() { return $('#search-form').elements.scope.value; }
function setScope(value) { for (const r of $$('#search-form [name=scope]')) r.checked = r.value === value; scopeChanged(); }
function scopeChanged() {
  const scope = currentScope();
  const chip = $('#scope-chip');
  chip.querySelector('[data-chip-label]').textContent = SCOPE_LABEL[scope] + (scope !== 'public' && docCount !== null ? ' ' + docCount.toLocaleString('ja-JP') + '件' : '');
  chip.querySelector('[data-chip-note]').textContent = SCOPE_SEND[scope];
  chip.querySelector('use').setAttribute('href', '#' + SCOPE_ICON[scope]);
  chip.classList.toggle('safe', scope === 'private');
  $('#public-query-row').hidden = scope !== 'combined';
  $('#search-form').elements.profile.closest('label').hidden = scope === 'private';
  $('#search-form').elements.domains.closest('label').hidden = scope === 'private';
  stored('scope', scope);
  updatePrivateEmpty();
}
for (const r of $$('#search-form [name=scope]')) r.addEventListener('change', scopeChanged);
function setCompact(on) { $('#search-form').classList.toggle('compact', on); $('#scope-chip').setAttribute('aria-expanded', String(!on)); }
$('#scope-chip').addEventListener('click', () => { setCompact(false); $('#search-form [name=scope]:checked').focus(); });

$('#search-form').addEventListener('submit', e => {
  e.preventDefault();
  const form = e.target; const f = new FormData(form); const scope = f.get('scope');
  const query = String(f.get('query') || '').trim();
  if (!query) { $('#query').focus(); return; }
  const publicScope = scope !== 'private';
  const data = {
    scope, query, profile: f.get('profile'),
    // The scope label (外部へ送信 / 公開分を送信) is the consent; the flag is only ever true for a scope that sends.
    public_confirmed: publicScope,
    domains: String(f.get('domains') || '').split(',').map(s => s.trim()).filter(Boolean)
  };
  if (scope === 'combined') data.public_query = String(f.get('public_query') || '').trim() || query;
  action(form, async () => {
    if (currentView !== 'search') showOnly('search');
    const box = $('#result');
    $('#recent').replaceChildren(); $('#private-empty').hidden = true; setCompact(true);
    box.replaceChildren(loadingState(publicScope, query, scope));
    let result;
    try { result = await api('/api/search', data); } catch (error) { box.replaceChildren(failureBlock(error.message)); throw error; }
    renderReport(box, result, { live: true });
    if (publicScope) await api('/api/preferences', { profile: data.profile });
    notice('');
  });
});

function fmtSeconds(s) { if (typeof s !== 'number' || !isFinite(s)) return null; return s < 1 ? s.toFixed(2) : s < 10 ? s.toFixed(1) : String(Math.round(s)); }

// While waiting, the number people wait for is already on screen and ticking; the cards below are skeletons.
function loadingState(publicScope, query, scope) {
  const wrap = node('section', undefined, 'rep loading'); wrap.setAttribute('aria-busy', 'true');
  const main = node('div', undefined, 'rep-main');
  const head = node('header', undefined, 'rep-head'); head.append(node('h2', query));
  const chips = node('div', undefined, 'chips'); chips.append(chip(SCOPE_LABEL[scope] || '調査', '')); head.append(chips); main.append(head);
  const speed = node('section', undefined, 'speed');
  const big = node('p', undefined, 'sp-t num'); const t = node('span', '0.0'); big.append(t, node('small', '秒'));
  const line = node('p', undefined, 'sp-l'); line.append(node('span', undefined, 'spin'), document.createTextNode(publicScope ? '検索して本文を取得しています' : '社内資料を検索しています'));
  speed.append(big, line); main.append(speed);
  const started = Date.now();
  const tick = setInterval(() => { if (!t.isConnected) { clearInterval(tick); return; } t.textContent = fmtSeconds((Date.now() - started) / 1000); }, 100);
  for (let k = 0; k < 3; k++) { const c = node('div', undefined, 'skcard'); for (const w of ['w40', 'w92', 'w76']) c.append(node('div', undefined, 'sk ' + w)); main.append(c); }
  wrap.append(main);
  return wrap;
}

function failureBlock(message) {
  const d = node('section', undefined, 'state bad'); d.setAttribute('role', 'alert');
  d.append(node('p', '調べられませんでした', 'line1'), node('p', message || '検索語を変えて、もう一度試してください。'));
  const a = node('div', undefined, 'actions state-actions');
  a.append(button('設定を開く', 'btn btn-secondary', () => showView('settings').catch(e => notice(e.message))));
  d.append(a);
  return d;
}

/* ---------- report rendering ---------- */
const STATUS_TEXT = {
  ok: ['本文あり', 'good'], extracted: ['本文あり', 'good'],
  partial: ['一部だけ', 'warn'], short_text: ['一部だけ', 'warn'],
  challenge_page: ['取れなかった(ロボット確認)', 'bad'],
  access_denied: ['取れなかった(アクセス拒否)', 'bad'],
  rate_limited: ['取れなかった(アクセス制限)', 'bad'],
  timeout: ['取れなかった(時間切れ)', 'bad'],
  empty_text: ['取れなかった(本文なし)', 'bad'],
  body_too_large: ['取れなかった(大きすぎる)', 'bad'],
  unsupported_content_type: ['取れなかった(HTML以外)', 'bad'],
  http_error: ['取れなかった', 'bad'], network_error: ['取れなかった', 'bad'],
  blocked_address: ['安全のため取得せず', 'bad']
};
function statusBadge(source) {
  if (source.scope === 'private') return ['登録資料', 'info'];
  const q = source.quality, code = source.evidence?.code;
  if (q === 'redirected_offsite') return ['別のサイトへ転送', 'bad'];
  if (q === 'unverified') return ['本文あり(確認できず)', 'warn'];
  if (q === 'not_fetched') return ['本文は未取得', 'info'];
  if (q === 'ok') return STATUS_TEXT.ok;
  if (q === 'partial') return STATUS_TEXT[code] || STATUS_TEXT.partial;
  if (q === 'failed') return STATUS_TEXT[code] || ['取れなかった', 'bad'];
  return ['取れなかった', 'bad'];
}
function isUsable(s) { return s.scope === 'private' ? true : (s.quality === 'ok' && s.term_matches > 0); }
// 取れなかった = really tried and failed (or the page moved to another site). Never fetched = a separate count.
function isMissing(s) { return s.scope === 'public' && ['failed', 'redirected_offsite'].includes(s.quality); }
function isUnfetched(s) { return s.scope === 'public' && s.quality === 'not_fetched'; }
// The four parts of the summary bar, built only from the three judgements above.
function bucket(s) { if (isUsable(s)) return 'usable'; if (isMissing(s)) return 'missing'; if (isUnfetched(s)) return 'unfetched'; return 'nomatch'; }

function pad(n) { return String(n).padStart(2, '0'); }
function parseDate(value) { if (value === null || value === undefined || value === '') return null; const d = typeof value === 'number' ? new Date(value * 1000) : new Date(value); return isNaN(d) ? null : d; }
function fmtDate(d) { return d.getFullYear() + '-' + pad(d.getMonth() + 1) + '-' + pad(d.getDate()); }
function fmtDateTime(d) { return fmtDate(d) + ' ' + pad(d.getHours()) + ':' + pad(d.getMinutes()); }

const CHIP_TONE = { good: 'ok', ok: 'ok', warn: 'warn', bad: 'bad', info: 'info' };
function chip(text, tone, extra) { return node('span', text, 'chip ' + (CHIP_TONE[tone] || 'line') + (extra ? ' ' + extra : '')); }

function appendHighlighted(parent, text, terms) {
  const lower = text.toLowerCase(); const marks = new Array(text.length).fill(false);
  for (const t of terms || []) {
    const needle = String(t).toLowerCase(); if (!needle) continue;
    let from = 0, at;
    while ((at = lower.indexOf(needle, from)) !== -1) { for (let i = at; i < at + needle.length; i++) marks[i] = true; from = at + 1; }
  }
  let i = 0;
  while (i < text.length) {
    let j = i; while (j < text.length && marks[j] === marks[i]) j++;
    const piece = text.slice(i, j);
    if (marks[i]) parent.append(node('mark', piece)); else parent.append(document.createTextNode(piece));
    i = j;
  }
}

function hostOf(url) { try { return new URL(url).hostname; } catch { return ''; } }

async function openDocument(id) {
  const d = await api('/api/documents/' + id); const pre = $('#document-detail'); pre.textContent = d.title + '\n\n' + d.body; pre.dataset.id = id; pre.hidden = false; pre.scrollIntoView();
}

function sourceCard(source) {
  const usable = isUsable(source);
  const card = node('article', undefined, 'scard ' + (usable ? 'usable' : 'unusable'));
  const head = node('div', undefined, 'scard-h');
  head.append(node('span', source.citation, 'cite'));
  const titles = node('div', undefined, 'scard-t');
  titles.append(node('h3', source.title || '(題名なし)'));
  const sub = node('div', undefined, 'sub');
  const cite = source.cite_url || source.url;
  if (cite) {
    const a = node('a', undefined, 'src'); a.href = cite; a.target = '_blank'; a.rel = 'noopener noreferrer'; a.title = cite;
    a.append(document.createTextNode(hostOf(cite) || cite), icon('i-ext')); sub.append(a);
  }
  const [label, tone] = statusBadge(source);
  sub.append(chip(label, tone));
  if (source.scope === 'public') sub.append(source.official ? chip('公式', 'good') : source.preferred_domain ? chip('指定のドメイン', 'info') : chip('公式か未確認', ''));
  titles.append(sub); head.append(titles); card.append(head);
  if (source.quality === 'redirected_offsite') card.append(node('p', '検索結果の ' + hostOf(source.url) + ' から ' + hostOf(source.final_url) + ' へ転送されたため、根拠としては使っていません。', 'foot-note'));
  if (source.excerpt) { const quote = node('p', undefined, 'quote'); appendHighlighted(quote, source.excerpt, source.match_terms); card.append(quote); }
  else if (source.scope === 'private') card.append(node('p', '題名だけが一致しました。本文には検索語がありません。', 'foot-note'));
  const facts = node('p', undefined, 'facts num');
  const fact = (k, v) => { const sp = node('span', k); sp.append(node('b', v)); facts.append(sp); };
  if (source.scope === 'public') {
    const fetched = parseDate(source.evidence?.fetched_at), published = parseDate(source.published_at);
    if (fetched) fact('取得', fmtDateTime(fetched));
    fact('公表日', published ? fmtDate(published) : '不明');
    if (source.evidence?.characters) fact('本文', Number(source.evidence.characters).toLocaleString('ja-JP') + '文字' + (source.evidence.truncated ? '(一部)' : ''));
    if (usable || fetched) card.append(facts);
  } else {
    const created = parseDate(source.created_at);
    if (created) { fact('登録', fmtDate(created)); card.append(facts); }
    const actions = node('div', undefined, 'actions');
    actions.append(button('資料を開く', 'btn btn-secondary', async () => { try { await showView('documents'); await openDocument(source.id); } catch (e) { notice(e.message); } }));
    card.append(actions);
  }
  return card;
}

function conflictBlock(c) {
  const box = node('section', undefined, 'conflict');
  box.append(node('h3', '食い違いの可能性: ' + (c.label || '')), node('p', c.message, 'foot-note'));
  const grid = node('div', undefined, 'sidebyside');
  for (const o of c.observations || []) {
    const cell = node('div', undefined, 'obs');
    cell.append(node('strong', '[' + o.citation + ']', 'num'), node('p', o.text));
    grid.append(cell);
  }
  box.append(grid);
  return box;
}

// 速さが主役: the seconds first, then 使える根拠 n / m and the four-part bar. One function, called for the phone and the PC copy.
function speedPanel(r, sources, n, conflicts) {
  const panel = node('section', undefined, 'speed');
  const secs = fmtSeconds(r.elapsed_seconds ?? r.public?.elapsed_seconds);
  const big = node('p', undefined, 'sp-t num');
  if (secs !== null) big.append(document.createTextNode(secs), node('small', '秒')); else big.append(document.createTextNode('—'));
  panel.append(big);
  const row = node('div', undefined, 'sp-row');
  const u = node('p', undefined, 'sp-n num'); u.append(node('span', '使える根拠', 'sp-k'), document.createTextNode(String(n.usable)), node('small', '/ ' + sources.length));
  row.append(u);
  if (r.scope === 'private') {
    if (docCount !== null) { const d = node('p', undefined, 'sp-n num'); d.append(node('span', '登録した資料', 'sp-k'), document.createTextNode(docCount.toLocaleString('ja-JP')), node('small', '件')); row.append(d); }
    panel.append(row); return panel;
  }
  panel.append(row);
  const bar = node('div', undefined, 'bar'); bar.setAttribute('role', 'img');
  bar.setAttribute('aria-label', '使える ' + n.usable + '件、本文は未取得 ' + n.unfetched + '件、本文に一致なし ' + n.nomatch + '件、取れなかった ' + n.missing + '件');
  for (const [k, cls] of [['usable', 'ok'], ['nomatch', 'warn'], ['unfetched', 'none'], ['missing', 'bad']]) if (n[k]) { const part = node('i', undefined, cls); part.style.flex = String(n[k]); bar.append(part); }
  const legend = node('ul', undefined, 'legend');
  for (const [k, cls, label] of [['usable', 'ok', '使える'], ['unfetched', 'none', '本文は未取得'], ['nomatch', 'warn', '本文に一致なし'], ['missing', 'bad', '取れなかった']]) {
    const li = node('li'); li.append(node('span', undefined, 'dot ' + cls), node('span', label), node('b', String(n[k]), 'num')); legend.append(li);
  }
  const sub = node('div', undefined, 'hero-sub'); sub.append(chip('金額の食い違い ' + conflicts.length, conflicts.length ? 'warn' : '', 'num'));
  panel.append(bar, legend, sub);
  return panel;
}

// この調査: what was sent, which engines did not answer, Markdown, and the server's note on what is verified.
function infoPanel(r) {
  const panel = node('section', undefined, 'panel');
  panel.append(node('h2', 'この調査'));
  const dl = node('dl', undefined, 'kv2');
  const kv = (k, v) => { const d = node('div'); d.append(node('dt', k), node('dd', v, 'num')); dl.append(d); };
  kv('送信した語', r.queries?.length ? r.queries.join(' / ') : (r.external_query_sent === null ? '結果を確認できません' : 'なし'));
  const failures = r.engine_failures || r.public?.engine_failures || [];
  if (failures.length) kv('応答しなかった検索元', failures.map(x => Array.isArray(x) ? x[0] + '(' + (ENGINE_REASON[x[1]] || '応答なし') + ')' : String(x)).join('、'));
  panel.append(dl);
  const actions = node('div', undefined, 'actions');
  const download = button(undefined, 'btn btn-secondary', () => {
    const blob = new Blob([r.markdown || ''], { type: 'text/markdown' }); const url = URL.createObjectURL(blob);
    const a = node('a'); a.href = url; a.download = 'research-evidence.md'; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  });
  download.append(icon('i-save'), document.createTextNode('Markdownで保存'));
  actions.append(download); panel.append(actions);
  panel.append(node('p', (r.verification || '') + (r.errors?.length ? ' 一部の処理が完了しませんでした。' : ''), 'foot-note'));
  return panel;
}

function wrap(cls, child) { const d = node('div', undefined, cls); d.append(child); return d; }

function renderReport(container, r, opts = {}) {
  container.replaceChildren();
  const sources = r.sources || [];
  const usable = sources.filter(isUsable), others = sources.filter(s => !isUsable(s));
  const conflicts = r.conflict_candidates || r.public?.conflict_candidates || [];
  const n = { usable: 0, nomatch: 0, unfetched: 0, missing: 0 }; for (const s of sources) n[bucket(s)]++;
  const isPrivate = r.scope === 'private';
  if (opts.live) { // too few usable sources: keep the box open with 条件を絞る open instead of an advice sentence
    const thin = !isPrivate && usable.length < 3; setCompact(!thin); if (thin) $('#options').open = true;
  }
  const rep = node('section', undefined, 'rep');
  const main = node('div', undefined, 'rep-main');
  const rail = node('aside', undefined, 'rail'); rail.setAttribute('aria-label', '調査の要約');

  const head = node('header', undefined, 'rep-head');
  head.append(node('h2', r.query || ''));
  const chips = node('div', undefined, 'chips');
  chips.append(chip(SCOPE_LABEL[r.scope] || '調査', ''));
  const profile = r.profile || r.public?.profile;
  if (!isPrivate && PROFILE_LABEL[profile]) chips.append(chip(PROFILE_LABEL[profile], ''));
  const created = parseDate(r.created_at);
  chips.append(chip(created ? fmtDateTime(created) : '—', '', 'num'));
  head.append(chips); main.append(head);

  if (r.scope === 'combined' && r.public?.status === 'failed') {
    const w = node('section', undefined, 'state warn'); w.setAttribute('role', 'status');
    w.append(node('p', '公開検索を完了できませんでした', 'line1'));
    if (r.external_query_sent === null) w.append(node('p', '送信結果を確認できません'));
    main.append(w);
  }
  main.append(wrap('m-only', speedPanel(r, sources, n, conflicts)));
  rail.append(wrap('pc-only', speedPanel(r, sources, n, conflicts)));
  if (!sources.length) {
    const st = node('section', undefined, 'state');
    st.append(node('p', isPrivate ? '一致する資料がありません' : '結果がありませんでした', 'line1'));
    const a = node('div', undefined, 'actions state-actions');
    if (isPrivate) a.append(button('資料を登録', 'btn btn-secondary', () => showView('documents').catch(e => notice(e.message))));
    else if (opts.live) a.append(button('条件を絞る', 'btn btn-secondary', () => { setCompact(false); const d = $('#options'); d.open = true; d.querySelector('summary').focus(); }));
    if (a.firstChild) st.append(a);
    main.append(st);
  }
  for (const c of conflicts) main.append(conflictBlock(c));
  if (sources.length) {
    const tabs = node('div', undefined, 'tabs'); tabs.setAttribute('role', 'tablist'); tabs.setAttribute('aria-label', '結果の絞り込み');
    const list = node('div', undefined, 'cardlist'); list.setAttribute('role', 'tabpanel');
    const draw = arr => list.replaceChildren(...arr.map(sourceCard));
    const tab = (label, arr, selected) => {
      const b = button(label, undefined, () => { for (const x of tabs.children) x.setAttribute('aria-selected', String(x === b)); draw(arr); });
      b.setAttribute('role', 'tab'); b.setAttribute('aria-selected', String(selected)); b.append(node('span', String(arr.length), 'tab-n')); tabs.append(b);
    };
    tab('すべて', sources, true); tab('使える', usable, false); tab('使えなかった', others, false);
    main.append(tabs, list); draw(sources);
  }
  main.append(wrap('m-only', infoPanel(r)));
  rail.append(wrap('pc-only', infoPanel(r)));
  rep.append(main, rail); container.append(rep);
}

function emptyState(iconId, text) {
  const box = node('div', undefined, 'empty');
  const ico = node('div', undefined, 'es-ico'); ico.append(icon(iconId));
  box.append(ico, node('p', text, 'es-t'));
  return box;
}

/* ---------- documents ---------- */
$('#document-form').addEventListener('submit', e => {
  e.preventDefault();
  action(e.target, async () => {
    const f = new FormData(e.target);
    await api('/api/documents', { title: f.get('title'), body: f.get('body') });
    e.target.reset(); await loadDocs(); notice('資料を登録しました。外部には送っていません。');
  });
});
async function loadDocs() {
  const d = await api('/api/documents'); const list = $('#document-list'); list.replaceChildren();
  setDocCount(d.documents.length);
  if (!d.documents.length) { list.append(emptyState('i-doc', 'まだ資料がありません')); return; }
  const card = node('div', undefined, 'card'); card.append(node('h2', '登録した資料'));
  for (const doc of d.documents) {
    const row = node('div', undefined, 'list-row');
    const text = node('div', undefined, 'row-text'); text.append(node('strong', doc.title), node('span', Number(doc.characters).toLocaleString('ja-JP') + '文字 · ' + (parseDate(doc.created) ? fmtDate(parseDate(doc.created)) : ''), 'meta'));
    // Same two taps as a saved research: the bin, then 削除する (or やめる). The row goes only after the server said it is gone.
    const side = node('span', undefined, 'rrow-side');
    const open = button('開く', 'btn btn-secondary', () => openDocument(doc.id).catch(e => notice(e.message)));
    const del = button(undefined, 'rrow-del', () => { row.classList.add('confirming'); yes.focus(); });
    del.setAttribute('aria-label', '削除'); del.title = '削除'; del.append(icon('i-trash'));
    const yes = button('削除する', 'btn btn-danger-fill sm', async () => {
      yes.disabled = true;
      try {
        await api('/api/documents/' + doc.id + '/delete', {});
        if ($('#document-detail').dataset.id === doc.id) { $('#document-detail').textContent = ''; $('#document-detail').hidden = true; }
        notice(''); await loadDocs();
      } catch (e) { yes.disabled = false; row.classList.remove('confirming'); notice(e.message); }
    });
    const no = button('やめる', 'btn btn-secondary sm', () => { row.classList.remove('confirming'); del.focus(); });
    side.append(open, del, yes, no);
    row.append(text, side);
    card.append(row);
  }
  list.append(card);
}

/* ---------- saved reports (open / delete one / delete all) ---------- */
function reportRow(report, onDeleted) {
  const row = node('div', undefined, 'rrow');
  const main = button(undefined, 'rrow-main', () => openReport(report.id));
  main.append(node('strong', report.query));
  const chips = node('span', undefined, 'chips');
  const created = parseDate(report.created);
  chips.append(chip(created ? fmtDateTime(created) : '日時不明', '', 'num'), chip(SCOPE_LABEL[report.scope] || '調査', report.scope === 'private' ? 'info' : ''));
  chips.append(chip('使える根拠 ' + (report.usable ?? 0) + (report.total ? ' / ' + report.total : ''), report.usable > 0 ? 'good' : 'warn', 'num'));
  main.append(chips);
  // Delete is two taps on the same row: the bin, then 削除する (or やめる). Nothing leaves the list until the server said it is gone.
  const side = node('span', undefined, 'rrow-side');
  const del = button(undefined, 'rrow-del', () => { row.classList.add('confirming'); yes.focus(); });
  del.setAttribute('aria-label', '削除'); del.title = '削除'; del.append(icon('i-trash'));
  const yes = button('削除する', 'btn btn-danger-fill sm', async () => {
    yes.disabled = true;
    try { await api('/api/reports/' + report.id + '/delete', {}); row.remove(); notice(''); if (onDeleted) await onDeleted(); }
    catch (e) { yes.disabled = false; row.classList.remove('confirming'); notice(e.message); }
  });
  const no = button('やめる', 'btn btn-secondary sm', () => { row.classList.remove('confirming'); del.focus(); });
  side.append(del, yes, no);
  row.append(main, side);
  return row;
}
async function openReport(id) {
  try {
    const r = await api('/api/reports/' + id);
    showOnly('reports'); renderReport($('#saved-result'), r); openedReportId = id; $('#report-list').hidden = true; $('#report-detail').hidden = false; window.scrollTo(0, 0);
  } catch (e) { notice(e.message); }
}
async function loadReports() {
  const d = await api('/api/reports'); const list = $('#report-list'); list.replaceChildren();
  $('#report-count').textContent = d.reports.length ? d.reports.length + '件' : '';
  $('#delete-all').hidden = !d.reports.length; if (!d.reports.length) $('#delete-all-confirm').hidden = true;
  if (!d.reports.length) { list.append(emptyState('i-saved', 'まだ調査がありません')); return; }
  const rows = node('div', undefined, 'rows'); for (const report of d.reports) rows.append(reportRow(report, loadReports)); list.append(rows);
}
async function loadRecent() {
  const d = await api('/api/reports'); const box = $('#recent'); box.replaceChildren();
  if ($('#result').firstChild) return;
  if (!d.reports.length) { box.append(emptyState('i-saved', 'まだ調査がありません')); return; }
  const h = node('div', undefined, 'sec-h'); h.append(node('h2', '最近の調査'));
  if (d.reports.length > 5) h.append(button('すべて見る', 'btn btn-text', () => showView('reports').catch(e => notice(e.message))));
  const rows = node('div', undefined, 'rows'); for (const report of d.reports.slice(0, 5)) rows.append(reportRow(report, loadRecent));
  box.append(h, rows);
}
$('#report-back').addEventListener('click', () => { $('#report-detail').hidden = true; $('#report-list').hidden = false; openedReportId = null; loadReports().catch(e => notice(e.message)); });
$('#report-delete').addEventListener('click', async () => {
  const b = $('#report-delete');
  if (!b.classList.contains('armed')) { b.classList.add('armed'); b.textContent = '本当に削除する'; setTimeout(() => { b.classList.remove('armed'); b.replaceChildren(icon('i-trash'), document.createTextNode('この調査を削除')); }, 4000); return; }
  try { await api('/api/reports/' + openedReportId + '/delete', {}); b.classList.remove('armed'); b.replaceChildren(icon('i-trash'), document.createTextNode('この調査を削除')); $('#saved-result').replaceChildren(); await showView('reports'); notice(''); }
  catch (e) { notice(e.message); }
});
$('#delete-all').addEventListener('click', () => { $('#delete-all-text').textContent = '保存した調査 ' + $('#report-count').textContent + 'をすべて削除します'; $('#delete-all-confirm').hidden = false; $('#delete-all-yes').focus(); });
$('#delete-all-no').addEventListener('click', () => { $('#delete-all-confirm').hidden = true; $('#delete-all').focus(); });
$('#delete-all-yes').addEventListener('click', async () => {
  const b = $('#delete-all-yes'); b.disabled = true;
  try { await api('/api/reports/delete-all', {}); $('#delete-all-confirm').hidden = true; await loadReports(); notice(''); }
  catch (e) { notice(e.message); } finally { b.disabled = false; }
});

/* ---------- settings ---------- */
async function health() {
  const d = await api('/api/operations'); backendHealthy = d.backend_healthy;
  const line = $('#health'); line.textContent = d.backend_healthy ? '検索エンジンは動いています。' : '検索エンジンに接続できません。再起動してください。';
  line.className = 'status-line ' + (d.backend_healthy ? 'good' : 'bad');
  $('#health-dot').className = 'dot ' + (d.backend_healthy ? 'ok' : 'bad');
  $('#diagnostics').textContent = JSON.stringify(d.events, null, 2);
}
$('#refresh-health').addEventListener('click', () => health().then(() => notice('')).catch(e => notice(e.message)));
$('#restart').addEventListener('click', async () => {
  try { notice('再起動しています。'); await api('/api/restart', {}); await health(); notice(backendHealthy ? '再起動しました。' : '再起動を開始しました。数秒後に「状態を確認」を押してください。'); } catch (e) { notice(e.message); }
});
$('#restore-form').addEventListener('submit', e => {
  e.preventDefault();
  action(e.target, async () => {
    const file = $('#restore-file').files[0];
    const most = limits.restore_bytes || 16 * 1024 * 1024;
    if (!file || file.size > most) throw Error(Math.floor(most / 1000000) + 'MB以下のバックアップを選んでください。');
    const d = await restoreInPieces(file);
    const docs = await api('/api/documents'); setDocCount(docs.documents.length);  // the count is right before the message says done
    notice(d.restored + '件を復元しました。');
  });
});
// The file goes up in pieces (each well under any tunnel's request cap); the server restores from its copy on disk and the screen
// asks for the state, so a full library (minutes) never depends on one long request. A lost piece is sent again from where the server is.
async function restoreInPieces(file) {
  const up = await api('/api/restore/upload', { size: file.size });
  const base = '/api/restore/upload/' + up.upload;
  let offset = 0, misses = 0;
  while (offset < file.size) {
    try {
      const r = await api(base, file.slice(offset, Math.min(file.size, offset + up.chunk_bytes)), { 'Content-Type': 'application/octet-stream', 'X-Upload-Offset': String(offset) });
      offset = r.received; misses = 0;
    } catch (e) {
      if ((e.status && e.status !== 409 && e.status < 500) || ++misses > 4) throw e;
      await pause(1000 * misses); offset = (await api(base)).received;
    }
  }
  // finish may be sent again (the server starts the restore once): when its answer is lost the screen asks for the state and goes on from there
  let finishError = null, resends = 0;
  const finish = async () => { try { await api(base + '/finish', {}); finishError = null; } catch (e) { if (e.status && e.status < 500) throw e; finishError = e; } };
  await finish();
  for (misses = 0; ; ) {
    await pause(1000);
    let s;
    try { s = await api(base); misses = 0; } catch (e) { if ((e.status && e.status < 500) || ++misses > 60) throw e; continue; }
    if (s.state === 'done') return s;
    if (s.state === 'failed') throw Error(s.error.message);
    if (s.state === 'uploading') { if (++resends > 5) throw finishError || Error('処理に失敗しました。'); await finish(); }  // finish never arrived
  }
}
$('#account-form').addEventListener('submit', e => {
  e.preventDefault();
  action(e.target, async () => {
    const f = new FormData(e.target);
    await api('/api/accounts', { name: f.get('name'), password: f.get('password') }); e.target.reset(); notice('利用者を追加しました。');
  });
});
$('#stop').addEventListener('click', async () => {
  try {
    await api('/api/stop', {}); clearPrivate(); showOnly('gate'); $('#view-gate').hidden = true; $('#nav').hidden = true;
    const msg = platform === 'darwin' ? 'アプリを停止しました。RustXNG.appを開くと再起動できます。' : 'アプリを停止しました。このタブは閉じてかまいません。';
    notice(msg);
  } catch (e) { notice(e.message); }
});

/* ---------- AI guide ---------- */
const aiGuide = 'https://github.com/aoaibiz/fast-research の docs/INSTALL-AI.md を読み、この PC のあなた自身に MCP サーバー「fast-research」として入れてください。今ある設定は消さずに足してください。入れたら公開情報の検索を 1 回して、出典つきで答えられるか確かめてください。';
for (const b of $$('[data-copy-guide]')) b.addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(aiGuide); notice('導入案内をコピーしました。担当のAIの会話に貼り付けてください。'); }
  catch { notice('コピーできませんでした。GitHubの導入手順を開き、そのURLを担当のAIに渡してください。'); }
});
for (const b of $$('#nav button')) b.addEventListener('click', () => showView(b.dataset.view).then(() => notice('')).catch(e => notice(e.message)));
for (const b of $$('[data-open-view]')) b.addEventListener('click', () => showView(b.dataset.openView).catch(e => notice(e.message)));

window.addEventListener('pageshow', e => { if (e.persisted) { clearPrivate(); init().catch(x => notice(x.message)); } });
// A one-time login link (#link=...): the page posts the token itself, then reloads its own address so the Strict cookie is sent.
async function start() {
  if (location.hash.startsWith('#link=')) {
    const token = location.hash.slice(6);
    history.replaceState(null, '', location.pathname + location.search);
    try { await api('/api/link-login', { token }); location.replace('/'); return; } catch (e) { notice(e.message); }
  }
  await init();
}
start().catch(e => notice(e.message));
window.addEventListener('hashchange', () => { if (location.hash.startsWith('#link=')) start().catch(e => notice(e.message)); });
