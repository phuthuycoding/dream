// dashboard.js — session list, create/edit modal, lifecycle actions.
//
// mount() renders into #view-dashboard and starts a poll loop; unmount()
// stops it. All mutations re-render from the server response so the list
// stays the source of truth (FR-001, FR-006).

import { api, hasAttention, AuthError } from './api.js';

const POLL_MS = 3000;

const STATUS_LABEL = { Live: 'live', Dead: 'dead', Resurrected: 'resurrected' };

let pollTimer = null;
let sessions = [];
let editingId = null; // null → create mode

const $ = (id) => document.getElementById(id);

function showError(el, err) {
  el.textContent = err instanceof Error ? err.message : String(err);
  el.classList.remove('hidden');
}

function statusBadge(session) {
  // `resurrected` (TASK-009 marker) wins over a plain `Live` — FR-005
  // wants the resurrection visible on the dashboard.
  const key = session.resurrected ? 'Resurrected' : session.status;
  const label = STATUS_LABEL[key] || String(key).toLowerCase();
  const badge = document.createElement('span');
  badge.className = `badge ${label}`;
  badge.textContent = label;
  return badge;
}

function relTime(iso) {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return iso;
  const secs = Math.max(0, (Date.now() - then) / 1000);
  if (secs < 60) return `${Math.floor(secs)}s ago`;
  if (secs < 3600) return `${Math.floor(secs / 60)}m ago`;
  if (secs < 86400) return `${Math.floor(secs / 3600)}h ago`;
  return `${Math.floor(secs / 86400)}d ago`;
}

function sessionCard(session) {
  const dead = session.status === 'Dead';
  const card = document.createElement('div');
  const activeRoute = /^#\/s\/([0-9a-fA-F-]+)$/.exec(window.location.hash);
  card.className = `session-card${dead ? ' is-dead' : ''}${activeRoute && activeRoute[1] === session.id ? ' active' : ''}`;
  card.dataset.sessionId = session.id;

  const head = document.createElement('div');
  head.className = 'session-head';
  const statusDot = document.createElement('span');
  statusDot.className = `status-dot ${session.resurrected ? 'resurrected' : session.status.toLowerCase()}`;
  head.appendChild(statusDot);
  const name = document.createElement('span');
  name.className = 'session-name';
  name.textContent = session.name;
  name.title = session.name;
  head.appendChild(name);
  if (hasAttention(session.id)) {
    const dot = document.createElement('span');
    dot.className = 'attention-dot';
    dot.title = 'attention';
    head.appendChild(dot);
  }
  head.appendChild(statusBadge(session));
  card.appendChild(head);

  const meta = document.createElement('div');
  meta.className = 'session-meta';
  const addRow = (label, value, mono, copyable, title) => {
    const row = document.createElement('div');
    row.className = 'meta-row';
    const k = document.createElement('span');
    k.className = 'mk';
    k.textContent = label;
    const v = document.createElement('span');
    v.className = `mv${mono ? ' mono' : ''}${copyable ? ' copyable' : ''}`;
    v.textContent = value;
    v.title = title ?? value;
    if (copyable) {
      v.addEventListener('click', (ev) => {
        ev.stopPropagation();
        // navigator.clipboard is undefined on insecure origins (LAN/tailnet
        // http) — fall back to a selection copy for those.
        const done = () => {
          v.classList.add('copied');
          setTimeout(() => v.classList.remove('copied'), 900);
        };
        const fallback = () => {
          const ta = document.createElement('textarea');
          ta.value = session.id;
          document.body.appendChild(ta);
          ta.select();
          const ok = document.execCommand('copy');
          ta.remove();
          if (ok) done();
        };
        if (navigator.clipboard) {
          void navigator.clipboard.writeText(session.id).then(done, fallback);
        } else {
          fallback();
        }
      });
    }
    row.appendChild(k);
    row.appendChild(v);
    meta.appendChild(row);
  };
  addRow('cmd', session.cmd, true);
  addRow('cwd', session.cwd, true);
  addRow('id', session.id.slice(0, 8), true, true, `${session.id} (click to copy)`);
  addRow('created', relTime(session.created_at), false, false, `created ${session.created_at}`);
  if (session.setup_script) addRow('setup', session.setup_script, true);
  card.appendChild(meta);

  const actions = document.createElement('div');
  actions.className = 'session-actions';
  const mk = (label, cls, title, fn) => {
    const btn = document.createElement('button');
    btn.className = `btn small ${cls}`;
    btn.textContent = label;
    btn.title = title;
    btn.addEventListener('click', (ev) => {
      ev.stopPropagation();
      void fn();
    });
    actions.appendChild(btn);
  };

  mk('attach', 'primary', 'Open the terminal', () => {
    window.location.hash = `#/s/${session.id}`;
  });
  mk('edit', '', 'Rename / edit metadata', () => openModal(session));
  if (session.status !== 'Dead') {
    mk('kill', '', 'Terminate the shell (session stays listed)', () =>
      run(() => api.killSession(session.id)));
  } else {
    mk('respawn', '', 'Spawn a fresh shell', () =>
      run(() => api.respawnSession(session.id)));
  }
  mk('clean', '', "Reset this session's replay log", () =>
    run(() => api.cleanSession(session.id)));
  const spacer2 = document.createElement('span');
  spacer2.className = 'spacer';
  actions.appendChild(spacer2);
  mk('delete', 'danger', 'Delete the session and its state', () => {
    if (window.confirm(`Delete session "${session.name}"? Its state and log are removed.`)) {
      return run(() => api.deleteSession(session.id));
    }
    return undefined;
  });
  card.appendChild(actions);

  // Clicking the item attaches — every interactive child stops
  // propagation before this can fire. On touch the drawer closes.
  card.addEventListener('click', () => {
    window.location.hash = `#/s/${session.id}`;
    document.getElementById('shell').classList.remove('side-open');
  });
  return card;
}

