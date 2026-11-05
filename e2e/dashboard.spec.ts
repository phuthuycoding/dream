// dashboard.spec.ts — TC-017 (auth gate + session list) and TC-022
// (kill / rename / delete / attention badge through the real UI).

// NOTE: all specs run behind installAuthShim — see helpers.ts BUG-E2E-001.
// Without it the product cannot serve its own assets to any browser.

import {
  test,
  expect,
  attachTerminal,
  createSession,
  getSession,
  installAuthShim,
  newInstrumentedPage,
  openDashboard,
  rest,
  runInTerminal,
} from './helpers';

test('TC-017: auth gate first, then dashboard lists sessions', async ({ context, daemon }) => {
  await installAuthShim(context, daemon.token);
  const page = await context.newPage();

  // No token → gate, not the dashboard.
  await page.goto(`${daemon.baseUrl}/`);
  await expect(page.locator('#view-gate')).toBeVisible();
  await expect(page.locator('#view-dashboard')).toBeHidden();
  await expect(page.locator('#session-list')).toBeEmpty();

  // A bad token is rejected and the gate explains why.
  await page.fill('#gate-token', 'definitely-wrong');
  await page.locator('#gate-form button[type=submit]').click();
  await expect(page.locator('#gate-error')).toBeVisible();
  await expect(page.locator('#view-gate')).toBeVisible();

  // The real token opens the dashboard.
  await page.fill('#gate-token', daemon.token);
  await page.locator('#gate-form button[type=submit]').click();
  await expect(page.locator('#view-dashboard')).toBeVisible();
  await expect(page.locator('#dashboard-empty')).toBeVisible();

  // A session created server-side appears on the next poll (3s cadence).
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'dash-listed' });
  const card = page.locator('.session-card', { hasText: 'dash-listed' });
  await expect(card).toBeVisible();
  await expect(card.locator('.badge')).toHaveText('live');
  // FR-006: cards show the short id (first 8 chars); full id is in the title attr.
  await expect(card.locator('.session-meta')).toContainText(s.id.slice(0, 8));
});

test('TC-022: kill, rename, delete through the UI; bell raises the attention badge', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  const page = await newInstrumentedPage(context);
  await openDashboard(page, daemon.baseUrl, daemon.token);

  // Create through the UI modal — lands on the terminal view.
  await page.locator('#btn-new').click();
  await page.locator('#f-name').fill('life-cycle');
  await page.locator('#modal-submit').click();
  await expect(page.locator('#view-terminal')).toBeVisible();
  const id = /#\/s\/([0-9a-f-]+)/.exec(page.url())![1];
  await page.locator('#btn-back').click();
  await expect(page.locator('#view-dashboard')).toBeVisible();

  const card = page.locator(`.session-card[data-session-id="${id}"]`);
  await expect(card).toBeVisible();
  await expect(card.locator('.badge')).toHaveText('live');

  // Kill via UI → dead badge, still listed, respawn offered.
  await card.locator('button', { hasText: 'kill' }).click();
  await expect(card.locator('.badge')).toHaveText('dead');
  await expect(card.locator('button', { hasText: 'respawn' })).toBeVisible();
  expect((await getSession(daemon.baseUrl, daemon.token, id)).status).toBe('Dead');

  // Rename via the edit modal.
  await card.locator('button', { hasText: 'edit' }).click();
  await expect(page.locator('#modal')).toBeVisible();
  await page.locator('#f-name').fill('renamed-session');
  await page.locator('#modal-submit').click();
  await expect(card.locator('.session-name')).toHaveText('renamed-session');
  expect((await getSession(daemon.baseUrl, daemon.token, id)).name).toBe('renamed-session');

  // Delete via UI (confirm dialog accepted) → card gone, session gone.
  page.on('dialog', (d) => void d.accept());
  await card.locator('button', { hasText: 'delete' }).click();
  await expect(card).toHaveCount(0);
  const remaining = await rest(daemon.baseUrl, daemon.token, 'GET', '/api/sessions');
  expect(remaining.map((s: any) => s.id)).not.toContain(id);

  // Attention: a BEL in session output while attached sets the flag + the
  // ● title. The dashboard dot is a *cross-tab* signal — unmount() clears
  // the flag when navigating back, so a second page viewing the dashboard
  // is what legitimately observes it.
  const s2 = await createSession(daemon.baseUrl, daemon.token, { name: 'bell-ringer' });
  await attachTerminal(page, daemon.baseUrl, daemon.token, s2.id);
  await runInTerminal(page, "printf '\\a'");
  await expect(page).toHaveTitle(/^● /, { timeout: 15_000 });

  const page2 = await context.newPage();
  await openDashboard(page2, daemon.baseUrl, daemon.token);
  await expect(
    page2.locator(`.session-card[data-session-id="${s2.id}"] .attention-dot`),
  ).toBeVisible();
});

test('TC-017b: sessions created and attached through the dashboard flow', async ({
  context,
  daemon,
}) => {
  await installAuthShim(context, daemon.token);
  // Second look at the dashboard list: create via REST, attach via the
  // card's attach button, confirm the terminal view mounts and back works.
  const s = await createSession(daemon.baseUrl, daemon.token, { name: 'attach-via-card' });
  const page = await newInstrumentedPage(context);
  await openDashboard(page, daemon.baseUrl, daemon.token);
  const card = page.locator(`.session-card[data-session-id="${s.id}"]`);
  await expect(card).toBeVisible();
  await card.locator('button', { hasText: 'attach' }).click();
  await expect(page.locator('#view-terminal')).toBeVisible();
  await expect(page.locator('#term-title')).toHaveText('attach-via-card');
  await expect(page.locator('#term-status')).toHaveText('connected');
});
