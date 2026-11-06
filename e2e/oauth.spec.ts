// oauth.spec.ts — the gate's "Sign in with GitHub" affordance tracks the
// daemon's oauth configuration, and the bearer-token path keeps working
// beside it. Placeholder credentials suffice: these tests assert the
// button, the /auth/mode probe and the /auth/github redirect target —
// none of them contact github.com.

import { test, expect } from './helpers';

const OAUTH_ARGS = [
  '--github-client-id',
  'e2e-client-id',
  '--github-client-secret',
  'e2e-client-secret',
  '--github-allowlist',
  'e2e-user',
];

// A DREAM_GITHUB_* variable on the runner would configure oauth even when
// the test passes no flags — scrub it from the spawned daemon's env.
const GITHUB_ENV_VARS = [
  'DREAM_GITHUB_CLIENT_ID',
  'DREAM_GITHUB_CLIENT_SECRET',
  'DREAM_GITHUB_ALLOWLIST',
];

test.describe('daemon without oauth flags', () => {
  test.use({ daemonOpts: { unsetEnv: GITHUB_ENV_VARS } });

  test('gate hides the GitHub sign-in button and the mode probe reports github:false', async ({
    page,
    daemon,
  }) => {
    const modeProbe = page.waitForResponse('**/auth/mode');
    await page.goto(`${daemon.baseUrl}/`);
    await expect(page.locator('#view-gate')).toBeVisible();

    // Wait for the gate's own probe so a hidden button can't pass before
    // the fetch that would have revealed it resolves.
    const mode = await (await modeProbe).json();
    expect(mode).toEqual({ github: false });
    await expect(page.locator('#gate-github')).toBeHidden();
    await expect(page.locator('#btn-github')).toBeHidden();

    // The token form is the only way in.
    await expect(page.locator('#gate-token')).toBeVisible();

    // The login start route is absent too — the href would 404.
    const resp = await page.request.get(`${daemon.baseUrl}/auth/github`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(404);
  });
});

test.describe('daemon with oauth flags', () => {
  test.use({ daemonOpts: { args: OAUTH_ARGS, unsetEnv: GITHUB_ENV_VARS } });

  test('gate shows a GitHub sign-in link to the login start and the mode probe reports github:true', async ({
    page,
    daemon,
  }) => {
    const modeProbe = page.waitForResponse('**/auth/mode');
    await page.goto(`${daemon.baseUrl}/`);
    await expect(page.locator('#view-gate')).toBeVisible();

    const mode = await (await modeProbe).json();
    expect(mode).toEqual({ github: true });
    await expect(page.locator('#gate-github')).toBeVisible();
    const btn = page.locator('#btn-github');
    await expect(btn).toHaveText('Sign in with GitHub');
    await expect(btn).toHaveAttribute('href', '/auth/github');

    // The start route is live: it 302s to github.com's authorize page —
    // asserted without following, so the suite never leaves localhost.
    const resp = await page.request.get(`${daemon.baseUrl}/auth/github`, {
      maxRedirects: 0,
    });
    expect(resp.status()).toBe(302);
    expect(resp.headers()['location']).toContain('github.com/login/oauth/authorize');
  });

  test('pasting the bearer token still signs in when oauth is configured', async ({
    page,
    daemon,
  }) => {
    await page.goto(`${daemon.baseUrl}/`);
    await expect(page.locator('#view-gate')).toBeVisible();
    // Both paths are offered at once.
    await expect(page.locator('#btn-github')).toBeVisible();
    await expect(page.locator('#gate-token')).toBeVisible();

    await page.fill('#gate-token', daemon.token);
    await page.locator('#gate-form button[type=submit]').click();
    await expect(page.locator('#view-gate')).toBeHidden();
    await expect(page.locator('#view-home')).toBeVisible();
    // The session-list sidebar mounts once authed.
    await expect(page.locator('#view-dashboard')).toBeVisible();
  });
});
