// reattach.spec.ts — TC-019: closing the browser leaves the shell alive;
// the next attach replays retained scrollback and restores the exact
// screen via the snapshot. A second browser attaching the same session
// mirrors the same stream.

import {
  test,
  expect,
  attachTerminal,
  createSession,
  getSession,
  installAuthShim,
  newInstrumentedPage,
  runInTerminal,
  waitForSnapshotText,
  waitForWsText,
} from './helpers';

test('TC-019: browser close → reattach replays scrollback and restores screen', async ({
  context,
  browser,
  daemon,
}) => {
  await installAuthShim(context, daemon.token); // BUG-E2E-001 — see helpers.ts
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'sticky' });

  const page1 = await newInstrumentedPage(context);
  await attachTerminal(page1, daemon.baseUrl, daemon.token, s.id);
  await runInTerminal(page1, 'echo KEEP-SCROLLBACK');
  await waitForWsText(page1, 'KEEP-SCROLLBACK');
  await runInTerminal(page1, 'export S=1; echo SCREEN-STATE-$S');
  await waitForWsText(page1, 'SCREEN-STATE-1');

  // Browser tab gone — the shell must keep running server-side.
  await page1.close();
  expect((await getSession(daemon.baseUrl, daemon.token, s.id)).status).toBe('Live');

  // Fresh page, same context (token persists in localStorage): attach.
  const page2 = await newInstrumentedPage(context);
  await attachTerminal(page2, daemon.baseUrl, daemon.token, s.id);

  // Scrollback replay carries the earlier output…
  await waitForWsText(page2, 'KEEP-SCROLLBACK');
  // …and the snapshot repaints the same screen the first client left.
  await waitForSnapshotText(page2, 'SCREEN-STATE-1');
  await waitForSnapshotText(page2, 'KEEP-SCROLLBACK');

  // The shell is still the same one (exported variable survived).
  await runInTerminal(page2, 'echo STILL-$S-ALIVE');
  await waitForWsText(page2, 'STILL-1-ALIVE');

  // A second, independent browser context mirrors the same session.
  const ctx2 = await browser.newContext();
  await installAuthShim(ctx2, daemon.token);
  try {
    const page3 = await newInstrumentedPage(ctx2);
    await attachTerminal(page3, daemon.baseUrl, daemon.token, s.id);
    await waitForSnapshotText(page3, 'SCREEN-STATE-1');

    await runInTerminal(page2, 'echo FANOUT-MARK');
    await waitForWsText(page2, 'FANOUT-MARK');
    await waitForWsText(page3, 'FANOUT-MARK');
  } finally {
    await ctx2.close();
  }
});
