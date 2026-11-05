// mobile.spec.ts — brief TC-022 (plan TC-020 step 3): the terminal is
// usable on a phone-sized viewport — fits without horizontal scroll,
// soft-key row visible, input works.

import { devices } from '@playwright/test';

import {
  test,
  expect,
  attachTerminal,
  createSession,
  installAuthShim,
  newInstrumentedPage,
  runInTerminal,
  waitForWsText,
} from './helpers';

// iPhone geometry/touch, but on Chromium — the suite only vendors Chromium.
test.use({ ...devices['iPhone 13'], defaultBrowserType: 'chromium' });

test('iPhone viewport: terminal fits, soft keys show, input round-trips', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token); // BUG-E2E-001 — see helpers.ts
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'pocket' });
  const page = await newInstrumentedPage(context);
  await attachTerminal(page, daemon.baseUrl, daemon.token, s.id);

  // Soft-key row is visible on small/touch viewports…
  await expect(page.locator('#softkeys')).toBeVisible();
  // …and nothing forces horizontal scrolling.
  const overflow = await page.evaluate(
    () => document.documentElement.scrollWidth - window.innerWidth,
  );
  expect(overflow).toBeLessThanOrEqual(1);

  // Soft keys dispatch input (pointerdown path): Esc reaches the pty.
  await page.locator('#softkeys .softkey[data-key="esc"]').tap();
  await expect
    .poll(() => page.evaluate(() => (window as any).__ws?.sentOps()?.includes(0x69) ?? false))
    .toBe(true);

  // Real typing works and echoes back.
  await runInTerminal(page, 'echo MOBILE-OK');
  await waitForWsText(page, 'MOBILE-OK');
});
