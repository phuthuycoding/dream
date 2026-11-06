// terminal.js — xterm attach view: browser-style session tabs with
// keep-alive multi-attach. One xterm + WebSocket per open tab (lazy,
// LRU-capped); switching toggles pane visibility — no replay, no lost
// screen state.
//
// Wire protocol (src/ws/frame.rs): byte 0 is an ASCII opcode.
//   client → server: `i` + raw stdin bytes; `r` + cols,rows as u16 BE
//   server → client: `o` output, `s` snapshot, `e` end, `a` attention
//
// Attach contract (TASK-005/007): the server sends the retained
// scrollback tail as `o` frames, one `s` snapshot, then live `o`.
// The snapshot begins `\x1bc` + `\x1b[8;<rows>;<cols>t` — the client MUST
// resize xterm to that geometry before writing the dump, so the
// emulator's cells map 1:1 onto the screen. `o` frames that arrive
// before the snapshot are buffered and written after the resize.
// A dead session sends only `o` replay + `e` — no `s` — so pending
// scrollback is flushed on `e` or socket close.
//
// Tab model (ui-v2 FR-002): `localStorage["dream.tabs"]` holds the open
// tab list (order = strip order); `#/s/<id>` is the source of truth for
// the *active* tab. At most MAX_LIVE terminal instances stay alive —
// activating beyond the cap evicts the least-recently-used background
// tab (its tab stays in the strip and re-attaches on next activation).

import {
  api,
  wsUrl,
  setAttention,
  clearAttention,
  AuthError,
} from './api.js';

const OP_INPUT = 0x69; // 'i'
const OP_RESIZE = 0x72; // 'r'
const OP_OUTPUT = 0x6f; // 'o'
const OP_SNAPSHOT = 0x73; // 's'
const OP_END = 0x65; // 'e'
const OP_ATTENTION = 0x61; // 'a'

const RECONNECT_MIN_MS = 500;
const RECONNECT_MAX_MS = 8000;
const MAX_LIVE = 6;

const $ = (id) => document.getElementById(id);
const encoder = new TextEncoder();
const latin1 = new TextDecoder('latin1');
const utf8 = new TextDecoder();

// tabs: id -> {
//   id, session, term, fitAddon, socket, pane,
//   scrollback, snapshotApplied, ended, userClosed,
//   reconnectTimer, reconnectAttempt, appCursorMode,
// }
const tabs = new Map();
let liveOrder = []; // most-recently-used first — LRU bookkeeping
let activeId = null;
let resizeObserver = null;
let ctrlArmed = false;

function active() {
  return activeId ? tabs.get(activeId) : null;
}

function setStatus(text, cls) {
  const pill = $('term-status');
  pill.textContent = text;
  pill.className = `pill ${cls || ''}`;
}

function setTitle(attention) {
  const inst = active();
  const name = inst && inst.session ? inst.session.name : 'terminal';
  $('term-title').textContent = name;
  document.title = `${attention ? '● ' : ''}${name} — dream`;
}

// ---- wire encode/decode ----

function sendFrame(inst, opcode, payload) {
  if (!inst.socket || inst.socket.readyState !== WebSocket.OPEN) return;
  const frame = new Uint8Array(1 + payload.length);
  frame[0] = opcode;
  frame.set(payload, 1);
  inst.socket.send(frame);
}

function sendInput(inst, str) {
  sendFrame(inst, OP_INPUT, encoder.encode(str));
}

function sendResize(inst, cols, rows) {
  const payload = new Uint8Array(4);
  const view = new DataView(payload.buffer);
  view.setUint16(0, cols);
  view.setUint16(2, rows);
  sendFrame(inst, OP_RESIZE, payload);
}

