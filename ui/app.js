'use strict';
/* Debridr UI. No build step. Talks to Rust through Tauri commands;
   in a plain browser it falls back to mock.js for design work. */
const T = window.__TAURI__;
const invoke = (c, a) => (T ? T.core.invoke(c, a) : window.__MOCK__.invoke(c, a));
const listen = (e, f) => (T ? T.event.listen(e, f) : window.__MOCK__.listen(e, f));
async function pickFolders() {
  if (!T) return [await window.__MOCK__.pickFolder()];
  const r = await T.dialog.open({ directory: true, multiple: true, title: 'Выберите папку' });
  return r ? [].concat(r) : [];
}

/* ---------- helpers ---------- */
const $ = (s) => document.querySelector(s);
const esc = (s) => String(s ?? '').replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
const nf = (n, d = 1) => n.toLocaleString('ru-RU', { maximumFractionDigits: d });
const sz = (b) => (b >= 1099511627776 ? nf(b / 1099511627776, 2) + ' ТБ' : b >= 1073741824 ? nf(b / 1073741824, b >= 1e10 ? 0 : 1) + ' ГБ' : b >= 1048576 ? nf(b / 1048576, 0) + ' МБ' : b >= 1024 ? nf(b / 1024, 0) + ' КБ' : b + ' Б');
const dt = (ts) => (ts ? new Date(ts * 1000).toLocaleDateString('ru-RU', { day: '2-digit', month: 'short', year: 'numeric' }) : '—');
const FMT = { vst3: 'VST3', vst2: 'VST2', aax: 'AAX', au: 'AU', clap: 'CLAP' };
const SUPPORT = { exact: 'Точный разбор', heuristic: 'По названиям', none: 'Проекты не читаются' };
const STATE = { used: ['ok', 'Нужен'], oldonly: ['warn', 'Только в старых'], unused: ['bad', 'Нигде'], projectowned: ['neu', 'Часть проекта'] };
const S = {
  boot: null, settings: null, report: null, view: 'plugins', step: 1, scanning: false, progress: null,
  filter: 'all', q: '', selP: null, selPr: null, goneP: new Set(),
  dir: null, kids: [], file: null, allowOld: false, onlyUnused: false,
  sel: new Map(), trash: [], dups: null, modal: null, dawName: {},
  hidden: 0, hist: [null], hpos: 0, loading: false, dupProg: null, menu: null, playing: null, cache: null,
  dock: null, waves: {}, wavePending: {},
};
const toSrc = (path) => {
  const native = S.boot && S.boot.platform === 'windows' ? path.replace(/\//g, '\\') : path;
  return T ? T.core.convertFileSrc(native) : '';
};

/* ---------- toast / modal ---------- */
let toastTimer;
function toast(text, action) {
  const t = $('#toast');
  t.innerHTML = `<span>${esc(text)}</span>${action ? `<button data-act="toastAction">${esc(action.label)}</button>` : ''}`;
  toast.fn = action && action.fn;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => (t.hidden = true), action ? 8000 : 3200);
}

/* ---------- derived data ---------- */
function visiblePlugins() {
  return S.report.plugins.map((p) => {
    const formats = p.formats.filter((f) => !S.goneP.has(f.path));
    if (!formats.length) return null;
    const recommended = p.recommended.filter((x) => !S.goneP.has(x));
    let status = p.status;
    if (status === 'duplicate' && !recommended.length) status = 'used';
    return { ...p, formats, recommended, status, total_size: formats.reduce((a, f) => a + f.size, 0),
      recommended_size: formats.filter((f) => recommended.includes(f.path)).reduce((a, f) => a + f.size, 0) };
  }).filter(Boolean);
}
const selKey = (kind, key) => kind + '|' + key;
const selSize = () => [...S.sel.values()].reduce((a, i) => a + i.size, 0);

/* ---------- boot ---------- */
async function init() {
  S.boot = await invoke('boot');
  S.settings = S.boot.settings;
  S.boot.daws.forEach((d) => (S.dawName[d.daw] = d.name));
  listen('scan-progress', (e) => { S.progress = e.payload; if (S.scanning) paintProgress(); });
  listen('dup-progress', (e) => { S.dupProg = e.payload; paintDup(); });
  S.trash = await invoke('trash_list');
  if (S.boot.first_run) { S.view = 'wizard'; }
  else { await startScan(); return; }
  render();
}

async function startScan() {
  S.scanning = true; S.progress = null; S.view = S.view === 'wizard' ? 'wizard' : S.view;
  render();
  try {
    await invoke('save_settings', { settings: S.settings });
    S.report = await invoke('run_scan', { settings: S.settings });
    S.trash = await invoke('trash_list');
    S.goneP = new Set(); S.sel.clear(); S.dups = null; S.dir = null; S.file = null; S.selP = null; S.selPr = null;
    S.scanning = false;
    S.view = 'plugins';
    const vp = visiblePlugins();
    S.selP = (vp.find((p) => p.status !== 'used') || vp[0] || {}).name || null;
    S.selPr = (S.report.projects[0] || {}).id ?? null;
    try { await loadKids(); } catch (e) { S.kids = []; toast('Не удалось открыть папки сэмплов: ' + e); }
  } catch (e) {
    S.scanning = false;
    S.error = String(e);
  }
  render();
}

async function loadKids() {
  S.loading = true;
  try { const l = await invoke('sample_children', { dir: S.dir }); S.kids = l.items; S.hidden = l.hidden_files; }
  finally { S.loading = false; }
}
async function goDir(dir, push = true) {
  if (push) { S.hist = S.hist.slice(0, S.hpos + 1); S.hist.push(dir); S.hpos = S.hist.length - 1; }
  S.dir = dir; S.file = null; S.loading = true; render();
  await loadKids(); render();
}

/* ---------- rendering ---------- */
function render() {
  const root = $('#root');
  if (S.scanning) { root.innerHTML = scanScreen(); paintProgress(); return; }
  if (S.error) { root.innerHTML = `<div class="wiz"><div class="wiz-box"><h1>Скан не удался</h1><div class="note bad" style="white-space:pre-wrap;user-select:text">${esc(S.error)}</div><p style="display:flex;gap:8px"><button class="btn pri" data-act="retry">Назад к папкам</button><button class="btn" data-act="copyErr">Скопировать текст ошибки</button></p></div></div>`; return; }
  if (S.view === 'wizard') { root.innerHTML = wizard(); return; }
  if (!S.report) { root.innerHTML = ''; return; }
  const views = { plugins: [vPlugins, sPlugin], projects: [vProjects, sProject], samples: [vSamples, sSample], dups: [vDups, sDups], trash: [vTrash, sTrash], settings: [vSettings, sSettings] };
  const [main, side] = views[S.view];
  const keepScroll = $('.list') ? $('.list').scrollTop : 0;
  root.innerHTML = `<div class="app ${S.dock ? 'has-dock' : ''}">${nav()}<main class="card main">${main()}${actionBar()}</main><aside class="card side">${side()}</aside>${dock()}</div>${S.menu ? menu() : ''}${S.modal ? modal() : ''}`;
  const l = $('.list'); if (l) l.scrollTop = keepScroll;
  paintWaves();
}

function nav() {
  const items = [['plugins', 'Плагины'], ['projects', 'Проекты'], ['samples', 'Сэмплы'], ['dups', 'Дубликаты'], ['trash', 'Корзина'], ['settings', 'Настройки']];
  return `<nav class="card nav"><div class="brand">Debridr<small>${S.settings.daws.map((d) => S.dawName[d]).join(' · ')}</small></div>` +
    items.map(([k, l]) => `<button data-act="view" data-v="${k}" aria-current="${S.view === k}">${l}${k === 'trash' && S.trash.length ? `<span class="badge">${S.trash.length}</span>` : ''}</button>`).join('') +
    `<div class="sp"></div><div class="mute" style="padding:8px 10px;font-size:11.5px">Скан: ${nf(S.report.timings.plugins_ms + S.report.timings.projects_ms + S.report.timings.samples_ms + S.report.timings.analysis_ms, 0)} мс<br>из кэша проектов: ${S.report.timings.cached_projects}</div></nav>`;
}

