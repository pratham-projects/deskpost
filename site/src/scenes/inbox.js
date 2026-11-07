// Inbox: `@ > -` glyphs drift in, coalesce into an envelope that opens, a thread (`| + L`) weaves down to the queue,
// and a ticket row lands in the stack. One 16 s cycle; the engine's 240 pre-stepped frames (7.9 s) end on an open envelope.
import { registerScene } from "../kit/ascii.js";

const CYCLE = 16000;
const GLYPHS = "@>-";
const SUBJECTS = ["Re: invoice", "Login loop on iOS", "Refund for order", "Re: printer jam", "Where is my key?", "Cannot export CSV", "Re: SSO setup", "Widget shows blank"];
const STATUS = ["pending", "in_progress", "resolved"];
const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
const smooth = (x) => { x = clamp(x); return x * x * (3 - 2 * x); };

const K = 0.000022;               // spring stiffness (cols per ms^2 per col)
const C = 2 * Math.sqrt(K);       // critical damping
const VMAX = 0.045;               // cols per ms while travelling to a target

registerScene("inbox", ({ cols, rows, rand }) => {
  const narrow = cols < 72;
  let W = Math.max(15, Math.min(34, cols - 6));
  if (W % 2 === 0) W--;
  const half = (W - 1) / 2;
  const H = Math.max(5, Math.round(W * 0.26));
  const fd = Math.max(2, Math.round(H * 0.5));   // closed flap depth
  const fh = Math.max(2, Math.round(H * 0.35));  // open flap height

  // layout: envelope above, ticket stack bottom right, thread in the gap
  const nRows = narrow ? 2 : 3;
  const TH = nRows + 3;
  const ex0 = narrow ? Math.floor((cols - W) / 2) : cols - W - 4;
  const tt = rows - TH - 1;
  const top0 = narrow ? Math.max(Math.floor(rows * 0.4), rows - 21) : 1;
  const bandTop = top0 + fh + 1, bandBot = tt - (narrow ? 2 : 5) - H;
  const ey0 = bandBot >= bandTop ? bandTop + Math.floor((bandBot - bandTop) * (narrow ? 0.15 : 0.2)) : bandTop;
  const ye = ey0 + H;               // first thread row
  const threadX = ex0 + 3;
  const threadLen = Math.max(0, tt - ye);

  // keep the headline side calm: ambient glyphs fade toward the left (wide) or the top (narrow)
  const calm = (c, r) => narrow
    ? 0.15 + 0.85 * smooth((r - (top0 - 4)) / 6)
    : 0.2 + 0.8 * smooth((c / cols - 0.25) / 0.3);

  const ps = [];
  const add = (o) => {
    ps.push({ x: rand() * cols, y: rand() * rows, vx: 0, vy: 0, g: Math.floor(rand() * 3), ph: rand() * 6.28,
      dx: 0.0008 + rand() * 0.0022, dy: (rand() - 0.5) * 0.0006, on: false, ...o });
  };
  const gate = rand;

  // envelope outline
  for (let x = 0; x < W; x++) for (const y of [0, H - 1]) {
    add({ kind: "b", tx: ex0 + x, ty: ey0 + y, ch: x === 0 || x === W - 1 ? "+" : "-", tone: 1, a: 0.8, gd: gate() * 2800, rd: 12000 + gate() * 2200 });
  }
  for (let y = 1; y < H - 1; y++) for (const x of [0, W - 1]) {
    add({ kind: "b", tx: ex0 + x, ty: ey0 + y, ch: "|", tone: 1, a: 0.8, gd: gate() * 2800, rd: 12000 + gate() * 2200 });
  }
  // flap: closed V pointing down, or open and pointing up
  for (let i = 1; i <= half; i++) {
    const yc = Math.max(1, Math.round((i * fd) / half)), ycn = Math.max(1, Math.round(((i + 1) * fd) / half));
    const yo = -Math.max(1, Math.round((i * fh) / half)), yon = -Math.max(1, Math.round(((i + 1) * fh) / half));
    for (const side of i === half ? [0] : [0, 1]) {
      const x = side ? W - 1 - i : i;
      const step = (a, b) => a !== b;
      const closedCh = i === half ? "v" : step(yc, ycn) ? (side ? "/" : "\\") : "-";
      const openCh = i === half ? "^" : step(yo, yon) ? (side ? "\\" : "/") : "-";
      add({ kind: "f", tx: ex0 + x, ty: ey0 + yc, ch: closedCh, tx2: ex0 + x, ty2: ey0 + yo, ch2: openCh, tone: 2, a: 0.95,
        gd: 200 + gate() * 2600, flip: 6000 + gate() * 500, rd: 12000 + gate() * 2200 });
    }
  }
  // letter inside the open envelope
  const lines = [[2, 8, W - 5], [4, 4, W - 5], [6, 4, W - 12]].filter(([y]) => y <= H - 2);
  lines.forEach(([y, x0, x1], k) => {
    for (let x = x0; x <= x1; x++) {
      if (x % 4 === 3) continue;
      add({ kind: "l", tx: ex0 + x, ty: ey0 + y, ch: "-", tone: 0, a: 0.85, gd: 3000 + gate() * 1500, rd: 11200 + gate() * 2000 });
    }
    if (k === 0) add({ kind: "l", seal: true, tx: ex0 + 4, ty: ey0 + y, ch: "@", tone: 2, a: 1, gd: 3200 + gate() * 1200, rd: 11200 + gate() * 2000 });
  });
  // ambient drifters
  const nAmb = Math.round(cols * rows * 0.028);
  for (let i = 0; i < nAmb; i++) add({ kind: "a" });

  // ticket stack
  const tickets = [];
  let id = 4810 + Math.floor(rand() * 60), posted = -1, si = Math.floor(rand() * SUBJECTS.length);
  const post = (born) => {
    tickets.unshift({ id: ++id, subj: SUBJECTS[si++ % SUBJECTS.length], st: STATUS[id % 3], born });
    if (tickets.length > nRows) tickets.pop();
  };
  post(-1e9); post(-1e9);

  const line = (l, r) => l.slice(0, Math.max(0, W - r.length - 2)) + " ".repeat(Math.max(1, W - l.slice(0, Math.max(0, W - r.length - 2)).length - r.length)) + r;

  return {
    step(t, dt, ptr, put) {
      const tt0 = t + 5600;            // start 5.6 s in so the first paint already shows an envelope forming
      const cyc = tt0 % CYCLE, cn = Math.floor(tt0 / CYCLE);
      if (cyc >= 7000 && posted !== cn) { posted = cn; post(t); }

      for (const p of ps) {
        let tx = 0, ty = 0, ch = "", on = false;
        if (p.kind !== "a") {
          on = cyc >= p.gd && cyc < p.rd;
          const open = p.kind === "f" && cyc >= p.flip;
          tx = open ? p.tx2 : p.tx; ty = open ? p.ty2 : p.ty; ch = open ? p.ch2 : p.ch;
          if (p.on && !on) { p.vx += (gate() - 0.5) * 0.02; p.vy -= 0.003 + gate() * 0.01; }
          p.on = on;
        }
        let ax = 0, ay = 0;
        const dx = p.x - ptr.col, dy = (p.y - ptr.row) * 2, d = Math.hypot(dx, dy);
        if (d < 8 && d > 0.01) { const f = (1 - d / 8) * 0.00004; ax += (dx / d) * f; ay += (dy / d) * f * 0.5; }
        if (on) {
          ax += K * (tx - p.x) - C * p.vx;
          ay += K * (ty - p.y) - C * p.vy;
        } else {
          ax += (p.dx - p.vx) * 0.004;
          ay += (p.dy - p.vy) * 0.004;
        }
        p.vx += ax * dt; p.vy += ay * dt;
        if (on) { const sp = Math.hypot(p.vx, p.vy); if (sp > VMAX) { p.vx *= VMAX / sp; p.vy *= VMAX / sp; } }
        p.x += p.vx * dt; p.y += p.vy * dt;
        if (!on) {
          if (p.x > cols + 2) p.x = -2; else if (p.x < -2) p.x = cols + 2;
          if (p.y < -2) p.y = rows + 2; else if (p.y > rows + 2) p.y = -2;
        }

        const c = Math.round(p.x), r = Math.round(p.y);
        const near = on ? clamp(1 - Math.hypot(tx - p.x, (ty - p.y) * 2) / 3) : 0;
        if (p.kind === "l" && on) {
          // the letter waits inside the closed envelope, then shows once the flap is open
          const show = smooth((cyc - 6400) / 600);
          if (show > 0 && near > 0.55) put(c, r, ch, p.tone, p.a * show * (p.seal ? 0.75 + 0.25 * Math.sin(t * 0.004) : 1));
        } else if (on && near > 0.55) {
          let a = p.a * (0.7 + 0.3 * near);
          if (p.seal) a *= 0.75 + 0.25 * Math.sin(t * 0.004);
          put(c, r, ch, p.tone, a);
        } else if (on) {
          put(c, r, GLYPHS[p.g], 1, 0.3 + 0.45 * near);
        } else if (!(c >= ex0 - 1 && c <= ex0 + W && r >= tt - 1 && r <= tt + TH)) {
          put(c, r, GLYPHS[p.g], 0, (0.3 + 0.35 * (0.5 + 0.5 * Math.sin(t * 0.002 + p.ph))) * calm(c, r));
        }
      }

      // thread: grows down from the envelope, weaves, drains away on release
      const grow = smooth((cyc - 6400) / 1300) * (1 - smooth((cyc - 12000) / 2000));
      if (threadLen > 3 && grow > 0) {
        const n = Math.floor(threadLen * grow);
        for (let i = 0; i <= n; i++) {
          const y = ye + i, x = threadX + Math.round(Math.sin(i * 0.5 + t * 0.0007) * 1.4);
          const head = i === n && grow < 1;
          if (head || i % 5 === 4) put(x, y, "+", head ? 2 : 1, 0.9);
          else if (i % 10 === 7) { put(x, y, "L", 1, 0.9); put(x + 1, y, "-", 0, 0.6); put(x + 2, y, "-", 0, 0.4); }
          else put(x, y, "|", 0, 0.7);
        }
      }

      // ticket stack, same idea as the ledger floor
      const text = (r, s, tone, a, shown = s.length) => { for (let i = 0; i < Math.min(s.length, shown); i++) put(ex0 + i, r, s[i], tone, a, true); };
      text(tt, "-".repeat(W), 0, 0.5);
      text(tt + 1, line("INBOX", `${tickets.length} open` + (Math.floor(t / 500) % 2 ? "_" : " ")), 0, 0.85);
      tickets.forEach((k, i) => {
        text(tt + 2 + i, line(`#${k.id} ${k.subj}`, k.st), i === 0 ? 1 : 0, 1 - i * 0.25, Math.floor((t - k.born) / 20));
      });
      text(tt + 2 + nRows, "-".repeat(W), 0, 0.5);
    },
  };
});
