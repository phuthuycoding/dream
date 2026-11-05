// helpers.ts — daemon lifecycle, REST client, and browser-side WS probing
// for the dream E2E suite.
//
// Screen-content strategy: xterm renders through the WebGL/canvas addon, so
// there is no `.xterm-rows` text layer to assert on. Instead every
// instrumented page wraps window.WebSocket and records the binary frames
// the client receives (o/s/e/a) and sends (i/r). `wsText(page)` is exactly
// the byte stream the xterm emulator consumed — asserting on it is as close
// to "what the user sees" as the DOM allows. Structural DOM checks
// (.xterm present, status pill, overlay) cover the rest.

import { spawn, ChildProcess } from 'node:child_process';
import { once } from 'node:events';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';

import base, { expect, BrowserContext, Page } from '@playwright/test';

export const DREAM_BIN = path.resolve(__dirname, '..', 'target', 'debug', 'dream');

// ---------------------------------------------------------------- daemon --

export interface Daemon {
  proc: ChildProcess;
  port: number;
  dataDir: string;
  token: string;
  baseUrl: string;
  stdout: string[];
  stderr: string[];
}

export function mkDataDir(): string {
  return mkdtempSync(path.join(tmpdir(), 'dream-e2e-'));
}

export async function getFreePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = net.createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const addr = srv.address();
      srv.close(() => {
        if (typeof addr === 'object' && addr) resolve(addr.port);
        else reject(new Error('could not allocate an ephemeral port'));
      });
    });
  });
}

async function portAnswers(port: number): Promise<boolean> {
  try {
    const resp = await fetch(`http://127.0.0.1:${port}/api/sessions`);
    return resp.status === 401 || resp.status === 200;
  } catch {
    return false;
  }
}

/** Poll until the daemon on `port` answers HTTP (401 or 200). */
export async function waitForServer(port: number, timeoutMs = 15_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await portAnswers(port)) return;
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(`port ${port} did not answer within ${timeoutMs}ms`);
}

export interface BootOptions {
  dataDir?: string;
  port?: number;
  args?: string[];
  cwd?: string;
  timeoutMs?: number;
}

/** Spawn the real binary; resolves once the token file exists and the port
 *  answers HTTP. stdout/stderr are retained on the returned handle. */
export async function bootDaemon(opts: BootOptions = {}): Promise<Daemon> {
  const dataDir = opts.dataDir ?? mkDataDir();
  const port = opts.port ?? (await getFreePort());
  const stdout: string[] = [];
  const stderr: string[] = [];
  const proc = spawn(
    DREAM_BIN,
    ['--port', String(port), '--data-dir', dataDir, ...(opts.args ?? [])],
    { stdio: ['ignore', 'pipe', 'pipe'], cwd: opts.cwd },
  );
  proc.stdout!.on('data', (d) => stdout.push(...String(d).split('\n')));
  proc.stderr!.on('data', (d) => stderr.push(...String(d).split('\n')));

  const deadline = Date.now() + (opts.timeoutMs ?? 15_000);
  let token = '';
  while (Date.now() < deadline) {
    if (proc.exitCode !== null) {
      throw new Error(
        `dream exited early (code ${proc.exitCode})\nstdout:\n${stdout.join('\n')}\nstderr:\n${stderr.join('\n')}`,
      );
    }
    try {
      token = readFileSync(path.join(dataDir, 'token'), 'utf8').trim();
    } catch {
      /* not written yet */
    }
    if (token && (await portAnswers(port))) {
      return {
        proc,
        port,
        dataDir,
        token,
        baseUrl: `http://127.0.0.1:${port}`,
        stdout,
        stderr,
      };
    }
    await new Promise((r) => setTimeout(r, 50));
  }
  proc.kill('SIGKILL');
  throw new Error(
    `dream did not become ready within ${opts.timeoutMs ?? 15_000}ms\nstdout:\n${stdout.join('\n')}\nstderr:\n${stderr.join('\n')}`,
  );
}

