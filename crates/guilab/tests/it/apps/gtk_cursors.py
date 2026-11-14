#!/usr/bin/env python3
"""GTK4 cursor test app: three side-by-side areas, each with its own cursor.

- "named": the crosshair shape (cursor-shape protocol)
- "image": a custom 32x32 texture cursor, solid red with a 2 px blue border, hotspot at
  its center (a client cursor surface)
- "hidden": no cursor

Prints `pos NAME X Y W H` (logical, window-relative) once laid out, then `ready`."""
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gtk, Gio, GLib, Gdk


def out(*a):
    print(*a, flush=True)


def red_texture(size=32):
    px = bytearray()
    for y in range(size):
        for x in range(size):
            edge = x < 2 or y < 2 or x >= size - 2 or y >= size - 2
            px += bytes([0, 0, 255, 255] if edge else [255, 0, 0, 255])
    return Gdk.MemoryTexture.new(size, size, Gdk.MemoryFormat.R8G8B8A8, GLib.Bytes.new(bytes(px)), size * 4)


app = Gtk.Application(application_id="lab.gui.Cursors", flags=Gio.ApplicationFlags.NON_UNIQUE)


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="guilab cursors")
    box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, homogeneous=True)
    areas = {}
    for name, cursor in [
        ("named", Gdk.Cursor.new_from_name("crosshair", None)),
        ("image", Gdk.Cursor.new_from_texture(red_texture(), 16, 16, None)),
        ("hidden", Gdk.Cursor.new_from_name("none", None)),
    ]:
        label = Gtk.Label(label=name, vexpand=True, hexpand=True)
        label.set_cursor(cursor)
        box.append(label)
        areas[name] = label
    win.set_child(box)

    # Once laid out in its first frame.
    def report(*_):
        bounds = {name: w.compute_bounds(win) for name, w in areas.items()}
        if not all(ok and r.get_width() > 0 for ok, r in bounds.values()):
            return GLib.SOURCE_CONTINUE
        for name, (_, r) in bounds.items():
            out("pos", name, int(r.get_x()), int(r.get_y()), int(r.get_width()), int(r.get_height()))
        out("ready")
        return GLib.SOURCE_REMOVE
    win.add_tick_callback(report)
    win.present()


app.connect("activate", activate)
app.run(None)
