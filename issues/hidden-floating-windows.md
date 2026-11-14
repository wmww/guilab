# Floating windows can hide each other for good

Floating windows are always re-centered (`shell.rs` `centered`) and xdg move/resize requests are ignored. Two floats of similar size overlap, and clicking the top one never exposes the other: an agent can't reach the lower one (e.g. `gtk_input.py --floats`: float B sits entirely under float A).

Ideas: cascade new floats instead of centering all of them; honor `xdg_toplevel.move` (title-bar drags); or a CLI way to raise a window by id (`guilab raise ID`, also handy with dialogs).
