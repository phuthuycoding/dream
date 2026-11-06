## UC-002 Allowlist gate for GitHub identities

`--github-allowlist` holds comma-separated GitHub logins and `org:NAME` entries. After the code exchange the callback matches the user's `login` (or org memberships via `/user/orgs`) against the list; a non-allowlisted identity gets 403 and no session cookie.
