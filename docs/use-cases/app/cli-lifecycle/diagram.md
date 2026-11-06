# Use case diagram — cli-lifecycle

```
Owner ──(dream up)─────────────► [UC-001] install+start launchd agent
Owner ──(dream down/status/restart)► [UC-002] service lifecycle
Owner ──(dream logs/url/ps/doctor) ► [UC-003] observe service + sessions
Owner ──(browser)──────────────► [UC-004] polished dashboard
Owner ──(dream --help/binary)──► [UC-005] renamed product
```