// Snapshot geometry: `\x1bc` + `\x1b[8;<rows>;<cols>t` leads the stream.
function parseGeometry(payload) {
  const head = latin1.decode(payload.subarray(0, 64));
  const match = /\x1b\[8;(\d+);(\d+)t/.exec(head);
  if (!match) return null;
  const rows = Number(match[1]);
  const cols = Number(match[2]);
  return rows > 0 && cols > 0 ? { rows, cols } : null;
}

// Track DECCKM application-cursor mode from the output stream so the
// soft-key arrows emit the right sequence in TUIs. Byte scan (no string
// allocation) for `\x1b[?1h` / `\x1b[?1l`; the last occurrence wins.
function scanCursorMode(inst, payload) {
  for (let i = payload.length - 5; i >= 0; i -= 1) {
    if (
      payload[i] === 0x1b &&
      payload[i + 1] === 0x5b && // '['
      payload[i + 2] === 0x3f && // '?'
      payload[i + 3] === 0x31 // '1'
    ) {
      if (payload[i + 4] === 0x68) inst.appCursorMode = true; // 'h'
      else if (payload[i + 4] === 0x6c) inst.appCursorMode = false; // 'l'
      return;
    }
  }
}

// ---- frame handlers ----

function handleOutput(inst, payload) {
  scanCursorMode(inst, payload);
  if (inst.snapshotApplied) {
    inst.term.write(payload);
  } else {
    inst.scrollback.push(payload);
  }
}

function flushScrollback(inst) {
  for (const chunk of inst.scrollback) inst.term.write(chunk);
  inst.scrollback = [];
}

function handleSnapshot(inst, payload) {
  // The dump may carry DECCKM (\x1b[?1h/l); scan it like `o` frames do so
  // soft arrows are right after a fresh attach.
  scanCursorMode(inst, payload);
  const geometry = parseGeometry(payload);
  if (
    geometry &&
    (inst.term.cols !== geometry.cols || inst.term.rows !== geometry.rows)
  ) {
    inst.term.resize(geometry.cols, geometry.rows);
  }
  flushScrollback(inst);
  inst.term.write(payload);
  inst.snapshotApplied = true;
  // Adopt the container size next; if it differs from the emulator's the
  // server gets an `r` and reflows/repaints at the real browser size.
  if (inst.id === activeId) fitSoon(inst);
}

function handleEnd(inst, payload) {
  flushScrollback(inst); // dead attach: replay arrives without a snapshot
  inst.ended = true;
  if (inst.id === activeId) {
    setStatus('ended', 'bad');
    $('overlay-text').textContent =
      utf8.decode(payload) || 'session ended';
    $('term-overlay').classList.remove('hidden');
    $('btn-term-respawn').classList.remove('hidden');
  }
}

function handleAttention(inst) {
  setAttention(inst.id);
  if (inst.id === activeId) setTitle(true);
}

function onMessage(inst, event) {
  if (!(event.data instanceof ArrayBuffer) || event.data.byteLength === 0) return;
  const frame = new Uint8Array(event.data);
  const payload = frame.subarray(1);
  switch (frame[0]) {
    case OP_OUTPUT:
      handleOutput(inst, payload);
      break;
    case OP_SNAPSHOT:
      handleSnapshot(inst, payload);
      break;
    case OP_END:
      handleEnd(inst, payload);
      break;
    case OP_ATTENTION:
      handleAttention(inst);
      break;
    default:
      break; // unknown opcode — drop
  }
}

// ---- socket lifecycle ----

function connect(inst) {
  clearTimeout(inst.reconnectTimer);
  inst.userClosed = false;
  inst.scrollback = [];
  inst.snapshotApplied = false;
  inst.ended = false;
  inst.socket = new WebSocket(wsUrl(inst.id));
  inst.socket.binaryType = 'arraybuffer';
  if (inst.id === activeId) setStatus('connecting…', 'warn');

  inst.socket.onopen = () => {
    inst.reconnectAttempt = 0;
    // A reconnect replays scrollback + snapshot; reset so the replay
    // does not duplicate history already on screen.
    inst.term.reset();
    if (inst.id === activeId) setStatus('connected', 'ok');
    if (inst.id === activeId) fitSoon(inst);
  };
  inst.socket.onmessage = (ev) => onMessage(inst, ev);
  inst.socket.onclose = () => {
    flushScrollback(inst); // never strand buffered replay bytes
    if (inst.userClosed || inst.ended) return;
    const delay = Math.min(
      RECONNECT_MIN_MS * 2 ** inst.reconnectAttempt,
      RECONNECT_MAX_MS,
    );
    inst.reconnectAttempt += 1;
    if (inst.id === activeId) {
      setStatus(`reconnecting in ${Math.round(delay / 100) / 10}s`, 'warn');
    }
    inst.reconnectTimer = setTimeout(() => connect(inst), delay);
  };
  inst.socket.onerror = () => {
    // onclose follows with the real teardown/reconnect decision.
  };
}

function dispose(inst) {
  inst.userClosed = true;
  clearTimeout(inst.reconnectTimer);
  if (inst.socket) {
    inst.socket.onclose = null;
    inst.socket.onmessage = null;
    inst.socket.close();
    inst.socket = null;
  }
  if (inst.term) {
    inst.term.dispose();
    inst.term = null;
  }
  inst.pane.remove();
  tabs.delete(inst.id);
  liveOrder = liveOrder.filter((x) => x !== inst.id);
}

async function respawnAndReattach(inst) {
  try {
    inst.session = await api.respawnSession(inst.id);
  } catch (err) {
    if (err instanceof AuthError) return; // gate handles it
    window.alert(err instanceof Error ? err.message : String(err));
    return;
  }
  inst.ended = false;
  if (inst.id === activeId) {
    $('term-overlay').classList.add('hidden');
    $('btn-term-respawn').classList.add('hidden');
  }
  connect(inst);
}

// ---- sizing ----

function fitSoon(inst) {
  // Fit runs after layout settles; onTermResize below relays the result.
  requestAnimationFrame(() => {
    try {
      inst.fitAddon.fit();
    } catch {
      /* pane hidden — next resize/visibility event retries */
    }
  });
}

function onTermResize(inst, { cols, rows }) {
  sendResize(inst, cols, rows);
}

// ---- input ----

function ctrlChar(ch) {
  const code = ch.toUpperCase().charCodeAt(0);
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code - 0x40);
  if (ch === '?') return '\x7f';
  return ch;
}

