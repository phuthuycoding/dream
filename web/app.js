// app.js — bootstrap, token gate and the hash router.
//
// Routes: `#/` home panel, `#/s/<id>` terminal attach. The session-list
// sidebar (#view-dashboard) is persistent once authed — it is the
// dashboard; `#main` swaps between the home panel and the terminal.
// Views live in dashboard.js / terminal.js; this module decides which is
// mounted and whether the token gate stands in front (FR-007).

import { adoptTokenFromUrl, getToken, setToken, clearToken, api, AuthError } from './api.js';
import * as dashboard from './dashboard.js';
import * as terminal from './terminal.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['view-gate', 'view-home', 'view-terminal'];

let active = null; // 'terminal' | null
let dashAlive = false; // sidebar mounted + polling
let dashMounting = false; // mount in flight — drop must not leak it
let authed = false;
let navGen = 0;
let navQueue = Promise.resolve();

function show(viewId) {
  for (const id of VIEWS) {
    $(id).classList.toggle('hidden', id !== viewId);
  }
}

function showGate(message) {
  const err = $('gate-error');
  err.textContent = message || '';
  err.classList.toggle('hidden', !message);
  unmountActive();
  dropDashboard();
  show('view-gate');
}

async function ensureDashboard() {
  if (dashAlive || dashMounting) return;
  dashMounting = true;
  try {
    $('sidebar').classList.remove('hidden');
    await dashboard.mount();
    if (authed) {
      dashAlive = true;
    } else {
      // logout landed mid-mount — undo it
      dashboard.unmount();
      $('sidebar').classList.add('hidden');
    }
  } finally {
    dashMounting = false;
  }
}

function dropDashboard() {
  if (!dashAlive) return;
  dashboard.unmount();
  $('sidebar').classList.add('hidden');
  dashAlive = false;
}

function unmountActive() {
  if (active === 'terminal') terminal.unmount();
  active = null;
}

// Views keep module-level state, so two mounts in flight would corrupt
// each other. Navigations run one at a time; a step superseded by a newer
// hashchange while still queued bails without touching the views.
function route() {
  const gen = ++navGen;
  navQueue = navQueue
    .then(() => (gen === navGen ? applyRoute() : undefined))
    .catch((err) => console.warn('route failed:', err));
  return navQueue;
}

async function applyRoute() {
  if (!authed) {
    showGate();
    return;
  }
  const match = /^#\/s\/([0-9a-fA-F-]+)$/.exec(window.location.hash);
  try {
    await ensureDashboard();
  } catch (err) {
    $('sidebar').classList.add('hidden');
    if (err instanceof AuthError) {
      authed = false;
      showGate(err.message);
    } else {
      console.warn('dashboard mount failed:', err);
    }
    return;
  }
  if (match && active === 'terminal') {
    // Session switch inside the mounted view — no teardown, no replay.
    try {
      await terminal.activate(match[1]);
    } catch (err) {
      unmountActive();
      if (err instanceof AuthError) {
        authed = false;
        showGate(err.message);
      } else {
        console.warn('route activate failed:', err);
        window.location.hash = '#/';
      }
    }
    return;
  }
  unmountActive();
  try {
    if (match) {
      show('view-terminal');
      active = 'terminal';
      await terminal.mount(match[1]);
    } else {
      show('view-home');
    }
  } catch (err) {
    unmountActive();
    if (err instanceof AuthError) {
      authed = false;
      showGate(err.message);
    } else {
      // e.g. attaching a deleted session — fall back home.
      console.warn('route mount failed:', err);
      window.location.hash = '#/';
      show('view-home');
    }
  }
}

async function tryStoredToken() {
  if (!getToken()) {
    showGate();
    return;
  }
  try {
    await api.listSessions(); // probe — a bad stored token gets 401'd
    authed = true;
    await route();
  } catch (err) {
    if (err instanceof AuthError) {
      showGate('stored token rejected — paste the token again');
    } else {
      showGate(`cannot reach the daemon: ${err.message}`);
    }
  }
}

async function gateSubmit(ev) {
  ev.preventDefault();
  const token = $('gate-token').value.trim();
  if (!token) return;
  setToken(token);
  $('gate-token').value = '';
  await tryStoredToken();
}

function boot() {
  adoptTokenFromUrl();
  $('gate-form').addEventListener('submit', (ev) => void gateSubmit(ev));
  $('btn-logout').addEventListener('click', () => {
    clearToken();
    authed = false;
    showGate();
  });
  $('btn-side').addEventListener('click', () => {
    $('shell').classList.toggle('side-open');
  });
  $('side-backdrop').addEventListener('click', () => {
    $('shell').classList.remove('side-open');
  });
  window.addEventListener('hashchange', () => void route());
  window.addEventListener('dream:unauthorized', () => {
    authed = false;
    showGate('session expired — paste the token again');
  });
  void tryStoredToken();
}

boot();
