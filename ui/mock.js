/* Demo backend used when the page is opened in a normal browser (design work, screenshots).
   Inside the desktop app window.__TAURI__ exists and this file does nothing. */
(function () {
  if (window.__TAURI__) return;
  const MB = 1048576, GB = 1073741824;
  const W = (n, f) => f === 'vst3' ? `C:\\Program Files\\Common Files\\VST3\\${n}.vst3` : f === 'vst2' ? `C:\\Program Files\\VSTPlugins\\${n}.dll` : `C:\\Program Files\\Common Files\\Avid\\Audio\\Plug-Ins\\${n}.aaxplugin`;
  const fam = (name, fm) => ({ name, fm });
  const spec = [
    fam('FabFilter Pro-Q 3', { vst3: [38, ['Night Drive v7', 'Beat 042', 'Remix Alina', 'Album Intro']], vst2: [41, []], aax: [40, []] }),
    fam('Serum', { vst3: [112, ['Night Drive v7', 'Beat 042']], vst2: [118, ['Old Idea 2021', 'Sketch 2024-03']] }),
    fam('Valhalla VintageVerb', { vst3: [9, ['Night Drive v7', 'Vocal Demo', 'Album Intro']], vst2: [10, []], aax: [9, []] }),
    fam('Kontakt 7', { vst3: [520, ['Album Intro', 'Remix Alina']], aax: [515, []] }),
    fam('Kickstart 2', { vst3: [14, ['Night Drive v7', 'Beat 042', 'Remix Alina']], vst2: [15, []] }),
    fam('Sylenth1', { vst2: [12, ['Old Idea 2021']], vst3: [12, []] }),
    fam('Soundtoys Decapitator', { vst3: [61, ['Beat 042', 'Vocal Demo']], aax: [60, []] }),
    fam('Auto-Tune Pro', { vst3: [180, ['Vocal Demo']], aax: [176, []] }),
    fam('Arturia Pigments', { vst3: [260, ['Album Intro']], vst2: [255, []] }),
    fam('iZotope Ozone 9', { vst3: [210, []], vst2: [205, []], aax: [200, []] }),
    fam('Guitar Rig 6', { vst3: [340, []], vst2: [335, []], aax: [330, []] }),
    fam('Massive', { vst2: [96, []] }),
    fam('TAL-Reverb-4', { vst3: [7, []] }),
    fam('Waves CLA-76', { vst3: [22, ['Vocal Demo']], aax: [21, []] }),
  ];
  const rm = { aax: true };
  const plugins = spec.map(({ name, fm }) => {
    const any = Object.values(fm).some(([, u]) => u.length);
    const formats = Object.entries(fm).map(([format, [mb, users]]) => {
      const removable = users.length === 0;
      return { format, path: W(name, format), size: mb * MB, removable,
        used_by: users.map(p => ({ project: p, daw: 'ableton', exact: true })), protected_by: [],
        reason: users.length ? `Используется в проектах: ${users.length}` : 'Ни один проект не использует этот формат' };
    }).sort((a, b) => a.format.localeCompare(b.format));
    const recommended = formats.filter(f => f.removable && !(any && f.format === 'vst3')).map(f => f.path);
    const users = new Set(Object.values(fm).flatMap(([, u]) => u));
    return { name, formats, used_projects: users.size, status: !any ? 'unused' : recommended.length ? 'duplicate' : 'used',
      recommended, total_size: formats.reduce((a, f) => a + f.size, 0), recommended_size: formats.filter(f => recommended.includes(f.path)).reduce((a, f) => a + f.size, 0) };
  });
  const pr = (id, name, d, mb, miss) => ({ id, name, daw: 'ableton', path: `D:\\Projects\\${name}\\${name}.als`, mtime: Date.now() / 1000 - d * 86400, size: mb * MB, plugins: [], file_refs: 60 + id * 17, missing: miss, old: d > 365 });
  const projects = [pr(0, 'Night Drive v7', 19, 1480, []), pr(1, 'Beat 042', 33, 620, []), pr(2, 'Remix Alina', 89, 2310, []), pr(3, 'Album Intro', 104, 3950, []), pr(4, 'Vocal Demo', 150, 410, []), pr(5, 'Sketch 2024-03', 960, 340, []), pr(6, 'Old Idea 2021', 1800, 890, ['D:/Archive/Vox/Chops_03.wav', 'D:/Archive/Vox/Chops_04.wav', 'D:/Archive/Pads/Warm_A.wav'])];
  plugins.forEach(p => p.formats.forEach(f => f.used_by.forEach(u => {
    const pj = projects.find(x => x.name === u.project);
    if (pj && !pj.plugins.some(q => q.name === p.name && q.format === f.format)) pj.plugins.push({ name: p.name, format: f.format, state: 'ok' });
  })));

  // sample tree: [path, bytes, state]
  const files = [];
  const add = (dir, n, mb, st, by = []) => files.push({ slash: `D:/Samples/${dir}/${n}`, size: mb * MB, mtime: 0, state: st, by_name: false, gone: false, by });
  for (let i = 1; i <= 26; i++) add('Splice/Sounds/Drums', `Deep_House_Drums_${String(i).padStart(2, '0')}.wav`, 30 + i * 9, i % 3 ? 'unused' : 'used', i % 3 ? [] : ['Night Drive v7']);
  for (let i = 1; i <= 14; i++) add('Splice/Sounds/Vocals', `Vocal_Chops_${i}.wav`, 20 + i * 7, i % 5 === 0 ? 'oldonly' : 'unused', i % 5 === 0 ? ['Old Idea 2021'] : []);
  for (let i = 1; i <= 10; i++) add('Loopmasters/Deep Tech', `DT_Loop_${i}.wav`, 60 + i * 12, i < 3 ? 'used' : 'unused', i < 3 ? ['Beat 042'] : []);
  for (let i = 1; i <= 12; i++) add('Old Packs 2019', `Trap_Essentials_${i}.wav`, 80 + i * 25, 'unused');
  for (let i = 1; i <= 6; i++) add('Projects/Recorded', `Vocal_Take_${i}.wav`, 40 + i * 4, 'projectowned');
  const state = { gone: new Set(), trash: [], settings: null, cache: 38 * MB };
  const roots = ['D:/Samples'];
  const sub = (e) => e.slash;
  const info = (i) => { const e = files[i]; return { idx: i, path: e.slash, name: e.slash.split('/').pop(), size: e.size, mtime: 0, state: e.state, by_name: false, used_by: e.by }; };
  function children(dir) {
    const prefix = dir ? dir.replace(/\/$/, '') + '/' : 'D:/Samples/';
    const map = new Map();
    files.forEach((e, i) => {
      if (e.gone || !e.slash.startsWith(prefix)) return;
      const rest = e.slash.slice(prefix.length); const k = rest.indexOf('/');
      const name = k < 0 ? rest : rest.slice(0, k);
      const c = map.get(name) || { name, path: prefix + name, is_dir: k >= 0, bytes: 0, files: 0, unused_bytes: 0, used_bytes: 0, old_bytes: 0, owned_bytes: 0, state: k < 0 ? e.state : null, idx: k < 0 ? i : null };
      c.bytes += e.size; c.files++;
      ({ unused: () => c.unused_bytes += e.size, used: () => c.used_bytes += e.size, oldonly: () => c.old_bytes += e.size, projectowned: () => c.owned_bytes += e.size })[e.state]();
      map.set(name, c);
    });
    return [...map.values()].sort((a, b) => b.bytes - a.bytes);
  }
  function summary() {
    const s = { files: 0, bytes: 0, used_bytes: 0, old_bytes: 0, unused_bytes: 0, owned_bytes: 0, unused_files: 0 };
    files.filter(e => !e.gone).forEach(e => { s.files++; s.bytes += e.size; if (e.state === 'used') s.used_bytes += e.size; if (e.state === 'oldonly') s.old_bytes += e.size; if (e.state === 'projectowned') s.owned_bytes += e.size; if (e.state === 'unused') { s.unused_bytes += e.size; s.unused_files++; } });
    return s;
  }
  const report = () => ({ plugins, projects, samples: summary(), timings: { plugins_ms: 40, projects_ms: 310, samples_ms: 820, analysis_ms: 90, cached_projects: 211, parsed_projects: 3 },
    unreadable: ['D:\\Samples\\Pack\\Broken Rack.adg: ill-formed document', 'D:\\Projects\\Old\\Old.als: unexpected end of file'],
    warnings: ['Studio One: плагины определяются по названиям внутри проекта, формат неизвестен. Защищаются все форматы, которые эта DAW умеет загружать.'] });
  const listeners = {};
  const wait = ms => new Promise(r => setTimeout(r, ms));
  const handlers = {
    boot: async () => ({ settings: state.settings || { daws: ['ableton'], project_dirs: [], sample_dirs: [], extra_plugin_dirs: [], library_dirs: [], old_days: 365, include_backups: false, preferred_format: 'vst3', unknown_policy: 'safe' },
      daws: [['ableton', 'Ableton Live', true, 'exact'], ['studioone', 'Studio One', true, 'heuristic'], ['flstudio', 'FL Studio', false, 'heuristic'], ['protools', 'Pro Tools', true, 'heuristic'], ['bitwig', 'Bitwig Studio', false, 'heuristic'], ['cubase', 'Cubase', false, 'heuristic'], ['reaper', 'REAPER', false, 'heuristic'], ['logic', 'Logic Pro', false, 'none']]
        .map(([daw, name, installed, support]) => ({ daw, name, installed, support, project_dirs: installed ? [`C:\\Users\\you\\Documents\\${name}`] : [] })), first_run: !state.settings, platform: 'windows' }),
    save_settings: async ({ settings }) => { state.settings = settings; },
    run_scan: async () => { for (let i = 0; i <= 10; i++) { (listeners['scan-progress'] || []).forEach(f => f({ payload: i < 4 ? { stage: 'plugins', done: 0, total: 0 } : i < 8 ? { stage: 'projects', done: i * 26, total: 214 } : { stage: 'samples', done: i * 41000, total: 0 } })); await wait(140); } return report(); },
    sample_children: async ({ dir }) => ({ items: children(dir), hidden_files: 0, parent: null }),
    sample_top: async ({ state: st, under, limit }) => files.map((e, i) => [e, i]).filter(([e]) => !e.gone && ({ unused: 'unused', oldonly: 'oldonly', used: 'used', projectowned: 'projectowned' })[st] === e.state && (!under || e.slash.startsWith(under + '/'))).sort((a, b) => b[0].size - a[0].size).slice(0, limit).map(([, i]) => info(i)),
    sample_info: async ({ idx }) => info(idx),
    sample_summary: async () => summary(),
    find_duplicates: async () => { for (let i = 0; i <= 10; i++) { (listeners['dup-progress'] || []).forEach(f => f({ payload: i < 4 ? { stage: 'quick', done_files: i * 300, total_files: 1200, done_bytes: 0, total_bytes: 0 } : { stage: 'full', done_files: i * 4, total_files: 40, done_bytes: i * 400 * MB, total_bytes: 4 * GB } })); await wait(120); } const mk = i => info(i); return [[{ size: 330 * MB, files: [0, 1] }, [mk(2), mk(5)]], [{ size: 190 * MB, files: [2, 3] }, [mk(30), mk(41)]]]; },
    trash_move: async ({ items }) => {
      const moved = items.map(r => { const id = Math.random().toString(16).slice(2, 10); const it = { id, original_path: r.kind === 'plugin' ? r.key : files[+r.key].slash, stored_path: 'trash/' + id, size: 0, trashed_at: Date.now() / 1000, kind: r.kind, label: r.label };
        if (r.kind === 'sample') { files[+r.key].gone = true; it.size = files[+r.key].size; } else { const f = plugins.flatMap(p => p.formats).find(f => f.path === r.key); it.size = f ? f.size : 0; }
        state.trash.unshift(it); return it; });
      return { moved, errors: [] };
    },
    trash_list: async () => state.trash,
    cancel_duplicates: async () => {},
    wave: async ({ path, buckets }) => {
      await wait(150);
      let h = 0; for (const c of path) h = (h * 31 + c.charCodeAt(0)) >>> 0;
      const rnd = () => ((h = (h * 1103515245 + 12345) >>> 0) / 4294967296);
      const peaks = []; const hits = 4 + Math.floor(rnd() * 12);
      for (let i = 0; i < buckets; i++) { const ph = (i * hits / buckets) % 1; const env = Math.exp(-ph * 6) * (0.4 + rnd() * 0.6); peaks.push(-env, env * (0.85 + rnd() * 0.15)); }
      return { peaks, duration: 2 + (h % 20), sample_rate: 44100, channels: 2 };
    },
    reveal_path: async () => {},
    web_search: async () => {},
    plugin_uninstallers: async ({ name }) => name.startsWith('Massive') ? [] : [{ id: 'k1', name: name + ' 1.4.2', publisher: 'Vendor', version: '1.4.2', location: '' }],
    run_uninstaller: async () => {},
    cache_info: async () => ({ bytes: state.cache, files: [], folder: '' }),
    clear_cache: async () => { const n = state.cache; state.cache = 0; return n; },
    trash_restore: async ({ ids }) => { ids.forEach(id => { const it = state.trash.find(t => t.id === id); if (it && it.kind === 'sample') { const f = files.find(f => f.slash === it.original_path); if (f) f.gone = false; } state.trash = state.trash.filter(t => t.id !== id); }); return []; },
    trash_purge: async ({ ids }) => { const n = state.trash.filter(t => ids.includes(t.id)).reduce((a, t) => a + t.size, 0); state.trash = state.trash.filter(t => !ids.includes(t.id)); return n; },
    trash_empty: async () => { const n = state.trash.reduce((a, t) => a + t.size, 0); state.trash = []; return n; },
  };
  window.__MOCK__ = {
    invoke: (cmd, args) => (handlers[cmd] ? handlers[cmd](args || {}) : Promise.reject('no mock for ' + cmd)),
    listen: async (ev, fn) => { (listeners[ev] = listeners[ev] || []).push(fn); },
    pickFolder: async () => 'D:/Samples',
  };
})();