function head(title) {
  return `<div class="head"><h1>${title}</h1><div class="scan"><span>${S.report.projects.length} проектов · ${S.report.plugins.length} плагинов · ${nf(S.report.samples.files, 0)} файлов</span><button class="btn" data-act="rescan">Обновить скан</button></div></div>`;
}
function warnings() {
  return S.report.warnings.length && S.view === 'plugins' ? `<div class="warnbox">${S.report.warnings.map((w) => `<div class="note info">${esc(w)}</div>`).join('')}</div>` : '';
}

/* --- plugins --- */
function vPlugins() {
  const all = visiblePlugins();
  let list = all.filter((p) => p.name.toLowerCase().includes(S.q.toLowerCase()));
  if (S.filter !== 'all') list = list.filter((p) => p.status === S.filter);
  const unused = all.filter((p) => p.status === 'unused'), dup = all.filter((p) => p.status === 'duplicate');
  const free = all.reduce((a, p) => a + p.recommended_size, 0);
  const seg = [['all', 'Все'], ['unused', 'Не используются'], ['duplicate', 'Лишние форматы'], ['used', 'Используются']];
  return head('Плагины') + warnings() + `<div class="stats">
    <div class="stat"><b>${all.length}</b><span>плагинов</span></div>
    <div class="stat"><b>${unused.length}</b><span>нигде не используются</span></div>
    <div class="stat"><b>${dup.length}</b><span>с лишними форматами</span></div>
    <div class="stat"><b>${sz(free)}</b><span>можно освободить</span></div></div>
    <div class="tools"><input type="search" id="q" placeholder="Поиск плагина" value="${esc(S.q)}" aria-label="Поиск">
      <div class="seg">${seg.map(([k, l]) => `<button data-act="filter" data-v="${k}" aria-pressed="${S.filter === k}">${l}</button>`).join('')}</div>
      <button class="btn" style="margin-left:auto" data-act="selRec">Выбрать всё рекомендованное</button></div>
    <div class="list"><div class="row hd c-pl"><span></span><span>Плагин</span><span>Форматы · проектов</span><span class="num">Размер</span><span>Статус</span></div>` +
    (list.map((p) => {
      const st = { used: ['ok', 'Нужен'], duplicate: ['warn', 'Лишние форматы'], unused: ['bad', 'Не используется'] }[p.status];
      const n = p.formats.filter((f) => S.sel.has(selKey('plugin', f.path))).length;
      return `<div class="row c-pl" tabindex="0" data-act="selP" data-v="${esc(p.name)}" data-ctx="plugin" aria-selected="${S.selP === p.name}" ${p.image ? `data-img="${esc(toSrc(p.image))}"` : ''}>
        <span>${n ? `<span class="badge">${n}</span>` : ''}</span>
        <div class="name"><b>${esc(p.name)}</b>${p.image ? '<small>есть картинка</small>' : ''}</div>
        <div class="chips">${p.formats.map((f) => `<span class="chip ${f.used_by.length ? 'on' : ''}">${FMT[f.format]} · ${f.used_by.length}</span>`).join('')}</div>
        <div class="num mono">${sz(p.total_size)}</div><div><span class="pill ${st[0]}">${st[1]}</span></div></div>`;
    }).join('') || '<div class="empty">Ничего не найдено</div>') + '</div>';
}
function sPlugin() {
  const p = visiblePlugins().find((x) => x.name === S.selP);
  if (!p) return '<section class="empty">Выберите плагин слева</section>';
  const rec = p.recommended;
  return `${p.image ? `<img class="shot" src="${esc(toSrc(p.image))}" alt="${esc(p.name)}">` : ''}<section><h2>${esc(p.name)}</h2><div class="mute">Используется в проектах: ${p.used_projects}</div>
    <div style="display:flex;gap:6px;flex-wrap:wrap;margin-top:10px"><button class="btn sm" data-act="uninstallMenu" data-v="${esc(p.name)}">Деинсталлировать…</button><button class="btn sm" data-act="webSearch" data-v="${esc(p.name)}">${p.image ? 'Найти в интернете' : 'Как выглядит? Найти в интернете'}</button></div></section>
  <section><h3>Форматы на диске</h3>${p.formats.map((f) => {
    const k = selKey('plugin', f.path), on = S.sel.has(k);
    return `<div class="fmt" data-ctx="path" data-path="${esc(f.path)}"><div class="top"><b>${FMT[f.format]}</b><span class="mono mute">${sz(f.size)}</span></div>
      <div class="path">${esc(f.path)}</div><button class="btn sm" data-act="reveal" data-path="${esc(f.path)}" style="margin-bottom:6px">Показать в проводнике</button>
      ${f.used_by.length ? `<div class="mute" style="font-size:12px">Нужен в проектах (${f.used_by.length}):</div><ul>${f.used_by.slice(0, 8).map((u) => `<li>${esc(u.project)} <span class="mute">· ${esc(S.dawName[u.daw] || u.daw)}${u.exact ? '' : ', по названию'}</span></li>`).join('')}${f.used_by.length > 8 ? `<li>и ещё ${f.used_by.length - 8}</li>` : ''}</ul>` : ''}
      ${f.protected_by.length ? `<div class="note">${esc(f.reason)}</div>` : ''}
      ${f.removable ? `<div class="mute" style="font-size:12px;margin-bottom:6px">${esc(f.reason)}</div><button class="btn ${on ? '' : 'pri'} sm" data-act="togP" data-path="${esc(f.path)}" data-label="${esc(p.name + ' ' + FMT[f.format])}" data-size="${f.size}">${on ? 'Снять выбор' : 'Выбрать'}</button>` : '<button class="btn sm" disabled>Удалять нельзя</button>'}</div>`;
  }).join('')}</section>
  <section><h3>Рекомендация</h3>${rec.length
    ? `<div class="note info">${p.status === 'unused' ? 'Плагин не найден ни в одном проекте. ' : 'Оставить: ' + p.formats.filter((f) => !rec.includes(f.path)).map((f) => FMT[f.format]).join(', ') + '. '}Убрать: ${p.formats.filter((f) => rec.includes(f.path)).map((f) => FMT[f.format]).join(', ')} · ${sz(p.recommended_size)}.</div>
       <p style="margin:10px 0 0"><button class="btn pri" data-act="selPluginRec" data-v="${esc(p.name)}">Выбрать рекомендованное</button></p>`
    : '<div class="note ok">Здесь нечего убирать.</div>'}</section>`;
}

