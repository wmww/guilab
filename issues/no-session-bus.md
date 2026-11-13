# Apps run without a D-Bus session bus

Apps get the session dir as `XDG_RUNTIME_DIR` and no `DBUS_SESSION_BUS_ADDRESS` (like guibox), so they can't reach the user's desktop, but GTK and kitty log "Unable to acquire session bus" warnings, GApplication uniqueness is off, and portals, settings daemons and AT-SPI are unavailable.

design.md lists a private D-Bus per session under "Later" (also the way to get the AT-SPI tree). Until then this is expected noise in app logs.
