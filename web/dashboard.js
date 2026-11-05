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

function sessionCard(session) {
  const card = document.createElement('div');
  card.className = 'session-card';
  card.dataset.sessionId = session.id;

  const head = document.createElement('div');
  head.className = 'session-head';
  if (hasAttention(session.id)) {
    const dot = document.createElement('span');
    dot.className = 'attention-dot';
    dot.title = 'attention';
    head.appendChild(dot);
  }
  const name = document.createElement('span');
  name.className = 'session-name';
  name.textContent = session.name;
  name.title = session.name;
  name.style.cursor = 'pointer';
  name.addEventListener('click', () => {
    window.location.hash = `#/s/${session.id}`;
  });
  head.appendChild(name);
  const spacer = document.createElement('span');
  spacer.className = 'spacer';
  head.appendChild(spacer);
  head.appendChild(statusBadge(session));
  card.appendChild(head);

  const meta = document.createElement('div');
  meta.className = 'session-meta';
  const lines = [
    `cmd  ${session.cmd}`,
    `cwd  ${session.cwd}`,
    `id   ${session.id}`,
    `created ${session.created_at}`,
  ];
  if (session.setup_script) lines.push(`setup ${session.setup_script}`);
  for (const line of lines) {
    const div = document.createElement('div');
    div.textContent = line;
    meta.appendChild(div);
  }
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
    mk('kill', 'danger', 'Terminate the shell (session stays listed)', () =>
      run(() => api.killSession(session.id)));
  } else {
    mk('respawn', '', 'Spawn a fresh shell', () =>
      run(() => api.respawnSession(session.id)));
  }
  mk('clean', '', "Reset this session's replay log", () =>
    run(() => api.cleanSession(session.id)));
  mk('delete', 'danger', 'Delete the session and its state', () => {
    if (window.confirm(`Delete session "${session.name}"? Its state and log are removed.`)) {
      return run(() => api.deleteSession(session.id));
    }
    return undefined;
  });
  card.appendChild(actions);
  return card;
}

async function run(action) {
  try {
    await action();
    await refresh();
  } catch (err) {
    // 401 already raised webterm:unauthorized — the gate handles the UX.
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
  await refresh();
  pollTimer = setInterval(() => {
    refresh().catch((err) => {
      if (!(err instanceof AuthError)) console.warn('poll failed:', err);
    });
  }, POLL_MS);
}

export function unmount() {
  if (pollTimer !== null) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
  closeModal();
}
