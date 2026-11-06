## UC-001 Login with GitHub

Owner opens the dashboard, sees "Sign in with GitHub" on the gate view, and clicks it. `GET /auth/github` 302s to github.com/login/oauth/authorize (client_id + redirect_uri + random `state` in a short-lived cookie). After consent, `GET /auth/callback` verifies `state`, exchanges the code, fetches the user, and sets `dream_session` (HttpOnly, SameSite=Lax, ~30d; `Secure` only when the effective scheme is https via `X-Forwarded-Proto`) — subsequent `/api/*` calls and the WS upgrade authenticate via the cookie, so no token paste is needed on revisit.