/* --- projects --- */
function vProjects() {
  const ps = S.report.projects;
  return head('Проекты') + `<div class="stats"><div class="stat"><b>${ps.length}</b><span>проектов</span></div><div class="stat"><b>${sz(ps.reduce((a, p) => a + p.size, 0))}</b><span>сами файлы проектов</span></div><div class="stat"><b>${ps.filter((p) => p.missing.length).length}</b><span>с потерянными файлами</span></div></div>
  <div class="list"><div class="row hd c-pr"><span>Проект</span><span>DAW</span><span>Изменён</span><span class="num">Плагинов</span><span class="num">Файлов</span><span class="num">Размер</span></div>` +
    (ps.map((p) => `<div class="row c-pr" tabindex="0" data-act="selPr" data-v="${p.id}" data-ctx="path" data-path="${esc(p.path)}" aria-selected="${S.selPr === p.id}">
      <div class="name"><b>${esc(p.name)}</b>${p.missing.length ? `<small style="color:var(--bad)">не найдено файлов: ${p.missing.length}</small>` : ''}</div>
      <span class="mute">${esc(S.dawName[p.daw] || p.daw)}</span><span class="mute">${dt(p.mtime)}</span><span class="num mono">${p.plugins.length}</span><span class="num mono">${p.file_refs}</span><span class="num mono">${sz(p.size)}</span></div>`).join('') || '<div class="empty">Проекты не найдены. Проверьте папки в настройках.</div>') + '</div>';
}
function sProject() {
  const p = S.report.projects.find((x) => x.id === S.selPr);
  if (!p) return '<section class="empty">Выберите проект</section>';
  const stl = { ok: '', format_missing: '<span class="pill warn">нет этого формата</span>', not_installed: '<span class="pill bad">не установлен</span>' };
  return `<section><h2>${esc(p.name)}</h2><div class="mute">${esc(S.dawName[p.daw] || p.daw)} · ${dt(p.mtime)}${p.old ? ' · старый' : ''}</div><div class="path">${esc(p.path)}</div><button class="btn sm" data-act="reveal" data-path="${esc(p.path)}">Показать в проводнике</button></section>
  <section><h3>Плагины (${p.plugins.length})</h3>${p.plugins.map((x) => `<div class="li"><span>${esc(x.name)}</span><span style="display:flex;gap:6px">${stl[x.state] || ''}${x.format ? `<span class="chip on">${FMT[x.format]}</span>` : '<span class="chip">формат ?</span>'}</span></div>`).join('') || '<span class="mute">Сторонних плагинов нет</span>'}</section>
  <section><h3>Целостность</h3>${p.missing.length ? `<div class="note bad">Не найдено файлов: ${p.missing.length}. Проект уже сейчас открывается с пропусками.</div><ul style="padding-left:16px;margin:8px 0 0;font:11px var(--mono);color:var(--mute);word-break:break-all">${p.missing.slice(0, 12).map((m) => `<li>${esc(m)}</li>`).join('')}</ul>` : '<div class="note ok">Все ссылки на файлы найдены.</div>'}</section>`;
}

/* --- samples --- */
function crumbs() {
  const parts = S.dir ? S.dir.split('/') : [];
  let acc = '';
  const out = [`<button data-act="goDir" data-v="">Все папки</button>`];
  parts.forEach((p, i) => { acc += (i ? '/' : '') + p; out.push(`<span class="mute">/</span><button data-act="goDir" data-v="${esc(acc)}">${esc(p)}</button>`); });
  return `<div class="crumbs"><button class="navb" data-act="histBack" ${S.hpos > 0 ? '' : 'disabled'} aria-label="Назад" title="Назад (Alt+←)">←</button><button class="navb" data-act="histFwd" ${S.hpos < S.hist.length - 1 ? '' : 'disabled'} aria-label="Вперёд" title="Вперёд (Alt+→)">→</button>${out.join('')}${S.loading ? '<span class="mute" style="margin-left:8px">загружаю…</span>' : ''}</div>`;
}
function vSamples() {
  const s = S.report.samples;
  let kids = S.kids;
  if (S.onlyUnused) kids = kids.filter((k) => (k.is_dir ? k.unused_bytes > 0 : k.state === 'unused'));
  const here = S.kids.reduce((a, k) => a + k.unused_bytes + (k.state === 'unused' && !k.is_dir ? k.bytes : 0), 0);
  return head('Сэмплы') + `<div class="stats"><div class="stat"><b>${sz(s.bytes)}</b><span>${nf(s.files, 0)} файлов</span></div><div class="stat"><b>${sz(s.used_bytes)}</b><span>нужны проектам</span></div><div class="stat"><b>${sz(s.old_bytes)}</b><span>только в старых проектах</span></div><div class="stat"><b>${sz(s.unused_bytes)}</b><span>нигде не используются</span></div></div>
  ${crumbs()}<div class="tools"><button class="btn ${S.onlyUnused ? 'pri' : ''}" data-act="onlyUnused">Только неиспользуемое</button>
    <label class="mute" style="display:flex;gap:6px;align-items:center"><input type="checkbox" data-act="allowOld" ${S.allowOld ? 'checked' : ''}> Разрешить файлы из старых проектов</label>
    <button class="btn" style="margin-left:auto" data-act="selUnusedHere" ${here ? '' : 'disabled'}>Выбрать неиспользуемое здесь (${sz(here)})</button></div>
  <div class="list"><div class="row hd c-sa"><span></span><span>Имя</span><span>Используется / нет</span><span class="num">Файлов</span><span class="num">Размер</span></div>` +
    (kids.map((k) => {
      const checkable = !k.is_dir && (k.state === 'unused' || (k.state === 'oldonly' && S.allowOld));
      const on = !k.is_dir && S.sel.has(selKey('sample', k.idx));
      const total = k.bytes || 1;
      const st = !k.is_dir ? STATE[k.state] : null;
      const playing = !k.is_dir && S.playing === k.path;
      return `<div class="row c-sa" tabindex="0" data-act="${k.is_dir ? 'goDir' : 'selFile'}" data-v="${k.is_dir ? esc(k.path) : k.idx}" data-ctx="${k.is_dir ? 'dir' : 'sample'}" data-path="${esc(k.path)}" data-idx="${k.idx ?? ''}" data-name="${esc(k.name)}" data-size="${k.bytes}" data-state="${k.state || ''}" aria-selected="${!k.is_dir && S.file && S.file.idx === k.idx}">
        <span>${checkable ? `<input type="checkbox" data-act="togS" data-idx="${k.idx}" data-label="${esc(k.name)}" data-size="${k.bytes}" ${on ? 'checked' : ''}>` : ''}</span>
        <div class="name" style="display:flex;gap:8px;align-items:center">${k.is_dir ? '<span class="mute">▸</span>' : `<button class="play ${playing ? 'on' : ''}" data-act="play" data-path="${esc(k.path)}" data-name="${esc(k.name)}" aria-label="Прослушать">${playing ? '❚❚' : '▶'}</button>`}<b>${esc(k.name)}</b></div>
        <div>${k.is_dir ? `<span class="mono" style="font-size:12px">${sz(k.used_bytes + k.owned_bytes + k.old_bytes)} из ${sz(k.bytes)}</span><div class="split"><i style="width:${(k.used_bytes + k.owned_bytes) / total * 100}%;background:var(--ok)"></i><i style="width:${k.old_bytes / total * 100}%;background:var(--warn)"></i><i style="width:${k.unused_bytes / total * 100}%;background:var(--bad)"></i></div>` : `<span class="pill ${st[0]}">${st[1]}</span>`}</div>
        <span class="num mono">${k.is_dir ? nf(k.files, 0) : ''}</span><span class="num mono">${sz(k.bytes)}</span></div>`;
    }).join('') || `<div class="empty">${S.loading ? 'Загружаю…' : 'Здесь пусто'}</div>`) + (S.hidden ? `<div class="empty" style="padding:14px">И ещё ${nf(S.hidden, 0)} мелких файлов в этой папке. Показаны самые крупные.</div>` : '') + '</div>';
}
function dock() {
  if (!S.dock) return '';
  const a = $('#audio'), on = S.playing === S.dock && a && !a.paused;
  const name = S.dock.split('/').pop();
  return `<div class="card dock"><button class="bigplay" data-act="dockPlay" aria-label="${on ? 'Пауза' : 'Играть'}">${on ? '❚❚' : '▶'}</button>
    <div class="dock-info"><b title="${esc(S.dock)}">${esc(name)}</b><span class="mono mute" id="ptime">${waveMeta(S.dock)}</span></div>
    <div class="dock-wave" data-act="dockSeek"><canvas data-wave="${esc(S.dock)}" data-b="1600" data-dock="1"></canvas></div>
    <button class="x" data-act="dockClose" aria-label="Закрыть плеер" title="Закрыть">✕</button></div>`;
}
function waveMeta(path) {
  const w = S.waves['1600|' + path];
  if (!w) return S.wavePending['1600|' + path] ? 'строю волну…' : '';
  const f = (x) => Math.floor(x / 60) + ':' + String(Math.floor(x % 60)).padStart(2, '0');
  return `${f(w.duration)} · ${nf(w.sample_rate / 1000, 1)} кГц · ${w.channels === 1 ? 'моно' : w.channels === 2 ? 'стерео' : w.channels + ' кан.'}`;
}
function cssVar(n) { return getComputedStyle(document.documentElement).getPropertyValue(n).trim(); }
function drawWave(c, w, progress) {
  const dpr = window.devicePixelRatio || 1, W = c.clientWidth, H = c.clientHeight;
  if (!W || !H) return;
  c.width = W * dpr; c.height = H * dpr;
  const g = c.getContext('2d'); g.scale(dpr, dpr); g.clearRect(0, 0, W, H);
  if (w && w.error) { g.fillStyle = cssVar('--mute'); g.font = '11px sans-serif'; g.fillText(w.error, 6, H / 2 + 4); return; }
  if (!w) { g.fillStyle = cssVar('--line'); g.fillRect(0, H / 2, W, 1); return; }
  const n = w.peaks.length / 2, mid = H / 2, played = cssVar('--accent'), rest = cssVar('--mute');
  for (let x = 0; x < W; x++) {
    const i = Math.floor(x / W * n), lo = w.peaks[i * 2], hi = w.peaks[i * 2 + 1];
    g.fillStyle = progress != null && x / W <= progress ? played : rest;
    const y1 = mid - hi * mid * 0.95, y2 = mid - lo * mid * 0.95;
    g.fillRect(x, y1, 1, Math.max(1, y2 - y1));
  }
  if (progress != null && progress > 0) { g.fillStyle = cssVar('--fg'); g.fillRect(Math.round(progress * W), 0, 1.5, H); }
}
function progressOf(path) { const a = $('#audio'); return S.playing === path && a && a.duration ? a.currentTime / a.duration : null; }
function paintWaves() {
  document.querySelectorAll('canvas[data-wave]').forEach((c) => {
    const path = c.dataset.wave, b = +c.dataset.b, key = b + '|' + path;
    const w = S.waves[key];
    drawWave(c, w, progressOf(path) ?? (c.dataset.dock ? 0 : null));
    if (w || S.wavePending[key]) return;
    S.wavePending[key] = true;
    invoke('wave', { path, buckets: b }).then((res) => { S.waves[key] = res; }, (e) => { S.waves[key] = { error: String(e).slice(0, 80) }; })
      .finally(() => { delete S.wavePending[key]; document.querySelectorAll(`canvas[data-wave]`).forEach((cc) => { if (cc.dataset.wave === path && +cc.dataset.b === b) drawWave(cc, S.waves[key], progressOf(path)); }); const m = $('#ptime'); if (m && S.dock === path) m.textContent = waveMeta(path); });
  });
}
function sSample() {
  const f = S.file;
  const s = S.report.samples;
  if (!f) return `<section><h2>Как читать цвета</h2></section><section>
    <div class="li"><span class="pill ok">Нужен</span><span class="mute">используется недавним проектом</span></div>
    <div class="li"><span class="pill warn">Только в старых</span><span class="mute">только проекты старше ${S.settings.old_days} дн.</span></div>
    <div class="li"><span class="pill bad">Нигде</span><span class="mute">ни одной ссылки в проектах</span></div>
    <div class="li"><span class="pill neu">Часть проекта</span><span class="mute">лежит в папке проекта</span></div></section>
    <section><h3>Итого по библиотеке</h3><div class="note info">Можно убрать до ${sz(s.unused_bytes)} (${nf(s.unused_files, 0)} файлов). Всё идёт в Корзину программы, потом можно вернуть.</div></section>`;
  const st = STATE[f.state];
  return `<section><h2>${esc(f.name)}</h2><div class="path">${esc(f.path)}</div><div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap"><span class="pill ${st[0]}">${st[1]}</span><span class="mono mute">${sz(f.size)}</span><button class="btn sm" data-act="play" data-path="${esc(f.path)}" data-name="${esc(f.name)}">${S.playing === f.path ? 'Пауза' : 'Прослушать'}</button><button class="btn sm" data-act="reveal" data-path="${esc(f.path)}">Показать в проводнике</button></div></section>
  <section><h3>Используется в проектах</h3>${f.used_by.length ? f.used_by.map((n) => `<div class="li"><span>${esc(n)}</span></div>`).join('') : '<span class="mute">Ни в одном</span>'}
  ${f.by_name ? '<div class="note">Найден по имени файла: путь в проекте указывает на другое место. Файл оставлен на всякий случай.</div>' : ''}
  ${f.state === 'oldonly' ? '<div class="note">Используется только в проектах старше года. Включите «Разрешить файлы из старых проектов», если уверены.</div>' : ''}
  ${f.state === 'projectowned' ? '<div class="note info">Файл лежит в папке проекта. Программа его не трогает.</div>' : ''}</section>`;
}

