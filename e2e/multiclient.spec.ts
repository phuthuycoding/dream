// multiclient.spec.ts — brief TC-021: two browsers attached to the same
// session see the same output; a session whose shell dies offers respawn
// and comes back usable; "clean all logs" truncates the replay log while
// the terminal keeps working (plan TC-016/024/025 as E2E).

import { statSync } from 'node:fs';
import path from 'node:path';

import {
  test,
  expect,
  attachTerminal,
  createSession,
  getSession,
  installAuthShim,
  killSession,
  newInstrumentedPage,
  openDashboard,
  runInTerminal,
  waitForWsText,
} from './helpers';

test('two browsers on one session see the same output', async ({ context, browser, daemon }) => {
  await installAuthShim(context, daemon.token); // BUG-E2E-001 — see helpers.ts
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'multi' });

  const pageA = await newInstrumentedPage(context);
  await attachTerminal(pageA, daemon.baseUrl, daemon.token, s.id);

  const ctx2 = await browser.newContext();
  await installAuthShim(ctx2, daemon.token);
  try {
    const pageB = await newInstrumentedPage(ctx2);
    await attachTerminal(pageB, daemon.baseUrl, daemon.token, s.id);

    // Both clients get the retained scrollback + snapshot on attach…
    await runInTerminal(pageA, 'echo SHARED-SCREEN');
    await waitForWsText(pageA, 'SHARED-SCREEN');
    await waitForWsText(pageB, 'SHARED-SCREEN');

    // …and both can drive the same shell: B's input echoes back to both.
    await runInTerminal(pageB, 'echo FROM-SECOND-CLIENT');
    await waitForWsText(pageA, 'FROM-SECOND-CLIENT');
    await waitForWsText(pageB, 'FROM-SECOND-CLIENT');
  } finally {
    await ctx2.close();
  }
});

test('dead shell → end overlay offers respawn → fresh shell works', async ({ context, daemon }) => {
  await installAuthShim(context, daemon.token);
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'mortal' });
  const page = await newInstrumentedPage(context);
  await attachTerminal(page, daemon.baseUrl, daemon.token, s.id);
  await runInTerminal(page, 'echo BEFORE-DEATH');
  await waitForWsText(page, 'BEFORE-DEATH');

  // Kill the shell underneath the attached client: the server pushes the
  // end frame and the UI offers respawn.
  await killSession(daemon.baseUrl, daemon.token, s.id);
  await expect(page.locator('#term-overlay')).toBeVisible();
  await expect(page.locator('#overlay-text')).toContainText('respawn');
  expect((await getSession(daemon.baseUrl, daemon.token, s.id)).status).toBe('Dead');

  // BUG-E2E-002 (product bug, reported): #term-overlay has no z-index, so
  // xterm's inner layers (z-index up to 11) cover it and its buttons can't
  // be clicked. The header respawn button — also unhidden by the `e`
  // frame — sits outside #term-wrap and works.
  await page.locator('#btn-term-respawn').click();
  await expect(page.locator('#term-overlay')).toBeHidden();
  await expect(page.locator('#term-status')).toHaveText('connected');
  expect((await getSession(daemon.baseUrl, daemon.token, s.id)).status).toBe('Live');
  await runInTerminal(page, 'echo RESPAWNED-OK');
  await waitForWsText(page, 'RESPAWNED-OK');
});

test('clean all logs truncates the replay log; terminal keeps working', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'loggy' });
  const page = await newInstrumentedPage(context);
  await attachTerminal(page, daemon.baseUrl, daemon.token, s.id);
  await runInTerminal(page, 'seq 1 200 && echo SEQ-DONE');
  await waitForWsText(page, 'SEQ-DONE');
  await runInTerminal(page, 'echo BEFORE-CLEAN-MARK');
  await waitForWsText(page, 'BEFORE-CLEAN-MARK');

  const logPath = path.join(daemon.dataDir, 'sessions', s.id, 'replay.cast');
  const sizeBefore = statSync(logPath).size;
  expect(sizeBefore).toBeGreaterThan(100);

  // Dashboard → clean all logs (accept the confirm dialog).
  page.on('dialog', (d) => void d.accept());
  await openDashboard(page, daemon.baseUrl, daemon.token);
  await page.locator('#btn-clean-all').click();
  await expect.poll(() => statSync(logPath).size).toBeLessThan(sizeBefore);

  // Reattach: the terminal still connects and the shell still answers.
  await attachTerminal(page, daemon.baseUrl, daemon.token, s.id);
  await runInTerminal(page, 'echo AFTER-CLEAN-OK');
  await waitForWsText(page, 'AFTER-CLEAN-OK');
});
