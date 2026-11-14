# Wayland → X drag and drop doesn't drop

Priority: low (the user's call). X → Wayland works (`xwayland::x11_to_wayland_dnd`). Wayland → X doesn't: X windows aren't drag targets, because pointer focus is a plain `WlSurface` (`type PointerFocus = WlSurface` in handlers.rs).

## How others do it
anvil and cosmic-comp (ideas only, GPL) use the same design: pointer focus is an enum with an `X11Surface` variant for X windows (`From<KeyboardFocus>` for popup grabs too), and its `DndFocus` impl forwards to smithay's `DndFocus for X11Surface` with an offer enum over `WlOfferData`/`XwmOfferData`. All the XDND target logic is smithay's (`xwayland/xwm/dnd.rs`, `impl DndFocus for X11Surface`): on enter it takes `XdndSelection`, reads `XdndAware` (also through `XdndProxy`), sends `XdndEnter` with up to 3 types (else `XdndTypeList`), `XdndPosition` per motion (one in flight, the next cached until `XdndStatus`), and `XdndDrop` once the last status is in. A target's `ConvertSelection(XdndSelection)` reaches `xwm.dnd.transfer` → the Wayland source's `send`.

## What was tried
That enum (`PtrFocus { Wl, X }`): the exchange ran (enter, position/status, drop all sent and answered), but neither GTK4 nor Chromium on X requested the `XdndSelection` data after `XdndDrop`, and the GTK4 X app's own drags broke. Not kept: it touches every pointer-focus user (input.rs, grabs, popup grabs), so it isn't a small change.

## Suspects in smithay (pinned rev `9ff2c5a`), for whoever picks this up
- Every timestamp is `CurrentTime`: `SetSelectionOwner(XdndSelection)`, `XdndPosition` data[3], `XdndDrop` data[2]. Targets convert the selection with the drop's timestamp; real sources send the triggering event's server time. Check whether GTK/Chromium refuse or mis-order with 0.
- After the last `XdndStatus`, `handle_status` sends the deferred `XdndPosition`/`XdndDrop` to `window.window_id()`, not to the `XdndProxy` window that `motion`/`drop` use.
- `SelectionRequest` for `XdndSelection` returns without any reply when `n.owner` isn't smithay's selection window (the "not the owner anymore" branch), so a requestor would hang rather than fail.
- Not verified that anvil itself drops into GTK/Chromium X windows; building anvil and trying it is the quickest way to tell smithay bugs from ours.
