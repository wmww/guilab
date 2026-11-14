# Xwayland gaps

- HiDPI: X apps render at scale 1 and are upscaled (blurry). Sharp output would need `set_client_scale(s)` on the Xwayland client plus XSETTINGS (`Xft/DPI` = 96·1024·s, `Gdk/WindowScalingFactor`, `Gdk/UnscaledDPI`) via `X11Wm::set_xsettings`, re-sent on `output --scale`; core-X apps (xterm, Tk) would then be tiny unless `Xft.dpi` is also set in RESOURCE_MANAGER.
- Drag and drop between X and Wayland apps is untested. Pointer focus is a plain `WlSurface`, so X windows aren't `DndFocus` targets the way anvil's `PointerFocusTarget::X11Surface` is; X->Wayland drags go through smithay's XWM dnd code, also untested.
- No root cursor image (`X11Wm::set_cursor`), only matters once `screenshot --cursor` exists.
- `_NET_WM_PID` is the only pid source for X windows; apps that don't set it (xeyes) have no pid in `tree`.
