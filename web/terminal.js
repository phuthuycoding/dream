// terminal.js — xterm attach view: scrollback → snapshot → live stream,
// input/resize relay, reconnect, mobile soft keys.
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

const $ = (id) => document.getElementById(id);
const encoder = new TextEncoder();
const latin1 = new TextDecoder('latin1');
const utf8 = new TextDecoder();

let session = null;
let term = null;
let fitAddon = null;
let socket = null;
let scrollback = []; // `o` payloads buffered until the snapshot arrives
let snapshotApplied = false;
let ended = false;
let userClosed = false;
let reconnectTimer = null;
let reconnectAttempt = 0;
let appCursorMode = false; // DECCKM — tracked so soft arrows send SS3
let ctrlArmed = false;
let resizeObserver = null;

function setStatus(text, cls) {
  const pill = $('term-status');
  pill.textContent = text;
  pill.className = `pill ${cls || ''}`;
}

function setTitle(attention) {
  const name = session ? session.name : 'terminal';
  $('term-title').textContent = name;
  document.title = `${attention ? '● ' : ''}${name} — webterm`;
}

// ---- wire encode/decode ----

function sendFrame(opcode, payload) {
  if (!socket || socket.readyState !== WebSocket.OPEN) return;
  const frame = new Uint8Array(1 + payload.length);
  frame[0] = opcode;
  frame.set(payload, 1);
  socket.send(frame);
}

function sendInput(str) {
  sendFrame(OP_INPUT, encoder.encode(str));
}

