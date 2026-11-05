// attach.spec.ts — TC-018: create a session through the UI, attach, type a
// command, see its echo/output, and verify resize propagates to the pty.

import {
  test,
  expect,
  getSession,
  installAuthShim,
  lastSentResize,
  newInstrumentedPage,
  openDashboard,
  runInTerminal,
  sentResizePairs,
  waitForSentOp,
  waitForWsText,
} from './helpers';

test('TC-018: create + attach + echo + resize propagates', async ({ context, daemon }) => {
  await installAuthShim(context, daemon.token); // BUG-E2E-001 — see helpers.ts
  const page = await newInstrumentedPage(context);
  await openDashboard(page, daemon.baseUrl, daemon.token);

  // Create through the modal; the router lands on the attach view.
  await page.locator('#btn-new').click();
  await expect(page.locator('#modal')).toBeVisible();
  await page.locator('#f-name').fill('echo-room');
  await page.locator('#modal-submit').click();
  await expect(page.locator('#view-terminal')).toBeVisible();
  await expect(page.locator('.xterm')).toBeVisible();
  await expect(page.locator('#term-status')).toHaveText('connected');

  const id = /#\/s\/([0-9a-f-]+)/.exec(page.url())![1];
  expect((await getSession(daemon.baseUrl, daemon.token, id)).status).toBe('Live');

  // Type a command — its echo + output reach the client over the live `o`
  // stream, i.e. exactly what the xterm emulator rendered.
  await runInTerminal(page, 'echo ECHOMARK-42');
  await waitForWsText(page, 'ECHOMARK-42');
  await waitForSentOp(page, 0x69); // 'i' input frames went to the pty

  // Resize the viewport → fit addon → `r` frame → server resizes the pty.
  await page.setViewportSize({ width: 1100, height: 750 });
  await waitForSentOp(page, 0x72);
  const geometry = await lastSentResize(page);
  expect(geometry).not.toBeNull();

  // The pty now reports a geometry the client pushed. (Fit can fire more
  // than once while layout settles, so the assertion is membership in the
  // sent r-frame set rather than equality with the last one.)
  // Tag the output so other "dd dd" text in scrollback (timestamps)
  // cannot shadow the real stty pair.
  const lastSttyPair = () =>
    page.evaluate(() => {
      const m = /SZ\[(\d+) (\d+)\]/g;
      const text = (window as any).__ws.text();
      let last: string | null = null;
      for (let mm = m.exec(text); mm; mm = m.exec(text)) last = `${mm[1]} ${mm[2]}`;
      return last;
    });
  await runInTerminal(page, "stty size | sed 's/^/SZ[/' | sed 's/$/]/'");
  await expect.poll(lastSttyPair).not.toBeNull();
  const stty = await lastSttyPair();
  expect(await sentResizePairs(page)).toContain(stty);
});
