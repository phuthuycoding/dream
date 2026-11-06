# Use case diagram — ui-v2

```
Owner ──(open home, create/attach)────► [UC-001] page head #btn-new → modal
                                               → rows on .session-card
Owner ──(click session tab)───────────► [UC-002] #/s/<id> switches the
                                               attached xterm in place
Owner ──(tab × / middle-click)────────► [UC-003] detach tab, session
                                               stays alive server-side
Owner ──(phone: softkeys, bell dot)───► [UC-004] #softkeys + attention
                                               markers keep working
Owner ──(reduced motion / keyboard)───► [UC-005] tablist + aria, no
                                               transitions under
                                               prefers-reduced-motion
```