function onData(inst, data) {
  if (ctrlArmed) {
    ctrlArmed = false;
    $('softkey-ctrl').classList.remove('active');
    sendInput(inst, ctrlChar(data[0]) + data.slice(1));
  } else {
    sendInput(inst, data);
  }
}

function softKey(name) {
  const inst = active();
  if (!inst) return;
  const arrows = { left: 'D', down: 'B', up: 'A', right: 'C' };
  if (name === 'ctrl') {
    ctrlArmed = !ctrlArmed;
    $('softkey-ctrl').classList.toggle('active', ctrlArmed);
    return;
  }
  if (name === 'esc') sendInput(inst, '\x1b');
  else if (name === 'tab') sendInput(inst, '\t');
  else if (name in arrows) {
    const letter = arrows[name];
    if (ctrlArmed) {
      // CSI 1;5<x> — modified arrow in normal and app modes alike.
      sendInput(inst, `\x1b[1;5${letter}`);
      ctrlArmed = false;
      $('softkey-ctrl').classList.remove('active');
    } else {
      sendInput(inst, inst.appCursorMode ? `\x1bO${letter}` : `\x1b[${letter}`);
    }
  }
}

// ---- instance lifecycle ----

function createPane() {
  const pane = document.createElement('div');
  pane.className = 'term-pane hidden-pane';
  $('term-container').appendChild(pane);
  return pane;
}

async function ensureInstance(id) {
  let inst = tabs.get(id);
  if (inst) return inst;
  const session = await api.getSession(id);
  inst = {
    id,
    session,
    term: null,
    fitAddon: null,
    socket: null,
    pane: createPane(),
    scrollback: [],
    snapshotApplied: false,
    ended: false,
    userClosed: false,
    reconnectTimer: null,
    reconnectAttempt: 0,
    appCursorMode: false,
  };
  tabs.set(id, inst);

  inst.term = new Terminal({
    cursorBlink: true,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    fontSize: 14,
    scrollback: 10000,
    allowProposedApi: true,
    theme: { background: '#000000' },
  });
  inst.fitAddon = new FitAddon.FitAddon();
  inst.term.loadAddon(inst.fitAddon);
  inst.term.loadAddon(new WebLinksAddon.WebLinksAddon());
  try {
    const webgl = new WebglAddon.WebglAddon();
    inst.term.loadAddon(webgl);
    webgl.onContextLoss(() => webgl.dispose());
  } catch {
    // No WebGL — the DOM/canvas renderer already handles this terminal.
  }
  inst.term.open(inst.pane);

  inst.term.onData((data) => onData(inst, data));
  inst.term.onResize((size) => onTermResize(inst, size));
  inst.term.onBell(() => {
    if (document.hidden) {
      setAttention(inst.id);
      if (inst.id === activeId) setTitle(true);
    }
  });

  connect(inst);
  return inst;
}