/* --- duplicates --- */
function vDups() {
  const d = S.dups;
  const reclaim = d ? d.reduce((a, [g, files]) => a + g.size * Math.max(0, files.filter((f) => f.state === 'unused').length - (files.some((f) => f.state !== 'unused') ? 0 : 1)), 0) : 0;
  return head('Дубликаты') + `<div class="tools">${S.dupBusy ? '<button class="btn danger" data-act="cancelDups">Остановить</button>' : '<button class="btn pri" data-act="findDups">Найти дубликаты сэмплов</button>'}<span class="mute">Ищутся точные копии файлов от 1 МБ. Содержимое читается только у файлов одинакового размера.</span></div>
    ${S.dupBusy ? `<div style="padding:12px 16px;border-bottom:1px solid var(--line)"><div class="prog" style="margin:0 0 6px"><i id="dbar" style="width:${dupPct()}%"></i></div><div class="mute" id="dtext">${esc(dupText())}</div></div>` : ''}` +
    (d ? `<div class="stats"><div class="stat"><b>${d.length}</b><span>групп копий</span></div><div class="stat"><b>${sz(reclaim)}</b><span>можно освободить</span></div></div>` : '') +
    `<div class="list">${d ? (d.map(([g, files], gi) => `<div style="padding:10px 16px;border-bottom:1px solid var(--line)"><div style="display:flex;justify-content:space-between;margin-bottom:6px"><b>${esc(files[0].name)} <span class="mute">× ${files.length}</span></b><span class="mono mute">${sz(g.size)} каждая</span></div>
      ${files.map((f) => { const on = S.sel.has(selKey('sample', f.idx)); const ok = f.state === 'unused'; const st = STATE[f.state];
        return `<div class="li dupli" data-ctx="sample" data-path="${esc(f.path)}" data-idx="${f.idx}" data-name="${esc(f.name)}" data-size="${f.size}" data-state="${f.state}"><span style="display:flex;gap:8px;align-items:center;min-width:0"><input type="checkbox" data-act="togS" data-idx="${f.idx}" data-label="${esc(f.name)}" data-size="${f.size}" ${on ? 'checked' : ''} ${ok ? '' : 'disabled'}><button class="play ${S.playing === f.path ? 'on' : ''}" data-act="play" data-path="${esc(f.path)}" data-name="${esc(f.name)}" aria-label="Прослушать">${S.playing === f.path ? '❚❚' : '▶'}</button><span class="path" style="margin:0">${esc(f.path)}</span></span><canvas class="mini" data-wave="${esc(f.path)}" data-b="300" data-act="play" data-path="${esc(f.path)}"></canvas><span class="pill ${st[0]}">${st[1]}</span></div>`; }).join('')}
      <button class="btn sm" style="margin-top:6px" data-act="selDupGroup" data-v="${gi}">Выбрать лишние копии</button></div>`).join('') || '<div class="empty">Точных копий не найдено</div>') : `<div class="empty">${S.dupBusy ? 'Идёт поиск. Можно переключаться на другие разделы.' : 'Нажмите «Найти дубликаты сэмплов»'}</div>`}</div>`;
}
function dupPct() {
  const p = S.dupProg; if (!p) return 2;
  if (p.stage === 'quick') return 2 + (p.total_files ? p.done_files / p.total_files * 38 : 38);
  return 40 + (p.total_bytes ? p.done_bytes / p.total_bytes * 60 : 60);
}
function dupText() {
  const p = S.dupProg; if (!p) return 'Подготовка…';
  if (p.stage === 'quick') return `Шаг 1 из 2. Быстрая проверка: ${nf(p.done_files, 0)} из ${nf(p.total_files, 0)} файлов одинакового размера`;
  return `Шаг 2 из 2. Сравниваю содержимое: ${sz(p.done_bytes)} из ${sz(p.total_bytes)} (${nf(p.done_files, 0)} из ${nf(p.total_files, 0)} файлов)`;
}
function paintDup() { const b = $('#dbar'), t = $('#dtext'); if (b) b.style.width = dupPct() + '%'; if (t) t.textContent = dupText(); }
function sDups() {
  return `<section><h2>Дубликаты</h2></section><section><div class="note info">Если одну из копий использует проект, она останется. Выбрать для удаления можно только копии, на которые не ссылается ни один проект.</div></section>`;
}

