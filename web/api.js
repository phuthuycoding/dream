// api.js — token storage + REST/WS helpers.
//
// Token UX (FR-007): `/?token=T` → localStorage, stripped from the URL;
// every REST call carries `Authorization: Bearer T`; WS uses `?token=T`
// because the browser WebSocket API cannot set headers.

const TOKEN_KEY = 'dream.token';
const ATTENTION_ALL = 'dream.attention'; // JSON map {id: true}

export class AuthError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AuthError';
  }
}

export class ApiError extends Error {
  constructor(status, message) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

export function getToken() {
  return localStorage.getItem(TOKEN_KEY) || '';
}

export function setToken(token) {
  localStorage.setItem(TOKEN_KEY, token);
}

export function clearToken() {
  localStorage.removeItem(TOKEN_KEY);
}

// Pull `?token=` out of the URL into localStorage and remove it from the
// address bar so it is not copied into links or left in history.
export function adoptTokenFromUrl() {
  const url = new URL(window.location.href);
  const token = url.searchParams.get('token');
  if (!token) return;
  setToken(token);
  url.searchParams.delete('token');
  history.replaceState(null, '', url.pathname + url.search + url.hash);
}

async function request(path, { method = 'GET', body } = {}) {
  const resp = await fetch(path, {
    method,
    headers: {
      Authorization: `Bearer ${getToken()}`,
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (resp.status === 401) {
    clearToken();
    // Listeners (app.js) drop back to the auth gate no matter which
    // code path — poll, click, attach — surfaced the rejection.
    window.dispatchEvent(new CustomEvent('dream:unauthorized'));
    throw new AuthError('unauthorized — token rejected or missing');
  }
  if (!resp.ok) {
    const text = await resp.text().catch(() => '');
    throw new ApiError(resp.status, `${method} ${path} → ${resp.status} ${text}`.trim());
  }
  if (resp.status === 204) return null;
  return resp.json();
}

// `GET /auth/mode` → {github: bool} — public probe telling the gate whether
// the daemon was started with GitHub OAuth configured. Not routed through
// `request()`: it needs no Authorization header and must not trip the 401
// gate-clearing path.
export async function oauthMode() {
  const resp = await fetch('/auth/mode');
  if (!resp.ok) {
    throw new ApiError(resp.status, `GET /auth/mode → ${resp.status}`);
  }
  return resp.json();
}

// `POST /auth/logout` expires the `dream_session` cookie server-side; the
// response is a 302 to `/`, so this is a raw fetch, not `request()`'s JSON
// path. A non-2xx means the cookie survived — reject so the caller can
// warn instead of letting the user believe the logout landed.
export async function logout() {
  const resp = await fetch('/auth/logout', { method: 'POST' });
  if (!resp.ok) {
    throw new ApiError(resp.status, `POST /auth/logout → ${resp.status}`);
  }
}

export const api = {
  listSessions: () => request('/api/sessions'),
  createSession: (fields) =>
    request('/api/sessions', { method: 'POST', body: fields }),
  getSession: (id) => request(`/api/sessions/${id}`),
  patchSession: (id, fields) =>
    request(`/api/sessions/${id}`, { method: 'PATCH', body: fields }),
  deleteSession: (id) => request(`/api/sessions/${id}`, { method: 'DELETE' }),
  killSession: (id) => request(`/api/sessions/${id}/kill`, { method: 'POST' }),
  respawnSession: (id) =>
    request(`/api/sessions/${id}/respawn`, { method: 'POST' }),
  cleanSession: (id) =>
    request(`/api/sessions/${id}/clean`, { method: 'POST' }),
  cleanAllLogs: () => request('/api/logs/clean', { method: 'POST' }),
};

export function wsUrl(sessionId) {
  const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
  const token = encodeURIComponent(getToken());
  return `${proto}//${window.location.host}/api/sessions/${sessionId}/ws?token=${token}`;
}

// Attention flags are per-session and persist across attaches so the
// dashboard can show a dot for sessions that rang the bell / signalled
// while the owner was elsewhere (FR-014). Attaching clears the flag.
function readAttentionMap() {
  try {
    return JSON.parse(localStorage.getItem(ATTENTION_ALL) || '{}');
  } catch {
    return {};
  }
}

export function setAttention(sessionId) {
  const map = readAttentionMap();
  map[sessionId] = true;
  localStorage.setItem(ATTENTION_ALL, JSON.stringify(map));
}

export function clearAttention(sessionId) {
  const map = readAttentionMap();
  if (map[sessionId]) {
    delete map[sessionId];
    localStorage.setItem(ATTENTION_ALL, JSON.stringify(map));
  }
}

export function hasAttention(sessionId) {
  return Boolean(readAttentionMap()[sessionId]);
}