// Bounds live xterm+socket pairs to MAX_LIVE; the active tab is never
// evicted. Evicted tabs stay in the strip and re-attach lazily.
function evictIfNeeded() {
  while (liveOrder.length > MAX_LIVE) {
    const lru = [...liveOrder].reverse().find((id) => id !== activeId);
    if (!lru) return;
    dispose(tabs.get(lru));
  }
}

// Switch the view's active session. Called on mount and by the router on
// `#/s/<a>` → `#/s/<b>` — the terminal view stays mounted across both.
export async function activate(id) {
  const inst = await ensureInstance(id);
  const prev = activeId ? tabs.get(activeId) : null;
  activeId = id;
  liveOrder = [id, ...liveOrder.filter((x) => x !== id)];

  if (prev && prev.pane) prev.pane.classList.add('hidden-pane');
  inst.pane.classList.remove('hidden-pane');
  // The active pane leads DOM order so a bare `.xterm` locator resolves
  // to the visible emulator.
  $('term-container').prepend(inst.pane);

  clearAttention(id);
  setTitle(false);
  if (inst.ended) {
    setStatus('ended', 'bad');
  } else if (inst.socket && inst.socket.readyState === WebSocket.OPEN) {
    setStatus('connected', 'ok');
  } else if (inst.session.status === 'Dead') {
    setStatus('dead', 'bad');
  } else {
    setStatus('connecting…', 'warn');
  }
  $('term-overlay').classList.toggle('hidden', !inst.ended);
  $('btn-term-respawn').classList.toggle('hidden', !inst.ended);

  evictIfNeeded();
  fitSoon(inst);
  inst.term.focus();
}

// ---- mount / unmount ----

export async function mount(sessionId) {
  $('btn-back').onclick = () => {
    window.location.hash = '#/';
  };
  $('btn-overlay-back').onclick = () => {
    window.location.hash = '#/';
  };
  $('btn-overlay-respawn').onclick = () => {
    const inst = active();
    if (inst) void respawnAndReattach(inst);
  };
  $('btn-term-respawn').onclick = () => {
    const inst = active();
    if (inst) void respawnAndReattach(inst);
  };

  for (const btn of document.querySelectorAll('#softkeys .softkey')) {
    // pointerdown (not click) keeps focus inside the xterm textarea so
    // the virtual keyboard stays open between key taps. Assignment (not
    // addEventListener): the buttons outlive mount(), so listeners would
    // stack and fire once per past attach.
    btn.onpointerdown = (ev) => {
      ev.preventDefault();
      softKey(btn.dataset.key);
      const inst = active();
      if (inst && inst.term) inst.term.focus();
    };
  }

  resizeObserver = new ResizeObserver(() => {
    const inst = active();
    if (inst) fitSoon(inst);
  });
  resizeObserver.observe($('term-wrap'));
  window.addEventListener('orientationchange', fitActive);
  document.addEventListener('visibilitychange', onVisible);

  await activate(sessionId);
}

function fitActive() {
  const inst = active();
  if (inst) fitSoon(inst);
}

function onVisible() {
  if (!document.hidden) {
    fitActive();
    if (activeId) {
      clearAttention(activeId);
      setTitle(false);
    }
  }
}

export function unmount() {
  for (const inst of [...tabs.values()]) dispose(inst);
  activeId = null;
  liveOrder = [];
  if (resizeObserver) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
  window.removeEventListener('orientationchange', fitActive);
  document.removeEventListener('visibilitychange', onVisible);
  ctrlArmed = false;
  document.title = 'dream';
}