/* --- trash --- */
function vTrash() {
  const t = S.trash, total = t.reduce((a, i) => a + i.size, 0);
  return head('Корзина') + `<div class="stats"><div class="stat"><b>${t.length}</b><span>элементов</span></div><div class="stat"><b>${sz(total)}</b><span>занимает место, пока не очистите</span></div></div>
  <div class="tools"><span class="mute">Здесь лежит то, что вы убрали. Диск освободится, когда вы очистите корзину.</span>
    <button class="btn danger" style="margin-left:auto" data-act="emptyTrash" ${t.length ? '' : 'disabled'}>Очистить корзину</button></div>
  <div class="list"><div class="row hd c-tr"><span></span><span>Что</span><span>Тип</span><span class="num">Размер</span><span></span></div>` +
    (t.map((i) => `<div class="row c-tr" style="cursor:default" data-ctx="trash" data-id="${i.id}" data-path="${esc(i.original_path)}"><span></span><div class="name"><b>${esc(i.label)}</b><small>${esc(i.original_path)}</small></div><span class="mute">${i.kind === 'plugin' ? 'Плагин' : 'Сэмпл'}</span><span class="num mono">${sz(i.size)}</span>
      <span style="display:flex;gap:6px;justify-content:flex-end"><button class="btn sm" data-act="restore" data-v="${i.id}">Вернуть</button><button class="btn sm danger" data-act="purgeOne" data-v="${i.id}">Удалить</button></span></div>`).join('') || '<div class="empty">Корзина пуста</div>') + '</div>';
}
function sTrash() {
  return `<section><h2>Как работает корзина</h2></section><section>
    <div class="check note info">Файлы не удаляются, а переезжают в скрытую папку на том же диске. Это мгновенно, даже для больших библиотек.</div>
    <div class="note info" style="margin-top:8px">«Вернуть» кладёт файл ровно на прежнее место. Если там уже что-то лежит, программа ничего не перезапишет.</div>
    <div class="note" style="margin-top:8px">«Удалить» и «Очистить корзину» стирают файлы окончательно и не отменяются.</div></section>`;
}

/* --- settings --- */
function foldList(key, title, hint) {
  const arr = S.settings[key];
  return `<div class="fold"><h3>${title}</h3><p>${hint}</p><div class="items">${arr.map((p, i) => `<div class="it"><span>${esc(p)}</span><button data-act="rmDir" data-key="${key}" data-i="${i}" aria-label="Убрать">✕</button></div>`).join('')}</div><button class="btn sm" data-act="addDir" data-key="${key}">+ Добавить папку</button></div>`;
}
function vSettings() {
  const st = S.settings;
  return head('Настройки') + `<div class="list" style="padding:16px"><div class="set" style="max-width:640px">
    <h3 style="margin:0 0 6px">DAW</h3><div class="dawgrid" style="margin:8px 0 18px">${S.boot.daws.map((d) => `<button class="daw" data-act="togDaw" data-v="${d.daw}" aria-pressed="${st.daws.includes(d.daw)}"><b>${esc(d.name)}${d.installed ? '<span class="pill ok">найдена</span>' : ''}</b><small>${SUPPORT[d.support]}</small></button>`).join('')}</div>
    <div class="folders">${foldList('project_dirs', 'Папки с проектами', 'Всё, что программа будет проверять как «используется».')}${foldList('sample_dirs', 'Библиотеки сэмплов', 'Эти папки программа просматривает на неиспользуемые файлы.')}${foldList('extra_plugin_dirs', 'Дополнительные папки плагинов', 'Стандартные папки VST3, VST2, AAX, CLAP и AU находятся сами.')}${st.daws.includes('ableton') ? foldList('library_dirs', 'Ableton User Library', 'Рэки и пресеты тоже используют плагины.') : ''}</div>
    <label>Основной формат<select data-act="setPref">${['vst3', 'vst2', 'au', 'clap'].map((f) => `<option value="${f}" ${st.preferred_format === f ? 'selected' : ''}>${FMT[f]}</option>`).join('')}</select></label>
    <label>Проект считается старым после, дней<input type="number" min="30" step="30" value="${st.old_days}" data-act="setOld" style="width:90px"></label>
    <label>Для DAW с разбором по названиям<select data-act="setPolicy"><option value="safe" ${st.unknown_policy === 'safe' ? 'selected' : ''}>Безопасно: беречь все форматы</option><option value="balanced" ${st.unknown_policy === 'balanced' ? 'selected' : ''}>Сбалансированно: беречь основной формат</option></select></label>
    <label>Читать резервные копии Ableton (Backup)<input type="checkbox" data-act="setBackups" ${st.include_backups ? 'checked' : ''}></label>
    <label>Строгая защита: беречь файлы с тем же именем, что в проектах<input type="checkbox" data-act="setStrict" ${st.strict_names !== false ? 'checked' : ''}></label>
    <p><button class="btn pri" data-act="rescan">Сохранить и пересканировать</button></p>
    <h3 style="margin:26px 0 6px">Кэш программы</h3>
    <p class="mute" style="margin:0 0 8px">Кэш ускоряет повторный скан и хранит журнал ошибок. Настройки, папки и корзина при очистке остаются.</p>
    <div style="display:flex;gap:10px;align-items:center"><span class="mono">${S.cache ? sz(S.cache.bytes) : '…'}</span><button class="btn" data-act="clearCache" ${S.cache && S.cache.bytes ? '' : 'disabled'}>Очистить кэш</button></div>
    </div></div>`;
}
function sSettings() {
  return `<section><h2>Безопасность</h2></section><section><div class="note info">Если сомневаетесь, программа оставляет файл. Любой формат плагина, который может быть нужен проекту, помечается как «нужен».</div>
  <div class="note" style="margin-top:8px">Удалять можно только то, что не нашлось ни в одном проекте из выбранных папок. Если часть проектов лежит вне этих папок, добавьте их.</div></section>`;
}