async function run(action) {
  try {
    await action();
    await refresh();
  } catch (err) {
    // 401 already raised dream:unauthorized — the gate handles the UX.
    if (err instanceof AuthError) return;
    window.alert(err instanceof Error ? err.message : String(err));
  }
}

async function refresh() {
  sessions = await api.listSessions();
  render();
}

function render() {
  const list = $('session-list');
  list.replaceChildren();
  for (const session of sessions) {
    list.appendChild(sessionCard(session));
  }
  $('session-count').textContent = String(sessions.length);
  $('dashboard-empty').classList.toggle('hidden', sessions.length > 0);
}

// ---- create / edit modal ----

function openModal(session) {
  editingId = session ? session.id : null;
  $('modal-title').textContent = session ? `Edit "${session.name}"` : 'New session';
  $('modal-submit').textContent = session ? 'save' : 'create';
  // PATCH has no `cmd` field — editing the command would imply a respawn.
  $('f-cmd-row').classList.toggle('hidden', Boolean(session));
  $('f-name').value = session ? session.name : '';
  $('f-cmd').value = session ? session.cmd : '';
  $('f-cwd').value = session ? session.cwd : '';
  $('f-setup').value = session && session.setup_script ? session.setup_script : '';
  $('modal-error').classList.add('hidden');
  $('modal').classList.remove('hidden');
  $('f-name').focus();
}

function closeModal() {
  $('modal').classList.add('hidden');
  editingId = null;
}

function nonEmpty(value) {
  const trimmed = value.trim();
  return trimmed === '' ? undefined : trimmed;
}

async function submitModal(ev) {
  ev.preventDefault();
  const errEl = $('modal-error');
  errEl.classList.add('hidden');
  try {
    if (editingId) {
      await api.patchSession(editingId, {
        name: nonEmpty($('f-name').value),
        cwd: nonEmpty($('f-cwd').value),
        setup_script: nonEmpty($('f-setup').value),
      });
      closeModal();
      await refresh();
    } else {
      const created = await api.createSession({
        name: nonEmpty($('f-name').value),
        cmd: nonEmpty($('f-cmd').value),
        cwd: nonEmpty($('f-cwd').value),
        setup_script: nonEmpty($('f-setup').value),
      });
      closeModal();
      window.location.hash = `#/s/${created.id}`;
    }
  } catch (err) {
    if (err instanceof AuthError) return;
    showError(errEl, err);
  }
}

export async function mount() {
  $('btn-new').onclick = () => openModal(null);
  $('btn-empty-new').onclick = () => openModal(null);
  $('btn-refresh').onclick = () => void refresh();
  $('btn-clean-all').onclick = () => {
    if (window.confirm("Reset every session's replay log?")) {
      return run(() => api.cleanAllLogs());
    }
    return undefined;
  };
  $('modal-form').onsubmit = (ev) => void submitModal(ev);
  $('modal-cancel').onclick = closeModal;
  // Assignment (not addEventListener): #modal outlives mount(), so a
  // listener would stack once per dashboard visit.
  $('modal').onclick = (ev) => {
    if (ev.target === $('modal')) closeModal();
  };
  window.addEventListener('hashchange', onHashChange);
  await refresh();
  pollTimer = setInterval(() => {
    refresh().catch((err) => {
      if (!(err instanceof AuthError)) console.warn('poll failed:', err);
    });
  }, POLL_MS);
}

// Route changes re-mark the sidebar's active item immediately.
function onHashChange() {
  render();
}

export function unmount() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  window.removeEventListener('hashchange', onHashChange);
  closeModal();
}
