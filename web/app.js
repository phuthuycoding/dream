// app.js — bootstrap, token gate and the hash router.
//
// Routes: `#/` dashboard, `#/s/<id>` terminal attach. Views live in
// dashboard.js / terminal.js; this module only decides which is mounted
// and whether the token gate stands in front (FR-007).

import { adoptTokenFromUrl, getToken, setToken, clearToken, api, AuthError } from './api.js';
import * as dashboard from './dashboard.js';
import * as terminal from './terminal.js';

const $ = (id) => document.getElementById(id);
const VIEWS = ['view-gate', 'view-dashboard', 'view-terminal'];

let active = null; // 'dashboard' | 'terminal'
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
  show('view-gate');
}

function unmountActive() {
  if (active === 'dashboard') dashboard.unmount();
  else if (active === 'terminal') terminal.unmount();
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
  const target = match ? 'terminal' : 'dashboard';
  if (active === target && !match) return; // dashboard stays mounted
  unmountActive();
  try {
    if (match) {
      show('view-terminal');
      active = 'terminal';
      await terminal.mount(match[1]);
    } else {
      show('view-dashboard');
      active = 'dashboard';
      await dashboard.mount();
    }
  } catch (err) {
    unmountActive();
    if (err instanceof AuthError) {
      authed = false;
      showGate(err.message);
    } else {
      // e.g. attaching a deleted session — fall back to the list.
      console.warn('route mount failed:', err);
      window.location.hash = '#/';
      show('view-dashboard');
      active = 'dashboard';
      await dashboard.mount().catch(() => showGate());
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
  window.addEventListener('hashchange', () => void route());
  window.addEventListener('dream:unauthorized', () => {
    authed = false;
    showGate('session expired — paste the token again');
  });
  void tryStoredToken();
}

boot();
