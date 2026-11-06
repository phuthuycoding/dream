// firstrun.spec.ts — TC-023: first-run token URL + tailscale guidance,
// --public loud warning / non-loopback refusal, restart does not reprint
// the URL, and the binary is self-contained (assets embedded; only system
// libs on darwin).

import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { readFileSync, rmSync, statSync } from 'node:fs';
import path from 'node:path';

import {
  test,
  expect,
  bootDaemon,
  getFreePort,
  mkDataDir,
  stopDaemon,
  DREAM_BIN,
} from './helpers';

interface Spawned {
  proc: ReturnType<typeof spawn>;
  stdoutText: () => string;
  stderrText: () => string;
  exited: Promise<number | null>;
  kill: () => void;
}

function spawnDream(args: string[], cwd?: string): Spawned {
  const proc = spawn(DREAM_BIN, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
    cwd,
  });
  let out = '';
  let err = '';
  proc.stdout!.on('data', (d) => (out += String(d)));
  proc.stderr!.on('data', (d) => (err += String(d)));
  return {
    proc,
    stdoutText: () => out,
    stderrText: () => err,
    exited: once(proc, 'exit').then(([code]) => code as number | null),
    kill: () => proc.kill('SIGKILL'),
  };
}

async function waitForText(
  text: () => string,
  re: RegExp,
  timeoutMs = 15_000,
): Promise<RegExpExecArray> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const m = re.exec(text());
    if (m) return m;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`no match for ${re}; captured:\n${text()}`);
}

test('first run on a pipe reports the token file (TTY still prints the URL); restart reuses the token silently', async () => {
  const dataDir = mkDataDir();
  const port = await getFreePort();
  const first = spawnDream(['--port', String(port), '--data-dir', dataDir]);
  try {
    // Piped stdout is not a TTY — under launchd/systemd that same stream is
    // a world-readable log, so the bearer token must never be printed
    // there. TTY still gets "open this URL once" (interactive terminals).
    await waitForText(first.stdoutText, /dream ready — token file at/);
    expect(first.stdoutText()).not.toMatch(/\?token=/);
    const token = readFileSync(path.join(dataDir, 'token'), 'utf8').trim();
    expect(token.length).toBeGreaterThanOrEqual(32);

    // Persisted token matches; the file is owner-only (0600 on unix).
    const tokenFile = path.join(dataDir, 'token');
    expect(readFileSync(tokenFile, 'utf8').trim()).toBe(token);
    if (process.platform !== 'win32') {
      expect(statSync(tokenFile).mode & 0o777).toBe(0o600);
    }

    // Tailscale guidance is printed (serve recipe or install hint) and
    // never suggests funnel/public exposure.
    const guidance = first.stdoutText();
    expect(guidance).toMatch(/tailscale (serve --bg|not found)/);
    expect(guidance.toLowerCase()).not.toContain('funnel');
  } finally {
    first.kill();
    await first.exited.catch(() => {});
  }

  // Second run on the same data dir: token reused, URL not reprinted.
  const second = spawnDream(['--port', String(port), '--data-dir', dataDir]);
  try {
    await waitForText(second.stdoutText, /dream ready — token file at/);
    expect(second.stdoutText()).not.toMatch(/open this URL once/);
  } finally {
    second.kill();
    await second.exited.catch(() => {});
    rmSync(dataDir, { recursive: true, force: true });
  }
});

// BUG-E2E-001 tracking test (helpers.ts has the full write-up): the
// share-link URL must bootstrap the whole SPA. BUG-E2E-001 (gated
// subresources 401'd, SPA couldn't boot) is fixed — static assets now
// serve unauthenticated while /api/* and WS stay token-gated.
test('share-link URL boots the SPA — BUG-E2E-001 fixed', async ({ page, daemon }) => {
  await page.goto(`${daemon.baseUrl}/?token=${daemon.token}`);
  // app.js adopts the token from the URL into localStorage.
  await expect
    .poll(() => page.evaluate(() => localStorage.getItem('dream.token')))
    .toBe(daemon.token);
  await expect(page.locator('#view-dashboard')).toBeVisible();
});

test('--public prints a loud warning; non-loopback without it refuses to bind', async () => {
  const dataDir = mkDataDir();
  const port = await getFreePort();
  try {
    // Refusal path: exits non-zero before binding.
    const refused = spawnDream([
      '--bind',
      '0.0.0.0',
      '--port',
      String(port),
      '--data-dir',
      dataDir,
    ]);
    const code = await Promise.race([
      refused.exited,
      new Promise<null>((r) => setTimeout(() => r(null), 10_000)),
    ]);
    expect(code).not.toBeNull();
    expect(code).not.toBe(0);
    expect(refused.stderrText()).toMatch(/refusing to bind non-loopback/);
    refused.kill();

    // --public: loud stderr warning, then it binds and serves.
    const loud = spawnDream([
      '--bind',
      '0.0.0.0',
      '--port',
      String(port),
      '--data-dir',
      dataDir,
      '--public',
    ]);
    try {
      await waitForText(loud.stderrText, /WARNING: binding 0\.0\.0\.0 exposes dream/);
      await waitForText(loud.stdoutText, /dream ready/);
    } finally {
      loud.kill();
      await loud.exited.catch(() => {});
    }
  } finally {
    rmSync(dataDir, { recursive: true, force: true });
  }
});

test('self-contained binary: serves the UI from a bare cwd; links only system libs', async () => {
  // rust-embed packs web/ into the binary — no sibling asset dir needed.
  const bare = mkDataDir();
  const daemon = await bootDaemon({ cwd: bare });
  try {
    const resp = await fetch(`${daemon.baseUrl}/?token=${daemon.token}`);
    expect(resp.status).toBe(200);
    const html = await resp.text();
    expect(html).toContain('<title>dream</title>');
    expect(html).toContain('/app.js');
  } finally {
    await stopDaemon(daemon);
    rmSync(bare, { recursive: true, force: true });
  }

  // Packaging sanity (macOS): only system libraries, nothing from the
  // build tree or a bundled dylib.
  if (process.platform === 'darwin') {
    const out = execFileSync('otool', ['-L', DREAM_BIN], { encoding: 'utf8' });
    const libs = out
      .split('\n')
      .slice(1)
      .map((l) => l.trim().split(' ')[0])
      .filter(Boolean);
    expect(libs.length).toBeGreaterThan(0);
    for (const lib of libs) {
      expect(lib).toMatch(/^\/(usr\/lib|System\/Library)/);
    }
  }
});
