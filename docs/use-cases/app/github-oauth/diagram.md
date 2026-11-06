# Use case diagram — github-oauth

```
Owner ──(Sign in with GitHub)──────────► [UC-001] /auth/github → github
                                                authorize → /auth/callback
                                                → dream_session cookie
Owner ──(github login not allowed)─────► [UC-002] callback → 403, no cookie
Owner ──(logout / cookie expires)──────► [UC-003] POST /auth/logout → gate
Owner ──(curl Bearer / CLI / ?token=)──► [UC-004] bearer fallback still works

GitHub ──(authorize + code exchange)──► [UC-001] (external service)
```
