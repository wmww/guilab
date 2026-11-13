#!/usr/bin/env python3
"""GTK4 test app. Args: --menu (open a popover menu near the bottom-right corner),
--dialog (open a modal dialog), --second (open a second toplevel)."""
import sys
import gi
gi.require_version("Gtk", "4.0")
from gi.repository import Gtk, Gio, GLib

args = set(sys.argv[1:])
app = Gtk.Application(application_id="lab.gui.Test", flags=Gio.ApplicationFlags.NON_UNIQUE)


def activate(app):
    win = Gtk.ApplicationWindow(application=app, title="guilab test")
    win.set_default_size(400, 300)
    box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
    box.append(Gtk.Label(label="main window", vexpand=True))
    menu = Gio.Menu()
    for i in range(1, 6):
        menu.append(f"Item {i}", "app.noop")
    app.add_action(Gio.SimpleAction.new("noop", None))
    # Bottom-right: the popover must be flipped/slid to stay on the output.
    mb = Gtk.MenuButton(label="Menu", menu_model=menu, halign=Gtk.Align.END)
    box.append(mb)
    win.set_child(box)
    win.present()

    def later():
        if "--menu" in args:
            mb.popup()
        if "--dialog" in args:
            d = Gtk.Window(title="guilab dialog", transient_for=win, modal=True)
            d.set_default_size(200, 100)
            d.set_child(Gtk.Label(label="dialog"))
            d.present()
        if "--second" in args:
            w2 = Gtk.Window(title="second window", application=app)
            w2.set_default_size(300, 200)
            w2.set_child(Gtk.Label(label="second"))
            w2.present()
        return False

    GLib.timeout_add(300, later)


app.connect("activate", activate)
app.run([])