export async function stopDaemon(d: Daemon, keepDataDir = false): Promise<void> {
  if (d.proc.exitCode === null) {
    d.proc.kill('SIGKILL');
    await Promise.race([
      once(d.proc, 'exit'),
      new Promise((r) => setTimeout(r, 5_000)),
    ]);
  }
  if (!keepDataDir) rmSync(d.dataDir, { recursive: true, force: true });
}

// ------------------------------------------------------------------ REST --

export async function rest(
  baseUrl: string,
  token: string,
  method: string,
  route: string,
  body?: unknown,
): Promise<any> {
  const resp = await fetch(`${baseUrl}${route}`, {
    method,
    headers: {
      Authorization: `Bearer ${token}`,
      ...(body !== undefined ? { 'Content-Type': 'application/json' } : {}),
    },
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  if (!resp.ok) {
    throw new Error(`${method} ${route} → ${resp.status} ${await resp.text()}`);
  }
  return resp.status === 204 ? null : resp.json();
}

export const createSession = (baseUrl: string, token: string, fields: Record<string, unknown> = {}) =>
  rest(baseUrl, token, 'POST', '/api/sessions', fields);
export const getSession = (baseUrl: string, token: string, id: string) =>
  rest(baseUrl, token, 'GET', `/api/sessions/${id}`);
export const listSessions = (baseUrl: string, token: string) =>
  rest(baseUrl, token, 'GET', '/api/sessions');
export const killSession = (baseUrl: string, token: string, id: string) =>
  rest(baseUrl, token, 'POST', `/api/sessions/${id}/kill`);
export const respawnSession = (baseUrl: string, token: string, id: string) =>
  rest(baseUrl, token, 'POST', `/api/sessions/${id}/respawn`);

// ---------------------------------------------------------------- browser --

/**
 * BUG-E2E-001 (product bug — reported, not fixed by this suite):
 * `require_token` 401s every request without Bearer or ?token=. A browser
 * can put ?token= only on the share-link document URL — <script src>,
 * <link>, and module imports carry no credential — so /app.js, /style.css
 * and /vendor/* all 401 and the SPA can never boot for a real user.
 *
 * Until the product fixes the auth transport (e.g. a cookie set by the
 * share-link response), this shim injects `Authorization: Bearer <token>`
 * on requests that carry no credential — effectively the fix, so the rest
 * of the suite still exercises the real dashboard/terminal/WS code paths.
 * Requests that already carry Authorization are left untouched, so the
 * gate's wrong-token rejection is still genuinely tested. WS upgrades are
 * not routed by Playwright (they already auth via ?token=).
 * firstrun.spec.ts keeps an unshimmed test.fail tracking the real bug.
 */
export async function installAuthShim(context: BrowserContext, token: string): Promise<void> {
  await context.route('**/*', (route) => {
    const headers = { ...route.request().headers() };
    if (headers['authorization'] === undefined) {
      headers['authorization'] = `Bearer ${token}`;
    }
    void route.continue({ headers });
  });
}

/** Record all WS frames (inbound payloads + outbound opcodes) on
 *  `window.__ws`. Must be installed before the first navigation. */
export async function instrumentWs(page: Page): Promise<void> {
  await page.addInitScript(() => {
    const dec = new TextDecoder('latin1');
    const inbound: Uint8Array[] = [];
    const sent: Uint8Array[] = [];
    const NativeWS = window.WebSocket;
    class RecordingWS extends NativeWS {
      constructor(url: string | URL, protocols?: string | string[]) {
        super(url, protocols);
        this.addEventListener('message', (ev: MessageEvent) => {
          if (ev.data instanceof ArrayBuffer) {
            inbound.push(new Uint8Array(ev.data));
          }
        });
      }
      send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
        if (data instanceof ArrayBuffer) {
          sent.push(new Uint8Array(data));
        } else if (ArrayBuffer.isView(data)) {
          sent.push(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
        }
        super.send(data);
      }
    }
    (window as any).WebSocket = RecordingWS;
    (window as any).__ws = {
      ops: () => inbound.map((f) => f[0]),
      sentOps: () => sent.map((f) => f[0]),
      lastResize: () => {
        const f = sent.filter((x) => x[0] === 0x72).pop();
        return f ? { cols: f[1] * 256 + f[2], rows: f[3] * 256 + f[4] } : null;
      },
      sentResizes: () =>
        sent
          .filter((x) => x[0] === 0x72)
          .map((f) => `${f[3] * 256 + f[4]} ${f[1] * 256 + f[2]}`),
      text: () => inbound.map((f) => dec.decode(f.subarray(1))).join(''),
      snapshotText: () =>
        inbound
          .filter((f) => f[0] === 0x73)
          .map((f) => dec.decode(f.subarray(1)))
          .join(''),
      sawAttention: () => inbound.some((f) => f[0] === 0x61),
      reset: () => {
        inbound.length = 0;
        sent.length = 0;
      },
    };
  });
}

export async function newInstrumentedPage(context: BrowserContext): Promise<Page> {
  const page = await context.newPage();
  await instrumentWs(page);
  return page;
}

/** Navigate so the token lands in localStorage and the router ends at `hash`. */
export async function gotoAuthed(
  page: Page,
  baseUrl: string,
  token: string,
  hash = '#/',
): Promise<void> {
  await page.goto(`${baseUrl}/?token=${token}${hash}`);
}

export async function openDashboard(
  page: Page,
  baseUrl: string,
  token: string,
): Promise<void> {
  await gotoAuthed(page, baseUrl, token, '#/');
  await expect(page.locator('#view-dashboard')).toBeVisible();
}

/** Attach the terminal view and wait until the WS reports connected. */
export async function attachTerminal(
  page: Page,
  baseUrl: string,
  token: string,
  sessionId: string,
): Promise<void> {
  await gotoAuthed(page, baseUrl, token, `#/s/${sessionId}`);
  await expect(page.locator('#view-terminal')).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();
  await expect(page.locator('#term-status')).toHaveText('connected');
}

/** Type a command line into the focused xterm and press Enter. */
export async function runInTerminal(page: Page, line: string): Promise<void> {
  await page.locator('.xterm').click();
  await page.keyboard.type(line);
  await page.keyboard.press('Enter');
}

/** Poll until the bytes the browser's xterm consumed contain `needle`. */
export async function waitForWsText(page: Page, needle: string, timeout = 15_000): Promise<void> {
  await expect
    .poll(
      () => page.evaluate((x) => (window as any).__ws?.text()?.includes(x) ?? false, needle),
      { timeout },
    )
    .toBe(true);
}

/** Poll until a snapshot (`s`) frame's dump contains `needle` — the exact
 *  bytes that repaint the screen on (re)attach. */
export async function waitForSnapshotText(
  page: Page,
  needle: string,
  timeout = 15_000,
): Promise<void> {
  await expect
    .poll(
      () =>
        page.evaluate((x) => (window as any).__ws?.snapshotText()?.includes(x) ?? false, needle),
      { timeout },
    )
    .toBe(true);
}

/** Poll until the client sent at least one frame with this opcode
 *  (0x69 'i' input, 0x72 'r' resize). */
export async function waitForSentOp(page: Page, op: number, timeout = 15_000): Promise<void> {
  await expect
    .poll(() => page.evaluate((x) => (window as any).__ws?.sentOps()?.includes(x) ?? false, op), {
      timeout,
    })
    .toBe(true);
}

/** Last `r` frame the client sent, decoded to {cols, rows}. */
export function lastSentResize(page: Page): Promise<{ cols: number; rows: number } | null> {
  return page.evaluate(() => (window as any).__ws?.lastResize() ?? null);
}

/** Every `r` frame the client sent, as "rows cols" strings (stty format). */
export function sentResizePairs(page: Page): Promise<string[]> {
  return page.evaluate(() => (window as any).__ws?.sentResizes() ?? []);
}

// ---------------------------------------------------------------- fixture --

/** Per-test daemon: fresh data dir + ephemeral port, killed and removed on
 *  teardown. Specs that manage the lifecycle themselves (restart test) boot
 *  manually instead. */
export const test = base.extend<{ daemon: Daemon }>({
  daemon: async ({}, use) => {
    const d = await bootDaemon();
    await use(d);
    await stopDaemon(d);
  },
});

export { expect };