function sendResize(cols, rows) {
  const payload = new Uint8Array(4);
  const view = new DataView(payload.buffer);
  view.setUint16(0, cols);
  view.setUint16(2, rows);
  sendFrame(OP_RESIZE, payload);
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
function scanCursorMode(payload) {
  for (let i = payload.length - 5; i >= 0; i -= 1) {
    if (
      payload[i] === 0x1b &&
      payload[i + 1] === 0x5b && // '['
      payload[i + 2] === 0x3f && // '?'
      payload[i + 3] === 0x31 // '1'
    ) {
      if (payload[i + 4] === 0x68) appCursorMode = true; // 'h'
      else if (payload[i + 4] === 0x6c) appCursorMode = false; // 'l'
      return;
    }
  }
}

// ---- frame handlers ----

function handleOutput(payload) {
  scanCursorMode(payload);
  if (snapshotApplied) {
    term.write(payload);
  } else {
    scrollback.push(payload);
  }
}

function flushScrollback() {
  for (const chunk of scrollback) term.write(chunk);
  scrollback = [];
}

function handleSnapshot(payload) {
  // The dump may carry DECCKM (\x1b[?1h/l); scan it like `o` frames do so
  // soft arrows are right after a fresh attach.
  scanCursorMode(payload);
  const geometry = parseGeometry(payload);
  if (geometry && (term.cols !== geometry.cols || term.rows !== geometry.rows)) {
    term.resize(geometry.cols, geometry.rows);
  }
  flushScrollback();
  term.write(payload);
  snapshotApplied = true;
  // Adopt the container size next; if it differs from the emulator's the
  // server gets an `r` and reflows/repaints at the real browser size.
  fitSoon();
}

function handleEnd(payload) {
  flushScrollback(); // dead attach: replay arrives without a snapshot
  ended = true;
  setStatus('ended', 'bad');
  $('overlay-text').textContent =
    utf8.decode(payload) || 'session ended';
  $('term-overlay').classList.remove('hidden');
  $('btn-term-respawn').classList.remove('hidden');
}

function handleAttention() {
  if (!session) return;
  setAttention(session.id);
  setTitle(true);
}

function onMessage(event) {
  if (!(event.data instanceof ArrayBuffer) || event.data.byteLength === 0) return;
  const frame = new Uint8Array(event.data);
  const payload = frame.subarray(1);
  switch (frame[0]) {
    case OP_OUTPUT:
      handleOutput(payload);
      break;
    case OP_SNAPSHOT:
      handleSnapshot(payload);
      break;
    case OP_END:
      handleEnd(payload);
      break;
    case OP_ATTENTION:
      handleAttention();
      break;
    default:
      break; // unknown opcode — drop
  }
}

// ---- socket lifecycle ----

function connect() {
  clearTimeout(reconnectTimer);
  userClosed = false; // unmount() disarms reconnect; every attach re-arms it
  scrollback = [];
  snapshotApplied = false;
  ended = false;
  socket = new WebSocket(wsUrl(session.id));
  socket.binaryType = 'arraybuffer';
  setStatus('connecting…', 'warn');

  socket.onopen = () => {
    reconnectAttempt = 0;
    // A reconnect replays scrollback + snapshot; reset so the replay
    // does not duplicate history already on screen.
    term.reset();
    setStatus('connected', 'ok');
    fitSoon();
  };
  socket.onmessage = onMessage;
  socket.onclose = () => {
    flushScrollback(); // never strand buffered replay bytes
    if (userClosed || ended) return;
    const delay = Math.min(
      RECONNECT_MIN_MS * 2 ** reconnectAttempt,
      RECONNECT_MAX_MS,
    );
    reconnectAttempt += 1;
    setStatus(`reconnecting in ${Math.round(delay / 100) / 10}s`, 'warn');
    reconnectTimer = setTimeout(connect, delay);
  };
  socket.onerror = () => {
    // onclose follows with the real teardown/reconnect decision.
  };
}

async function respawnAndReattach() {
  try {
    session = await api.respawnSession(session.id);
  } catch (err) {
    if (err instanceof AuthError) return; // gate handles it
    window.alert(err instanceof Error ? err.message : String(err));
    return;
  }
  ended = false;
  $('term-overlay').classList.add('hidden');
  $('btn-term-respawn').classList.add('hidden');
  connect();
}

// ---- sizing ----

function fitSoon() {
  // Fit runs after layout settles; onResize below relays the result.
  requestAnimationFrame(() => {
    try {
      fitAddon.fit();
    } catch {
      /* container hidden — next resize/visibility event retries */
    }
  });
}

function onTermResize({ cols, rows }) {
  sendResize(cols, rows);
}

// ---- input ----

function ctrlChar(ch) {
  const code = ch.toUpperCase().charCodeAt(0);
  if (code >= 0x40 && code <= 0x5f) return String.fromCharCode(code - 0x40);
  if (ch === '?') return '\x7f';
  return ch;
}

function onData(data) {
  if (ctrlArmed) {
    ctrlArmed = false;
    $('softkey-ctrl').classList.remove('active');
    sendInput(ctrlChar(data[0]) + data.slice(1));
  } else {
    sendInput(data);
  }
}

function softKey(name) {
  const arrows = { left: 'D', down: 'B', up: 'A', right: 'C' };
  if (name === 'ctrl') {
    ctrlArmed = !ctrlArmed;
    $('softkey-ctrl').classList.toggle('active', ctrlArmed);
    return;
  }
  if (name === 'esc') sendInput('\x1b');
  else if (name === 'tab') sendInput('\t');
  else if (name in arrows) {
    const letter = arrows[name];
    if (ctrlArmed) {
      // CSI 1;5<x> — modified arrow in normal and app modes alike.
      sendInput(`\x1b[1;5${letter}`);
      ctrlArmed = false;
      $('softkey-ctrl').classList.remove('active');
    } else {
      sendInput(appCursorMode ? `\x1bO${letter}` : `\x1b[${letter}`);
    }
  }
}

// ---- mount / unmount ----

export async function mount(sessionId) {
  session = await api.getSession(sessionId);
  clearAttention(session.id);
  setTitle(false);
  setStatus(session.status.toLowerCase(), session.status === 'Dead' ? 'bad' : 'ok');

  term = new Terminal({
    cursorBlink: true,
    fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
    fontSize: 14,
    scrollback: 10000,
    allowProposedApi: true,
    theme: { background: '#000000' },
  });
  fitAddon = new FitAddon.FitAddon();
  term.loadAddon(fitAddon);
  term.loadAddon(new WebLinksAddon.WebLinksAddon());
  try {
    const webgl = new WebglAddon.WebglAddon();
    term.loadAddon(webgl);
    webgl.onContextLoss(() => webgl.dispose());
  } catch {
    // No WebGL — the DOM/canvas renderer already handles this terminal.
  }
  term.open($('term-container'));
  fitSoon();
  term.focus();

  term.onData(onData);
  term.onResize(onTermResize);
  term.onBell(() => {
    if (document.hidden && session) {
      setAttention(session.id);
      setTitle(true);
    }
  });

  $('btn-back').onclick = () => {
    window.location.hash = '#/';
  };
  $('btn-overlay-back').onclick = () => {
    window.location.hash = '#/';
  };
  $('btn-overlay-respawn').onclick = () => void respawnAndReattach();
  $('btn-term-respawn').onclick = () => void respawnAndReattach();

  for (const btn of document.querySelectorAll('#softkeys .softkey')) {
    // pointerdown (not click) keeps focus inside the xterm textarea so
    // the virtual keyboard stays open between key taps. Assignment (not
    // addEventListener): the buttons outlive mount(), so listeners would
    // stack and fire once per past attach.
    btn.onpointerdown = (ev) => {
      ev.preventDefault();
      softKey(btn.dataset.key);
      if (term) term.focus();
    };
  }

  resizeObserver = new ResizeObserver(fitSoon);
  resizeObserver.observe($('term-wrap'));
  window.addEventListener('orientationchange', fitSoon);
  document.addEventListener('visibilitychange', onVisible);

  connect();
}

function onVisible() {
  if (!document.hidden) {
    fitSoon();
    if (session) {
      clearAttention(session.id);
      setTitle(false);
    }
  }
}

export function unmount() {
  userClosed = true;
  clearTimeout(reconnectTimer);
  if (socket) {
    socket.onclose = null;
    socket.onmessage = null;
    socket.close();
    socket = null;
  }
  if (resizeObserver) {
    resizeObserver.disconnect();
    resizeObserver = null;
  }
  window.removeEventListener('orientationchange', fitSoon);
  document.removeEventListener('visibilitychange', onVisible);
  if (term) {
    term.dispose();
    term = null;
  }
  if (session) clearAttention(session.id);
  document.title = 'webterm';
  session = null;
}
