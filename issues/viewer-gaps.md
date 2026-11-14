# Viewer gaps

- A server on pixman (no render node) can't show the viewer. Idea: a GLES renderer on a Mesa software EGL device (llvmpipe, `EGLDevice::enumerate` with `EGL_MESA_device_software`, surfaceless) just for the viewer, presenting via shm, with session frames uploaded from `read_rgba` when `frame_seq` changes (an egui texture `set`) instead of the GPU mirror.
- Client-side decorations: the title bar can't show the host's real window state for minimize (no such state in xdg-shell) and our resize border is only 5 pt wide (no shadow margin outside the window). Untested on a real CSD-only host (GNOME); tested against guilab with `GUILAB_VIEWER_CSD=1`.