/* --- context menu --- */
function menu() {
  const m = S.menu;
  return `<div class="menu" style="left:${m.x}px;top:${m.y}px" role="menu">${m.items.map((it) => it === '-' ? '<hr>' : `<button role="menuitem" data-act="menuPick" data-i="${m.items.indexOf(it)}" ${it.disabled ? 'disabled' : ''} class="${it.danger ? 'danger' : ''}">${esc(it.label)}${it.hint ? `<small>${esc(it.hint)}</small>` : ''}</button>`).join('')}</div>`;
}
function openMenu(e, items) {
  S.menu = { x: Math.min(e.clientX, innerWidth - 270), y: Math.min(e.clientY, innerHeight - 40 - items.length * 34), items };
  render();
}
function copyText(t) { navigator.clipboard.writeText(t).then(() => toast('Путь скопирован'), () => toast(t)); }
async function reveal(path) { try { await invoke('reveal_path', { path }); } catch (e) { toast('Не удалось открыть: ' + e); } }
async function uninstallFor(name) {
  const p = S.report.plugins.find((x) => x.name === name);
  toast('Ищу деинсталлятор…');
  const list = await invoke('plugin_uninstallers', { name, paths: p ? p.formats.map((f) => f.path) : [] });
  $('#toast').hidden = true;
  if (!list.length) {
    S.modal = { title: 'Деинсталлятор не найден', ok: 'Понятно', body: `<p>Для «${esc(name)}» в списке программ Windows нет подходящей записи. Плагин можно убрать через корзину этой программы или через установщик производителя.</p>`, fn: async () => {} };
    render(); return;
  }
  S.modal = { title: 'Деинсталлировать ' + name, ok: 'Отмена', body: `<p class="mute" style="margin-top:0">Найдено в «Приложениях» Windows. Запустится программа удаления от производителя. Проверьте название, прежде чем нажать.</p>
    ${list.map((u) => `<div class="li"><span><b>${esc(u.name)}</b><br><small class="mute">${esc([u.publisher, u.version].filter(Boolean).join(' · '))}</small></span><button class="btn sm danger" data-act="runUninst" data-v="${esc(u.id)}">Запустить</button></div>`).join('')}
    <p class="mute" style="font-size:12px">После удаления нажмите «Обновить скан».</p>`, fn: async () => {} };
  render();
}
async function playPath(path) {
  const a = $('#audio');
  S.dock = path;
  if (S.playing === path && !a.paused) { a.pause(); render(); return; }
  if (S.playing !== path) {
    if (!T) { toast('Прослушивание работает в приложении'); return; }
    a.src = toSrc(path); S.playing = path;
  }
  try { await a.play(); } catch (e) { const ext = path.split('.').pop().toLowerCase(); toast(['aif', 'aiff', 'rex', 'rx2', 'caf', 'w64'].includes(ext) ? `Формат .${ext} не воспроизводится встроенным плеером` : 'Не удалось воспроизвести файл'); }
  render();
}

/* --- action bar / modal --- */
function actionBar() {
  if (!S.sel.size) return '';
  return `<div class="bar"><span><b>${S.sel.size}</b> выбрано · ${sz(selSize())}</span><button class="btn" data-act="clearSel">Снять выбор</button><button class="btn pri" data-act="confirmTrash">В корзину</button></div>`;
}
function modal() {
  const m = S.modal;
  return `<div class="modal-bg" data-act="closeModal"><div class="modal" data-stop="1"><h2>${esc(m.title)}</h2><div>${m.body}</div><div class="acts"><button class="btn" data-act="closeModal">Отмена</button><button class="btn ${m.danger ? 'danger' : 'pri'}" data-act="modalOk">${esc(m.ok)}</button></div></div></div>`;
}

/* --- wizard & scan --- */
function wizard() {
  const st = S.settings, step = S.step;
  const dots = `<div class="steps">${[1, 2].map((i) => `<i class="${i <= step ? 'on' : ''}"></i>`).join('')}</div>`;
  if (step === 1) return `<div class="wiz"><div class="wiz-box">${dots}<h1>В чём вы работаете?</h1><p class="mute">Отметьте DAW. Плагин нужен, если он используется в проектах любой из них.</p>
    <div class="dawgrid">${S.boot.daws.map((d) => `<button class="daw" data-act="togDaw" data-v="${d.daw}" aria-pressed="${st.daws.includes(d.daw)}"><b>${esc(d.name)}${d.installed ? '<span class="pill ok">найдена</span>' : ''}</b><small>${SUPPORT[d.support]}</small></button>`).join('')}</div>
    <div class="acts"><span></span><button class="btn pri" data-act="step2" ${st.daws.length ? '' : 'disabled'}>Дальше</button></div></div></div>`;
  return `<div class="wiz"><div class="wiz-box">${dots}<h1>Где всё лежит?</h1><p class="mute">Программа только читает эти папки. Ничего не удаляется без вашего подтверждения.</p>
    <div class="folders">${foldList('project_dirs', 'Папки с проектами', 'Укажите все места, где лежат проекты, в том числе архив.')}${foldList('sample_dirs', 'Библиотеки сэмплов', 'Можно несколько папок и дисков. Терабайт просматривается за секунды.')}${foldList('extra_plugin_dirs', 'Дополнительные папки плагинов (необязательно)', 'Стандартные папки находятся сами.')}${st.daws.includes('ableton') ? foldList('library_dirs', 'Ableton User Library (необязательно)', 'Рэки и пресеты тоже используют плагины.') : ''}</div>
    <div class="acts"><button class="btn" data-act="step1">Назад</button><button class="btn pri" data-act="rescan" ${st.project_dirs.length ? '' : 'disabled'}>Сканировать</button></div></div></div>`;
}
function scanScreen() {
  return `<div class="wiz"><div class="wiz-box"><h1>Сканирую…</h1><div class="prog"><i id="pbar" style="width:4%"></i></div><div class="mute" id="ptext">Запускаю</div></div></div>`;
}
function paintProgress() {
  const p = S.progress, bar = $('#pbar'), txt = $('#ptext');
  if (!bar || !p) return;
  const text = { plugins: 'Ищу установленные плагины', projects: `Читаю проекты: ${nf(p.done, 0)} из ${nf(p.total, 0)}`, samples: `Просматриваю сэмплы: ${nf(p.done, 0)} файлов`, analysis: 'Сопоставляю найденное' }[p.stage];
  txt.textContent = text;
  bar.style.width = ({ plugins: 8, projects: 10 + (p.total ? p.done / p.total * 40 : 0), samples: 55 + Math.min(35, p.done / 20000), analysis: 95 }[p.stage]) + '%';
}

/* ---------- actions ---------- */
function addSel(item) { S.sel.set(selKey(item.kind, item.key), item); }
function togSel(item) { const k = selKey(item.kind, item.key); S.sel.has(k) ? S.sel.delete(k) : S.sel.set(k, item); }

