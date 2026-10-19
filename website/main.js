// theme
document.querySelector('.theme-btn')?.addEventListener('click', () => {
  const r = document.documentElement, n = r.dataset.theme === 'light' ? 'dark' : 'light';
  r.dataset.theme = n;
  try { localStorage.setItem('tm-theme', n); } catch (e) {}
});

// live process demo (simulated)
(() => {
  const cv = document.getElementById('map'), tb = document.getElementById('ptable');
  if (!cv || !tb) return;
  const names = ['Safari', 'Xcode', 'Slack', 'Chrome Helper (Renderer)', 'WindowServer', 'Spotify', 'node', 'rust-analyzer', 'Docker', 'Finder', 'Mail', 'Figma', 'kernel_task', 'mds_stores', 'Photos', 'Terminal', 'cargo', 'Dock', 'Notes', 'Messages', 'zoom.us', 'backupd', 'Preview', 'TaskMaster'];
  const procs = names.map((n, i) => ({ n, pid: 300 + i * 137 % 9000, cpu: Math.random() * (i < 6 ? 30 : 4), mem: 40 + Math.random() * (i < 8 ? 1400 : 200), tgt: 0 }));
  procs.forEach((p) => (p.tgt = p.cpu));
  const hist = Array(150).fill(0);
  let sort = ['cpu', true], sel = null, q = '', hover = false;
  const css = (v) => getComputedStyle(document.documentElement).getPropertyValue(v).trim();
  const heat = (t) => `hsl(${Math.round(160 - 160 * Math.min(1, Math.max(0, t)))} 75% 55%)`;
  const col = (n) => { let h = 0; for (const c of n) h = (h * 31 + c.charCodeAt(0)) % 360; return `hsl(${h} 50% 72%)`; };
  const fmt = (m) => (m >= 1000 ? (m / 1000).toFixed(2) + ' GB' : Math.round(m) + ' MB');

  function step() {
    for (const p of procs) {
      if (Math.random() < 0.08) p.tgt = Math.random() < 0.15 ? 40 + Math.random() * 55 : Math.random() * 8;
      p.cpu += (p.tgt - p.cpu) * 0.35 + (Math.random() - 0.5) * 1.2; p.cpu = Math.max(0.1, p.cpu);
      p.mem = Math.max(20, p.mem + (Math.random() - 0.48) * 6);
    }
    const tot = Math.min(100, procs.reduce((a, p) => a + p.cpu, 0) / 8);
    hist.push(tot); hist.shift();
    document.getElementById('dcpu').textContent = Math.round(tot) + '%';
    document.getElementById('dmem').textContent = Math.round(procs.reduce((a, p) => a + p.mem, 0) / 160) + '%';
    draw(); render();
  }
  function draw() {
    const dpr = devicePixelRatio || 1, W = cv.clientWidth, H = cv.clientHeight, c = cv.getContext('2d');
    cv.width = W * dpr; cv.height = H * dpr; c.setTransform(dpr, 0, 0, dpr, 0, 0);
    c.fillStyle = css('--bg'); c.fillRect(0, 0, W, H);
    c.strokeStyle = css('--line'); c.lineWidth = 1;
    for (let i = 1; i < 4; i++) { c.beginPath(); c.moveTo(0, H * i / 4 + .5); c.lineTo(W, H * i / 4 + .5); c.stroke(); }
    const pts = hist.map((v, i) => [W * i / 149, H - (v / 100) * (H - 6) - 3]);
    const g = c.createLinearGradient(0, 0, 0, H); const a = css('--accent');
    g.addColorStop(0, a + '66'); g.addColorStop(1, a + '00');
    c.beginPath(); c.moveTo(0, H); pts.forEach((p) => c.lineTo(...p)); c.lineTo(W, H); c.fillStyle = g; c.fill();
    c.beginPath(); pts.forEach((p, i) => (i ? c.lineTo(...p) : c.moveTo(...p))); c.strokeStyle = a; c.lineWidth = 1.6; c.stroke();
  }
  function render() {
    const rows = procs.filter((p) => !q || p.n.toLowerCase().includes(q) || String(p.pid) === q)
      .sort((a, b) => { const k = sort[0], d = k === 'n' ? a.n.localeCompare(b.n) : a[k] - b[k]; return sort[1] ? -d : d; });
    const top = tb.scrollTop; tb.textContent = '';
    const head = document.createElement('div'); head.className = 'r h';
    [['n', 'Name'], ['pid', 'PID'], ['cpu', 'CPU %'], ['mem', 'Memory']].forEach(([k, t], i) => {
      const s = document.createElement('span'); s.textContent = t + (sort[0] === k ? (sort[1] ? ' v' : ' ^') : ''); if (i) s.className = 'c';
      s.onclick = () => { sort = sort[0] === k ? [k, !sort[1]] : [k, k !== 'n' && k !== 'pid']; render(); }; head.append(s);
    });
    tb.append(head);
    for (const p of rows) {
      const r = document.createElement('div'); r.className = 'r' + (sel === p.pid ? ' sel' : '');
      const n = document.createElement('span'); n.className = 'n';
      const i = document.createElement('i'); i.textContent = p.n[0]; i.style.background = col(p.n);
      n.append(i, p.n);
      const cells = [p.pid, p.cpu.toFixed(1), fmt(p.mem)].map((t) => { const s = document.createElement('span'); s.className = 'c'; s.textContent = t; return s; });
      cells[1].style.color = p.cpu > 50 ? heat(p.cpu / 100) : '';
      const b = document.createElement('i'); b.className = 'bar'; b.style.cssText = `right:${14 + 84 + 0}px;width:${Math.min(70, p.cpu * .7)}px;background:${heat(p.cpu / 100)}`;
      r.append(n, ...cells, b); r.onclick = () => { sel = sel === p.pid ? null : p.pid; render(); }; tb.append(r);
    }
    tb.scrollTop = top;
  }
  document.getElementById('dq').addEventListener('input', (e) => { q = e.target.value.trim().toLowerCase(); render(); });
  for (let i = 0; i < 20; i++) step();
  setInterval(() => { if (!document.hidden) step(); }, 1000);
  addEventListener('resize', draw);
})();

document.querySelectorAll('.copy').forEach((b) => b.addEventListener('click', async () => {
  try { await navigator.clipboard.writeText(b.dataset.copy); b.textContent = 'Copied'; setTimeout(() => (b.textContent = 'Copy'), 1600); } catch (e) {}
}));

// live GitHub star count
fetch('https://api.github.com/repos/MarshallNickolauson/TaskMaster').then((r) => r.json()).then((d) => {
  if (typeof d.stargazers_count !== 'number') return;
  const n = new Intl.NumberFormat('en', { notation: 'compact' }).format(d.stargazers_count);
  document.querySelectorAll('[data-stars]').forEach((el) => { el.querySelector('span').textContent = n; el.hidden = false; });
}).catch(() => {});
