## UC-003 Detach a session tab without killing it

Owner clicks a tab's close control — or middle-clicks the tab — to detach: the tab's socket and xterm are disposed, its id is removed from `dream.tabs`, and the strip re-renders. The session itself keeps running server-side (no kill call); it remains listed on the home view for later re-attach. Closing the active tab activates the nearest neighbour; closing the last tab routes back to `#/` home. (FR-002)
