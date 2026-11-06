## UC-003 Logout and session expiry

The dashboard logout button POSTs `/auth/logout`, which expires `dream_session` and returns the owner to the gate. An expired or tampered cookie is treated as absent — the next `/api` probe 401s and the gate view appears.
