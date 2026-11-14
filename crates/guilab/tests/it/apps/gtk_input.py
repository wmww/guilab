#!/usr/bin/env python3
"""GTK4 input test app. Prints what it receives (one line per event, flushed) and, once
laid out, `pos NAME X Y W H` (logical, window-relative) for each widget, so tests know
where to click.

Args: --floats opens two more (floating) windows, "float A" 300x200 and "float B" 200x100,
each just an entry printing `A TEXT` / `B TEXT`.

Layout: entry, button, click area (prints press counts and right-click menus), scroll list,
drag source / drop target, zoom area."""
import sys
import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Gdk", "4.0")
from gi.repository import Gtk, Gio, GLib, Gdk, GObject


def out(*a):
    print(*a, flush=True)


app = Gtk.Application(application_id="lab.gui.Input", flags=Gio.ApplicationFlags.NON_UNIQUE)


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="guilab input")
    win.set_default_size(600, 500)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
    widgets = {}

    entry = Gtk.Entry()
    entry.connect("changed", lambda e: out("entry", repr(e.get_text())))
    box.append(entry)
    widgets["entry"] = entry

    button = Gtk.Button(label="Click me")
    count = [0]

    def clicked(_):
        count[0] += 1
        out("clicked", count[0])
    button.connect("clicked", clicked)
    box.append(button)
    widgets["button"] = button

    # Click area: press counts (double-click) and a right-click context menu.
    area = Gtk.Label(label="click area (right-click: menu)")
    area.set_size_request(-1, 60)
    g = Gtk.GestureClick(button=0)
    g.connect("pressed", lambda g, n, x, y: out("pressed", g.get_current_button(), n))
    area.add_controller(g)
    menu = Gio.Menu()
    for i in range(1, 4):
        menu.append(f"Ctx {i}", f"app.ctx{i}")
        act = Gio.SimpleAction.new(f"ctx{i}", None)
        act.connect("activate", lambda a, p, i=i: out("menu", i))
        app.add_action(act)
    pop = Gtk.PopoverMenu(menu_model=menu, has_arrow=False)
    pop.set_parent(area)
    pop.connect("closed", lambda p: out("menu closed"))
    rc = Gtk.GestureClick(button=3)

    def right(g, n, x, y):
        r = Gdk.Rectangle()
        r.x, r.y, r.width, r.height = int(x), int(y), 1, 1
        pop.set_pointing_to(r)
        pop.popup()
    rc.connect("pressed", right)
    area.add_controller(rc)
    box.append(area)
    widgets["area"] = area

    # Scrollable list.
    lb = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    for i in range(100):
        lb.append(Gtk.Label(label=f"row {i}", xalign=0))
    sw = Gtk.ScrolledWindow(vexpand=True)
    sw.set_child(lb)
    sw.get_vadjustment().connect("value-changed", lambda a: out("vscroll", round(a.get_value())))
    box.append(sw)
    widgets["scroll"] = sw

    # Drag and drop.
    dnd = Gtk.Box(spacing=20, homogeneous=True)
    src = Gtk.Label(label="Drag me")
    src.set_size_request(-1, 50)
    ds = Gtk.DragSource(actions=Gdk.DragAction.COPY)
    ds.connect("prepare", lambda s, x, y: Gdk.ContentProvider.new_for_value("payload"))
    ds.connect("drag-begin", lambda s, d: out("drag begin"))
    src.add_controller(ds)
    dst = Gtk.Label(label="Drop here")
    dt = Gtk.DropTarget.new(GObject.TYPE_STRING, Gdk.DragAction.COPY)

    def dropped(t, v, x, y):
        out("dropped", v)
        return True
    dt.connect("drop", dropped)
    dst.add_controller(dt)
    dnd.append(src)
    dnd.append(dst)
    box.append(dnd)
    widgets["drag"] = src
    widgets["drop"] = dst

    zoom = Gtk.Label(label="zoom area")
    zoom.set_size_request(-1, 50)
    z = Gtk.GestureZoom()
    z.connect("scale-changed", lambda g, s: out("zoom", round(s, 2)))
    z.connect("end", lambda g, s: out("zoom end"))
    zoom.add_controller(z)
    box.append(zoom)
    widgets["zoom"] = zoom

    keys = Gtk.EventControllerKey()
    keys.connect("key-pressed", lambda c, kv, code, st: out("key", Gdk.keyval_name(kv), code, int(st)) and False)
    keys.connect("key-released", lambda c, kv, code, st: out("keyup", Gdk.keyval_name(kv)))
    win.add_controller(keys)
    hover = Gtk.EventControllerMotion()
    hover.connect("leave", lambda c: out("pointer left"))
    win.add_controller(hover)

    win.set_child(box)
    win.present()

    # Once laid out in its first frame (not after a fixed delay: a busy machine is slow).
    def report(*_):
        bounds = {name: w.compute_bounds(win) for name, w in widgets.items()}
        if not all(ok and b.get_width() > 0 for ok, b in bounds.values()):
            return GLib.SOURCE_CONTINUE
        for name, (_, b) in bounds.items():
            out("pos", name, round(b.get_x()), round(b.get_y()), round(b.get_width()), round(b.get_height()))
        out("ready")
        return GLib.SOURCE_REMOVE
    win.add_tick_callback(report)

    if "--floats" in sys.argv:
        for name, w, h in (("A", 300, 200), ("B", 200, 100)):
            f = Gtk.Window(title=f"float {name}", application=app)
            f.set_default_size(w, h)
            e = Gtk.Entry(valign=Gtk.Align.CENTER)
            e.connect("changed", lambda e, n=name: out(n, repr(e.get_text())))
            f.set_child(e)
            f.present()


app.connect("activate", activate)
app.run([])
