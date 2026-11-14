#!/usr/bin/env python3
"""Tk (pure X11) test app: an entry, a "Menu" button that posts a menu, and a "Dialog"
button that opens a transient dialog. Prints events (flushed): `key KEYSYM`, `edit TEXT`,
`menu N`, `dialog`, and once shown `pos NAME X Y W H` (window-relative pixels)."""
import tkinter as tk


def out(*a):
    print(*a, flush=True)


root = tk.Tk(className="GuilabTk")
root.title("guilab tk test")
root.geometry("400x300")
var = tk.StringVar()
var.trace_add("write", lambda *_: out("edit", repr(var.get())))
entry = tk.Entry(root, textvariable=var)
entry.pack(fill="x", padx=10, pady=10)
entry.bind("<Key>", lambda e: out("key", e.keysym))
menu = tk.Menu(root, tearoff=0)
for i in range(1, 6):
    menu.add_command(label=f"Tk item {i}", command=lambda i=i: out("menu", i))
mbtn = tk.Button(root, text="Menu", command=lambda: menu.post(mbtn.winfo_rootx(), mbtn.winfo_rooty() + mbtn.winfo_height()))
mbtn.pack(pady=5)


def dialog():
    d = tk.Toplevel(root)
    d.title("guilab tk dialog")
    d.transient(root)
    d.geometry("200x100")
    tk.Label(d, text="a dialog").pack(expand=True)
    out("dialog")


dbtn = tk.Button(root, text="Dialog", command=dialog)
dbtn.pack(pady=5)


def report():
    for name, w in (("entry", entry), ("menu", mbtn), ("dialog", dbtn)):
        out("pos", name, w.winfo_rootx() - root.winfo_rootx(), w.winfo_rooty() - root.winfo_rooty(), w.winfo_width(), w.winfo_height())
    out("ready")


root.after(500, report)
entry.focus_set()
root.mainloop()
