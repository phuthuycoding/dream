---
feature: web-terminal
context: app
created: 20261105_1911
status: archived
---
# Use Case Diagram

```mermaid
flowchart LR
  O[Owner] --> UC1[UC-001 Open dashboard, list sessions]
  O --> UC2[UC-002 Create session, attach]
  O --> UC3[UC-003 Reattach from another device]
  O --> UC5[UC-005 Kill, rename/edit, delete]
  O --> UC6[UC-006 First run: auth, tailscale guidance]
  O --> UC8[UC-008 Clean replay logs]
  D[Daemon] --> UC4[UC-004 Resurrect sessions on restart]
  D --> UC7[UC-007 Run setup script on spawn]
  UC3 -.->|same session| UC2
  UC4 -.->|re-runs| UC7
```

- Actors: Owner (single human, any browser in tailnet), Daemon (the webterm binary)
- Use cases: UC-001..UC-008 as indexed
- Relationships: reattach returns to the session created in UC-002; resurrection re-executes the UC-007 setup script
