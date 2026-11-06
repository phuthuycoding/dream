// ui-v2.spec.ts — sidebar master-detail UI + keep-alive multi-attach.
//
// Covers: sidebar session list + guided empty state, click-to-attach
// switching with preserved screen state, active-item highlight,
// attention dot, reduced motion, and the 6-live LRU cap.

import {
  test,
  expect,
  attachTerminal,
  createSession,
  getSession,
  installAuthShim,
  newInstrumentedPage,
  openDashboard,
  runInTerminal,
  waitForWsText,
} from './helpers';

const item = (page: any, id: string) =>
  page.locator(`.session-card[data-session-id="${id}"]`);
const panes = (page: any) => page.locator('#term-container .term-pane');

test('sidebar lists sessions; empty state guides creation', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const page = await newInstrumentedPage(context);
  await openDashboard(page, daemon.baseUrl, daemon.token);

  await expect(page.locator('#dashboard-empty')).toBeVisible();
  await page.locator('#btn-empty-new').click();
  await expect(page.locator('#modal')).toBeVisible();
  await page.locator('#modal-cancel').click();

  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'side-row' });
  const card = item(page, s.id);
  await expect(card).toBeVisible();
  await expect(card.locator('.session-name')).toHaveText('side-row');
  await expect(card.locator('.badge')).toHaveText('live');
  await expect(card.locator('.status-dot')).toBeVisible();
  await expect(page.locator('#session-count')).toHaveText('1');
  await expect(page.locator('#dashboard-empty')).toBeHidden();
});

test('clicking a sidebar item attaches; switching keeps screen state', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const a = await createSession(daemon.baseUrl, daemon.token, { name: 'side-a' });
  const b = await createSession(daemon.baseUrl, daemon.token, { name: 'side-b' });
  const page = await newInstrumentedPage(context);

  await attachTerminal(page, daemon.baseUrl, daemon.token, a.id);
  await runInTerminal(page, 'echo MARK-ALPHA');
  await waitForWsText(page, 'MARK-ALPHA');
  await expect(item(page, a.id)).toHaveClass(/active/);

  // Sidebar click on B — same mounted view, in-place switch.
  await item(page, b.id).locator('.session-name').click();
  await expect(page).toHaveURL(new RegExp(`#/s/${b.id}`));
  await expect(page.locator('#term-status')).toHaveText('connected');
  await expect(item(page, b.id)).toHaveClass(/active/);
  await runInTerminal(page, 'echo MARK-BETA');
  await waitForWsText(page, 'MARK-BETA');
  expect(await panes(page).count()).toBe(2);

  const snapshots = await page.evaluate(
    () => (window as any).__ws.ops().filter((op: number) => op === 0x73).length,
  );

  // Sidebar click back to A: pane toggles, no new socket, no replay.
  await item(page, a.id).locator('.session-name').click();
  await expect(page.locator('#term-title')).toHaveText('side-a');
  await expect(page.locator('.xterm:visible')).toHaveCount(1);
  expect(
    await page.evaluate(
      () => (window as any).__ws.ops().filter((op: number) => op === 0x73).length,
    ),
  ).toBe(snapshots);

  // The sidebar itself stays put — it's persistent, not a view.
  await expect(page.locator('#view-dashboard')).toBeVisible();
  await expect(item(page, b.id)).toBeVisible();
});

test('sidebar switches back home via back button; sessions stay live', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const a = await createSession(daemon.baseUrl, daemon.token, { name: 'keep-a' });
  const page = await newInstrumentedPage(context);

  await attachTerminal(page, daemon.baseUrl, daemon.token, a.id);
  await page.locator('#btn-back').click();
  await expect(page.locator('#view-home')).toBeVisible();
  await expect(page.locator('#view-dashboard')).toBeVisible();
  expect((await getSession(daemon.baseUrl, daemon.token, a.id)).status).toBe('Live');
});

test('bell flags the sidebar item', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const a = await createSession(daemon.baseUrl, daemon.token, { name: 'bell-side' });
  const page = await newInstrumentedPage(context);

  await attachTerminal(page, daemon.baseUrl, daemon.token, a.id);
  await runInTerminal(page, "printf '\\a'");
  await expect(page).toHaveTitle(/^● /, { timeout: 15_000 });

  // A second viewer on the dashboard route sees the attention dot.
  const page2 = await context.newPage();
  await openDashboard(page2, daemon.baseUrl, daemon.token);
  await expect(
    item(page2, a.id).locator('.attention-dot'),
  ).toBeVisible();
});

test('reduced motion collapses transitions', async ({ context, daemon }) => {
  await installAuthShim(context, daemon.token);
  const page = await newInstrumentedPage(context);
  await openDashboard(page, daemon.baseUrl, daemon.token);

  await page.emulateMedia({ reducedMotion: 'reduce' });
  const duration = await page.evaluate(
    () => getComputedStyle(document.querySelector('#view-terminal')!).animationDuration,
  );
  expect(parseFloat(duration)).toBeLessThan(0.02);
});

test('LRU cap: a 7th attach evicts the oldest live terminal instance', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const ids: string[] = [];
  for (let i = 0; i < 7; i += 1) {
    const s = await createSession(daemon.baseUrl, daemon.token, { name: `lru-${i}` });
    ids.push(s.id);
  }
  const page = await newInstrumentedPage(context);

  await attachTerminal(page, daemon.baseUrl, daemon.token, ids[0]);
  for (const id of ids.slice(1)) {
    await page.evaluate((x) => {
      window.location.hash = `#/s/${x}`;
    }, id);
    await expect(page.locator('#term-title')).toHaveText(
      new RegExp(`lru-${ids.indexOf(id)}`),
    );
  }

  // Seven attaches but only six live panes — lru-0 was evicted and
  // re-attaches (replays) when selected again.
  expect(await panes(page).count()).toBe(6);
  await item(page, ids[0]).locator('.session-name').click();
  await expect(page.locator('#term-title')).toHaveText('lru-0');
  await expect(page.locator('#term-status')).toHaveText('connected');
  expect(await panes(page).count()).toBe(6);
});
