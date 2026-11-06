## UC-004 Bearer-token fallback (curl/CLI/WS)

`Authorization: Bearer <token>` on `/api/*` and `?token=` on `/api/sessions/{id}/ws` keep working unchanged — curl, `dream ps`, `dream doctor`, and the existing e2e suite authenticate exactly as before, whether or not OAuth is configured.
