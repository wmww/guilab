# Wayland → X drag and drop doesn't drop

X → Wayland works (`xwayland::x11_to_wayland_dnd`). Wayland → X doesn't: X windows aren't drag targets, because pointer focus is a plain `WlSurface` (smithay's `X11Surface` is the `DndFocus` that bridges a Wayland drag into XDND, as anvil's `PointerFocusTarget::X11Surface` does).

Tried: a `PtrFocus { Wl(WlSurface), X(X11Surface) }` pointer focus (forwarding `PointerTarget` to the inner one, `DndFocus` with an offer enum over both `OfferData`s, `From<KbFocus>` for popup grabs; X windows' own wl_surface hits → `X`). Then smithay does the XDND exchange (XdndEnter, XdndPosition/XdndStatus, XdndDrop are sent and answered), but neither GTK4 (x11) nor Chromium (x11) ever requests the XdndSelection data after XdndDrop, so nothing drops; and the GTK4 X app's own drags failed afterwards. Not kept. Next: compare with anvil running the same apps (is it smithay or us?), check XdndDrop's timestamp (smithay sends CurrentTime) and XdndPosition coordinates.
