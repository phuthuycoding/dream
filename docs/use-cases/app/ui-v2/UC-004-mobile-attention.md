## UC-004 Use the terminal on mobile; see attention indicators

On a small viewport the terminal view keeps `#softkeys` reachable and working (Esc/Tab/Ctrl/arrows emit the same input frames as today) while the tab strip scrolls horizontally. A bell or `a` attention frame on a background tab marks that tab — the same flag still drives `.attention-dot` on the home row and the `●` document-title prefix; activating the tab clears it. All FR-004 selectors and the `#term-overlay` ended/respawn flow behave exactly as before. (FR-004)
