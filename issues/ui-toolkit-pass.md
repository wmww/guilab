# UI: what's left after the foundations pass

The flat look, theme tokens (`notes/design.md` "Theming") and widget states are done; `ui_shots` (viewer-ui example) shows every state in both themes at 1x/2x. Left for the user's design pass or a decision:
- Keyboard access to the menu bar itself (e.g. F10): keys go to the session app unless egui has focus, so there's no free key yet. Decide whether to reserve one.
- Without the image outline, a dark app on the dark view background has a faint edge (the resize handles mark it when there's room). Fine by the flat rule; revisit if it confuses.
- The View menu's zoom/size/scale steppers are stock egui buttons and drag values (themed, flat). A kit stepper/segmented control would look more native.