const A = {
  view: (el) => {
    S.view = el.dataset.v; render();
    if (S.view === 'samples') loadKids().then(render);
    if (S.view === 'settings') invoke('cache_info').then((c) => { S.cache = c; render(); }).catch(() => {});
  },
  filter: (el) => { S.filter = el.dataset.v; render(); },
  selP: (el) => { S.selP = el.dataset.v; render(); },
  selPr: (el) => { S.selPr = +el.dataset.v; render(); },
  togP: (el) => { togSel({ kind: 'plugin', key: el.dataset.path, label: el.dataset.label, size: +el.dataset.size }); render(); },
  selPluginRec: (el) => { const p = visiblePlugins().find((x) => x.name === el.dataset.v); p.formats.filter((f) => p.recommended.includes(f.path)).forEach((f) => addSel({ kind: 'plugin', key: f.path, label: p.name + ' ' + FMT[f.format], size: f.size })); render(); },
  selRec: () => { visiblePlugins().forEach((p) => p.formats.filter((f) => p.recommended.includes(f.path)).forEach((f) => addSel({ kind: 'plugin', key: f.path, label: p.name + ' ' + FMT[f.format], size: f.size }))); render(); toast(`Выбрано: ${S.sel.size}. Проверьте и нажмите «В корзину».`); },
  goDir: (el) => goDir(el.dataset.v || null),
  histBack: () => { if (S.hpos > 0) { S.hpos--; goDir(S.hist[S.hpos], false); } },
  histFwd: () => { if (S.hpos < S.hist.length - 1) { S.hpos++; goDir(S.hist[S.hpos], false); } },
  play: (el, e) => { e.stopPropagation(); playPath(el.dataset.path); },
  playToggle: () => { const a = $('#audio'); a.paused ? a.play() : a.pause(); render(); },
  playStop: () => { const a = $('#audio'); a.pause(); a.removeAttribute('src'); S.playing = null; render(); },
  seek: (el, e) => { const a = $('#audio'); const r = el.getBoundingClientRect(); if (a.duration) a.currentTime = (e.clientX - r.left) / r.width * a.duration; },
  reveal: (el, e) => { e.stopPropagation(); reveal(el.dataset.path); },
  webSearch: (el) => invoke('web_search', { query: el.dataset.v + ' plugin' }).catch((e) => toast(String(e))),
  uninstallMenu: (el) => uninstallFor(el.dataset.v),
  runUninst: async (el) => { S.modal = null; render(); try { await invoke('run_uninstaller', { id: el.dataset.v }); toast('Деинсталлятор запущен. После удаления обновите скан.'); } catch (e) { toast(String(e)); } },
  menuPick: (el) => { const it = S.menu.items[+el.dataset.i]; S.menu = null; render(); Promise.resolve(it.fn()).catch((e) => toast(String(e))); },
  cancelDups: () => invoke('cancel_duplicates'),
  setStrict: (el) => { S.settings.strict_names = el.checked; },
  clearCache: () => {
    S.modal = { title: 'Очистить кэш?', ok: 'Очистить', danger: false, body: `<p>Будет удалено ${sz(S.cache.bytes)}: кэш разобранных проектов и журнал ошибок. Настройки, папки и корзина останутся. Следующий скан прочитает проекты заново и займёт больше времени.</p>`,
      fn: async () => { const n = await invoke('clear_cache'); S.cache = await invoke('cache_info'); render(); toast(`Освобождено ${sz(n)}`); } };
    render();
  },
  selFile: async (el) => { S.file = await invoke('sample_info', { idx: +el.dataset.v }); S.dock = S.file.path; render(); },
  dockPlay: () => playPath(S.dock),
  dockClose: () => { const a = $('#audio'); a.pause(); a.removeAttribute('src'); S.playing = null; S.dock = null; render(); },
  dockSeek: async (el, e) => {
    const r = el.getBoundingClientRect(), frac = Math.min(1, Math.max(0, (e.clientX - r.left) / r.width));
    const a = $('#audio');
    if (S.playing !== S.dock) { await playPath(S.dock); }
    const go = () => { if (a.duration) a.currentTime = frac * a.duration; };
    a.readyState >= 1 ? go() : a.addEventListener('loadedmetadata', go, { once: true });
  },
  onlyUnused: () => { S.onlyUnused = !S.onlyUnused; render(); },
  allowOld: (el) => { S.allowOld = el.checked; render(); },
  togS: (el, e) => { e.stopPropagation(); togSel({ kind: 'sample', key: String(el.dataset.idx), label: el.dataset.label, size: +el.dataset.size, allowOld: S.allowOld }); render(); },
  selUnusedHere: async () => {
    const files = await invoke('sample_top', { state: 'unused', under: S.dir, limit: 50000 });
    const old = S.allowOld ? await invoke('sample_top', { state: 'oldonly', under: S.dir, limit: 50000 }) : [];
    [...files, ...old].forEach((f) => addSel({ kind: 'sample', key: String(f.idx), label: f.name, size: f.size, allowOld: S.allowOld }));
    render(); toast(`Выбрано файлов: ${S.sel.size}`);
  },
  findDups: async () => {
    S.dupBusy = true; S.dupProg = null; render();
    try { const r = await invoke('find_duplicates', { minSize: 1048576 }); if (r) S.dups = r; else toast('Поиск остановлен'); }
    finally { S.dupBusy = false; render(); }
  },
  selDupGroup: (el) => {
    const [, files] = S.dups[+el.dataset.v];
    const candidates = files.filter((f) => f.state === 'unused');
    const keepOne = files.every((f) => f.state === 'unused');
    candidates.slice(keepOne ? 1 : 0).forEach((f) => addSel({ kind: 'sample', key: String(f.idx), label: f.name, size: f.size }));
    render();
  },
  clearSel: () => { S.sel.clear(); render(); },
  confirmTrash: () => {
    const items = [...S.sel.values()];
    S.modal = { title: `Переместить в корзину: ${items.length}`, ok: 'Переместить', danger: false,
      body: `<p class="mute" style="margin-top:0">Освободится ${sz(selSize())}, когда вы очистите корзину. До этого всё можно вернуть.</p><ul style="padding-left:18px;margin:0">${items.slice(0, 7).map((i) => `<li>${esc(i.label)} <span class="mute">${sz(i.size)}</span></li>`).join('')}${items.length > 7 ? `<li class="mute">и ещё ${items.length - 7}</li>` : ''}</ul>`,
      fn: doTrash };
    render();
  },
  closeMenu: () => { S.menu = null; render(); },
  closeModal: (el, e) => { if (e.target.closest('[data-stop]') && !el.classList.contains('btn')) return; S.modal = null; render(); },
  modalOk: async () => { const fn = S.modal.fn; S.modal = null; render(); await fn(); },
  restore: async (el) => { const id = el.dataset.v; const errs = await invoke('trash_restore', { ids: [id] }); await afterTrashChange(); errs.length ? toast(errs[0]) : toast('Вернул на прежнее место'); },
  purgeOne: (el) => {
    const id = el.dataset.v, it = S.trash.find((t) => t.id === id);
    S.modal = { title: 'Удалить навсегда?', ok: 'Удалить навсегда', danger: true, body: `<p>${esc(it.label)} будет стёрт с диска. Это нельзя отменить.</p>`, fn: async () => { await invoke('trash_purge', { ids: [id] }); await afterTrashChange(); toast('Удалено навсегда'); } };
    render();
  },
  emptyTrash: () => {
    const total = S.trash.reduce((a, i) => a + i.size, 0);
    S.modal = { title: 'Очистить корзину?', ok: 'Очистить', danger: true, body: `<p>Будет стёрто ${S.trash.length} элем. и освобождено ${sz(total)}. Это нельзя отменить.</p>`, fn: async () => { await invoke('trash_empty'); await afterTrashChange(); toast(`Освобождено ${sz(total)}`); } };
    render();
  },
  toastAction: () => { if (toast.fn) toast.fn(); $('#toast').hidden = true; },
  rescan: () => { if (S.view === 'wizard') S.step = 2; startScan(); },
  retry: () => { S.error = null; if (!S.report) { S.view = 'wizard'; S.step = 2; } else S.view = 'settings'; render(); },
  copyErr: () => { navigator.clipboard.writeText(S.error || '').then(() => toast('Скопировано'), () => toast('Выделите текст и скопируйте вручную')); },
  togDaw: (el) => { const v = el.dataset.v, a = S.settings.daws; S.settings.daws = a.includes(v) ? a.filter((x) => x !== v) : [...a, v]; render(); },
  step1: () => { S.step = 1; render(); },
  step2: () => {
    if (!S.settings.project_dirs.length) S.settings.project_dirs = S.boot.daws.filter((d) => S.settings.daws.includes(d.daw)).flatMap((d) => d.project_dirs);
    S.step = 2; render();
  },
  addDir: async (el) => { const key = el.dataset.key; const r = await pickFolders(); r.filter(Boolean).forEach((p) => { if (!S.settings[key].includes(p)) S.settings[key].push(p); }); render(); },
  rmDir: (el) => { S.settings[el.dataset.key].splice(+el.dataset.i, 1); render(); },
  setPref: (el) => { S.settings.preferred_format = el.value; },
  setOld: (el) => { S.settings.old_days = Math.max(30, +el.value || 365); },
  setPolicy: (el) => { S.settings.unknown_policy = el.value; },
  setBackups: (el) => { S.settings.include_backups = el.checked; },
};

