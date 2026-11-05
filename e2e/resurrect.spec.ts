// resurrect.spec.ts — brief TC-020 / plan TC-021: kill the daemon, restart
// it on the same data dir, and every previously-live session comes back
// marked "resurrected" with its screen restored and a working shell
// (setup script re-ran).

import { rmSync } from 'node:fs';

import {
  test,
  expect,
  attachTerminal,
  bootDaemon,
  createSession,
  getFreePort,
  getSession,
  installAuthShim,
  mkDataDir,
  newInstrumentedPage,
  openDashboard,
  runInTerminal,
  stopDaemon,
  waitForSnapshotText,
  waitForWsText,
} from './helpers';

test('daemon restart resurrects live sessions: badge, restored screen, working shell', async ({
  context,
}) => {
  const dataDir = mkDataDir();
  const port = await getFreePort();
  let daemon = await bootDaemon({ dataDir, port });
  // The restart reuses the same data dir → same token, so one shim covers
  // both daemon incarnations (BUG-E2E-001 — see helpers.ts).
  await installAuthShim(context, daemon.token);
  try {
    const s = await createSession(daemon.baseUrl, daemon.token, {
      name: 'zombie',
      setup_script: 'echo SETUP-RAN-MARK',
    });

    const page1 = await newInstrumentedPage(context);
    await attachTerminal(page1, daemon.baseUrl, daemon.token, s.id);
    await waitForWsText(page1, 'SETUP-RAN-MARK');
    await runInTerminal(page1, 'echo BEFORE-KILL-MARK');
    await waitForWsText(page1, 'BEFORE-KILL-MARK');
    await page1.close();

    // Hard kill + restart on the same port and data dir.
    await stopDaemon(daemon, /* keepDataDir */ true);
    daemon = await bootDaemon({ dataDir, port });

    // Registry reports the resurrected flag (status itself stays Live —
    // the flag is what the dashboard badge reads).
    const restored = await getSession(daemon.baseUrl, daemon.token, s.id);
    expect(restored.resurrected).toBe(true);
    expect(restored.status).toBe('Live');

    // …and the dashboard renders the resurrected badge.
    const page2 = await newInstrumentedPage(context);
    await openDashboard(page2, daemon.baseUrl, daemon.token);
    const card = page2.locator(`.session-card[data-session-id="${s.id}"]`);
    await expect(card).toBeVisible();
    await expect(card.locator('.badge')).toHaveText('resurrected');

    // Attach: the retained log replays the pre-kill output (the user's
    // screen is restored), the resurrected shell re-ran its setup script,
    // and it accepts input.
    await attachTerminal(page2, daemon.baseUrl, daemon.token, s.id);
    await waitForWsText(page2, 'BEFORE-KILL-MARK');
    await waitForWsText(page2, 'SETUP-RAN-MARK');
    await runInTerminal(page2, 'echo POST-RES-MARK');
    await waitForWsText(page2, 'POST-RES-MARK');

    // Reattach once the new screen has content: the snapshot determin-
    // istically repaints the current (post-resurrect) screen.
    await page2.reload();
    await attachTerminal(page2, daemon.baseUrl, daemon.token, s.id);
    await waitForSnapshotText(page2, 'POST-RES-MARK');
  } finally {
    await stopDaemon(daemon, true);
    rmSync(dataDir, { recursive: true, force: true });
  }
});
