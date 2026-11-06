## UC-005 Navigate with reduced motion / keyboard and assistive tech

With `prefers-reduced-motion` set, all transitions and animations are disabled. The tab strip exposes `role=tablist`/`role=tab`/`aria-selected`, tabs are focusable and arrow-key navigable, the "+" and close controls carry aria-labels, and every interactive element shows a visible `:focus-visible` outline. Nothing in the reduced path hides or disables functionality — it only removes motion. (FR-005)