async function doTrash() {
  const items = [...S.sel.values()].map((i) => ({ kind: i.kind, key: i.key, label: i.label, allow_old: !!i.allowOld }));
  const res = await invoke('trash_move', { items });
  res.moved.filter((m) => m.kind === 'plugin').forEach((m) => S.goneP.add(m.original_path));
  S.sel.clear();
  await afterTrashChange();
  const moved = res.moved.length, size = res.moved.reduce((a, m) => a + m.size, 0);
  if (res.errors.length) { S.modal = { title: 'Часть не удалось переместить', ok: 'Понятно', body: `<p>Перемещено: ${moved}. Не удалось: ${res.errors.length}.</p><ul style="padding-left:18px">${res.errors.slice(0, 6).map((e) => `<li>${esc(e)}</li>`).join('')}</ul>`, fn: async () => {} }; render(); }
  else toast(`В корзине: ${moved} · ${sz(size)}`, { label: 'Отменить', fn: async () => { await invoke('trash_restore', { ids: res.moved.map((m) => m.id) }); res.moved.forEach((m) => S.goneP.delete(m.original_path)); await afterTrashChange(); } });
}
async function afterTrashChange() {
  S.trash = await invoke('trash_list');
  // anything the user restored is visible again
  const present = new Set(S.trash.map((t) => t.original_path));
  S.goneP.forEach((p) => { if (!present.has(p)) S.goneP.delete(p); });
  S.report.samples = await invoke('sample_summary');
  S.file = null; S.dups = null;
  await loadKids();
  render();
}

/* ---------- events ---------- */
document.addEventListener('click', (e) => {
  if (S.menu && !e.target.closest('.menu')) { S.menu = null; render(); return; }
  const el = e.target.closest('[data-act]');
  if (!el) return;
  if (el.tagName === 'SELECT' || (el.tagName === 'INPUT' && el.type === 'number')) return;
  const inCheckbox = e.target.tagName === 'INPUT' && e.target.type === 'checkbox';
  const act = inCheckbox && e.target.dataset.act ? e.target : el;
  const fn = A[act.dataset.act];
  if (!fn || act.hasAttribute('disabled')) return;
  if (act.classList.contains('modal-bg') && e.target !== act) return;
  Promise.resolve(fn(act, e)).catch((err) => toast(String(err)));
});
document.addEventListener('change', (e) => {
  const el = e.target;
  if (el.tagName === 'SELECT' || (el.tagName === 'INPUT' && (el.type === 'number' || (el.type === 'checkbox' && el.dataset.act === 'setBackups')))) A[el.dataset.act] && A[el.dataset.act](el);
});
document.addEventListener('input', (e) => {
  if (e.target.id !== 'q') return;
  S.q = e.target.value;
  const pos = e.target.selectionStart; render(); const q = $('#q'); q.focus(); q.setSelectionRange(pos, pos);
});
document.addEventListener('contextmenu', (e) => {
  const el = e.target.closest('[data-ctx]');
  e.preventDefault();
  if (!el || !S.report) { if (S.menu) { S.menu = null; render(); } return; }
  const d = el.dataset, path = d.path;
  const items = [];
  if (d.ctx === 'plugin') {
    const p = visiblePlugins().find((x) => x.name === d.v);
    if (!p) return;
    S.selP = p.name;
    items.push({ label: 'Показать в проводнике', fn: () => reveal(p.formats[0].path) });
    items.push({ label: 'Деинсталлировать…', hint: 'утилита удаления производителя', fn: () => uninstallFor(p.name) });
    items.push({ label: 'Найти в интернете', hint: 'картинки интерфейса', fn: () => invoke('web_search', { query: p.name + ' plugin' }) });
    items.push('-');
    items.push({ label: 'Выбрать рекомендованное для корзины', disabled: !p.recommended.length, fn: () => A.selPluginRec({ dataset: { v: p.name } }) });
    items.push({ label: 'Скопировать путь', fn: () => copyText(p.formats.map((f) => f.path).join('\n')) });
  } else if (d.ctx === 'sample') {
    const allowed = d.state === 'unused' || (d.state === 'oldonly' && S.allowOld);
    items.push({ label: S.playing === path ? 'Пауза' : 'Прослушать', fn: () => playPath(path) });
    items.push({ label: 'Показать в проводнике', fn: () => reveal(path) });
    items.push({ label: 'Скопировать путь', fn: () => copyText(path) });
    items.push('-');
    items.push({ label: allowed ? 'Выбрать для корзины' : 'Нельзя убрать: файл нужен', disabled: !allowed, fn: () => { togSel({ kind: 'sample', key: d.idx, label: d.name, size: +d.size, allowOld: S.allowOld }); render(); } });
  } else if (d.ctx === 'dir') {
    items.push({ label: 'Открыть', fn: () => goDir(path) });
    items.push({ label: 'Показать в проводнике', fn: () => reveal(path) });
    items.push({ label: 'Скопировать путь', fn: () => copyText(path) });
  } else if (d.ctx === 'trash') {
    items.push({ label: 'Вернуть на место', fn: () => A.restore({ dataset: { v: d.id } }) });
    items.push({ label: 'Показать исходную папку', fn: () => reveal(path) });
  } else if (d.ctx === 'path') {
    items.push({ label: 'Показать в проводнике', fn: () => reveal(path) });
    items.push({ label: 'Скопировать путь', fn: () => copyText(path) });
  }
  openMenu(e, items);
});
document.addEventListener('mouseup', (e) => {
  if (S.view !== 'samples') return;
  if (e.button === 3) { e.preventDefault(); A.histBack(); }
  if (e.button === 4) { e.preventDefault(); A.histFwd(); }
});
let hoverTimer;
document.addEventListener('mouseover', (e) => {
  const row = e.target.closest('[data-img]');
  const tip = $('#tip');
  clearTimeout(hoverTimer);
  if (!row) { tip.hidden = true; return; }
  hoverTimer = setTimeout(() => {
    tip.innerHTML = `<img src="${row.dataset.img}" alt="">`;
    const r = row.getBoundingClientRect();
    tip.style.left = Math.min(r.left + 200, innerWidth - 380) + 'px';
    tip.style.top = Math.min(r.bottom + 4, innerHeight - 260) + 'px';
    tip.hidden = false;
  }, 300);
});
document.addEventListener('keydown', (e) => {
  if (e.altKey && S.view === 'samples' && e.key === 'ArrowLeft') { e.preventDefault(); A.histBack(); return; }
  if (e.altKey && S.view === 'samples' && e.key === 'ArrowRight') { e.preventDefault(); A.histFwd(); return; }
  if (e.key === 'Escape' && S.menu) { S.menu = null; render(); return; }
  if (e.code === 'Space' && S.dock && !/INPUT|SELECT|TEXTAREA|BUTTON/.test(e.target.tagName) && !e.target.matches('.row')) { e.preventDefault(); playPath(S.dock); return; }
  if ((e.key === 'Enter' || e.key === ' ') && e.target.matches('.row[data-act]')) { e.preventDefault(); e.target.click(); }
  if (e.key === 'Escape' && S.modal) { S.modal = null; render(); }
});

init().catch((e) => { document.body.insertAdjacentHTML('beforeend', `<pre style="color:#ef7b72;padding:20px">${esc(e)}</pre>`); });

/* audio clock: updates only the time label and seek bar, no full re-render */
(function () {
  const a = $('#audio');
  a.addEventListener('timeupdate', () => {
    const dc = document.querySelector('canvas[data-dock]');
    if (dc && S.playing === dc.dataset.wave) drawWave(dc, S.waves['1600|' + dc.dataset.wave], progressOf(dc.dataset.wave));
    document.querySelectorAll('canvas.mini').forEach((c) => { if (c.dataset.wave === S.playing) drawWave(c, S.waves['300|' + c.dataset.wave], progressOf(c.dataset.wave)); });
    const t = $('#ptime'), b = $('#pseek');
    const f = (x) => (isFinite(x) ? Math.floor(x / 60) + ':' + String(Math.floor(x % 60)).padStart(2, '0') : '–');
    if (t) t.textContent = f(a.currentTime) + ' / ' + f(a.duration) + (S.waves['1600|' + S.playing] ? ' · ' + waveMeta(S.playing).split(' · ').slice(1).join(' · ') : '');
    if (b && a.duration) b.style.width = (a.currentTime / a.duration * 100) + '%';
  });
  a.addEventListener('ended', () => render());
  a.addEventListener('error', () => { const e = $('#perr'); if (e && S.playing) e.textContent = 'Этот файл не воспроизводится встроенным плеером.'; });
})();
