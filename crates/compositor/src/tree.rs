//! `tree`: the session's surfaces with ids and pixel geometry.


use guilab_proto::{EventData, NodeKind, TreeNode};
use smithay::{
    backend::renderer::utils::with_renderer_surface_state,
    desktop::{PopupKind, PopupManager},
    reexports::wayland_server::{Resource, protocol::wl_surface::WlSurface},
    utils::{Logical, Point, Rectangle},
    wayland::{
        compositor::{SubsurfaceCachedState, get_children, with_states},
        seat::WaylandFocus,
    },
};

use crate::{
    handlers::surface_id,
    session::Session,
    shell::{has_buffer, toplevel_ids},
    xwayland::is_or,
};

impl Session {
    /// Logs windows opening, closing and changing title since the last call (each frame).
    /// Diffing catches every path (xdg, X, client unmaps, disconnects) in one place.
    pub(crate) fn track_windows(&mut self) {
        let nonempty = |s: String| (!s.is_empty()).then_some(s);
        let mut now = Vec::new();
        for w in self.space.elements().filter(|w| !is_or(w)) {
            let Some(id) = w.wl_surface().and_then(|s| surface_id(&s)) else { continue };
            let (window, app_id, title, pid) = match w.x11_surface() {
                Some(x) => ("x11", nonempty(x.class()), nonempty(x.title()), x.pid()),
                None => {
                    let s = w.wl_surface().map(|s| s.into_owned());
                    let (app_id, title) = s.as_ref().map(toplevel_ids).unwrap_or_default();
                    let pid = s.and_then(|s| self.dh.get_client(s.id()).ok()).and_then(|c| c.get_credentials(&self.dh).ok()).map(|c| c.pid as u32);
                    ("toplevel", app_id, title, pid)
                }
            };
            now.push((id, window, app_id, title, pid));
        }
        let gone: Vec<(u32, Option<String>, Option<u32>)> = self.seen_windows.iter().filter(|s| !now.iter().any(|n| n.0 == s.0)).cloned().collect();
        for (id, title, pid) in gone {
            let by = pid.map(|p| self.closed_by(p).to_string());
            self.log.push(EventData::WindowUnmap { id, title, by }, None);
        }
        for (id, window, app_id, title, _) in &now {
            match self.seen_windows.iter().find(|s| s.0 == *id) {
                None => {
                    self.settle.note_map();
                    self.log.push(EventData::WindowMap { id: *id, window: window.to_string(), app_id: app_id.clone(), title: title.clone() },
                    None)
                }
                Some((_, t, _)) if t != title => self.log.push(EventData::Title { id: *id, title: title.clone() }, None),
                Some(_) => continue,
            };
        }
        self.seen_windows = now.into_iter().map(|(id, _, _, t, pid)| (id, t, pid)).collect();
    }

    /// Why a window of the app with this pid just closed: `exit` if the process is gone,
    /// `kill` if `guilab kill` signalled its process group, else `app` (it closed it itself).
    fn closed_by(&self, pid: u32) -> &'static str {
        // /proc/PID/stat: "pid (comm) state ppid pgrp ..."; comm may contain spaces.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
        let mut f = stat.rsplit_once(')').map(|(_, r)| r.split_whitespace()).into_iter().flatten();
        let state = f.next();
        let pgrp = f.nth(1).and_then(|g| g.parse::<u32>().ok());
        if matches!(state, None | Some("Z" | "X")) {
            "exit"
        } else if pgrp.is_some_and(|g| self.killed_group(g)) {
            "kill"
        } else {
            "app"
        }
    }

    /// Mapped toplevels, topmost first, each with its subsurfaces and popups nested.
    pub fn tree(&self) -> Vec<TreeNode> {
        let scale = self.scale();
        let mut out = Vec::new();
        for w in self.space.elements().rev() {
            let Some(surface) = w.wl_surface().map(|s| s.into_owned()) else { continue };
            let Some(geo) = self.space.element_geometry(w) else { continue };
            if let Some(x) = w.x11_surface() {
                let kind = if x.is_override_redirect() { NodeKind::X11Popup } else { NodeKind::X11 };
                let mut node = self.node(&surface, kind, geo, scale);
                let nonempty = |s: String| (!s.is_empty()).then_some(s);
                node.title = nonempty(x.title());
                node.class = nonempty(x.class());
                node.xid = Some(x.window_id());
                node.pid = x.pid().or_else(|| self.xw.conn.as_ref()?.pid(x.window_id()));
                out.push(node);
                continue;
            }
            let (app_id, title) = toplevel_ids(&surface);
            let mut node = self.node(&surface, NodeKind::Toplevel, geo, scale);
            node.app_id = app_id;
            node.title = title;
            // Surface origin = geometry origin minus the window geometry offset.
            let origin = geo.loc - w.geometry().loc;
            node.children = self.subsurfaces(&surface, origin, scale);

            // Popups: offsets are relative to the toplevel's geometry origin.
            let mut popups: Vec<(WlSurface, Option<WlSurface>, TreeNode)> = Vec::new();
            for (popup, offset) in PopupManager::popups_for_surface(&surface) {
                let ps = popup.wl_surface().clone();
                // Not on screen until its first buffer (geometry may still be empty).
                if !has_buffer(&ps) {
                    continue;
                }
                let pgeo = popup.geometry();
                let loc = geo.loc + offset;
                let mut n = self.node(&ps, NodeKind::Popup, Rectangle::new(loc, pgeo.size), scale);
                n.children = self.subsurfaces(&ps, loc - pgeo.loc, scale);
                let parent = match &popup {
                    PopupKind::Xdg(p) => p.get_parent_surface(),
                    PopupKind::InputMethod(_) => None,
                };
                popups.push((ps, parent, n));
            }
            node.children.extend(nest_popups(&surface, &mut popups));
            out.push(node);
        }
        out
    }

    fn node(&self, surface: &WlSurface, kind: NodeKind, r: Rectangle<i32, Logical>, scale: f64) -> TreeNode {
        // Round edges half-down, so a 427-wide logical window at scale 1.5 is 640px, not 641.
        let edge = |v: i32| (v as f64 * scale - 0.5).ceil() as i32;
        let (x0, y0) = (edge(r.loc.x), edge(r.loc.y));
        let (x1, y1) = (edge(r.loc.x + r.size.w), edge(r.loc.y + r.size.h));
        let pid = self
            .dh
            .get_client(surface.id())
            .ok()
            .and_then(|c| c.get_credentials(&self.dh).ok())
            .map(|c| c.pid as u32);
        TreeNode {
            id: surface_id(surface).unwrap_or(0),
            kind,
            app_id: None,
            title: None,
            pid,
            xid: None,
            class: None,
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
            children: Vec::new(),
        }
    }

    /// Subsurfaces (with a buffer) of `surface`, whose origin is at `origin`.
    fn subsurfaces(&self, surface: &WlSurface, origin: Point<i32, Logical>, scale: f64) -> Vec<TreeNode> {
        get_children(surface)
            .into_iter()
            .filter_map(|child| {
                let loc = origin + with_states(&child, |s| s.cached_state.get::<SubsurfaceCachedState>().current().location);
                let size = with_renderer_surface_state(&child, |s| s.surface_size()).flatten()?;
                let mut n = self.node(&child, NodeKind::Subsurface, Rectangle::new(loc, size), scale);
                n.children = self.subsurfaces(&child, loc, scale);
                Some(n)
            })
            .collect()
    }
}

/// Moves the popups whose parent is `parent` out of `list`, each with its own popups nested.
fn nest_popups(parent: &WlSurface, list: &mut Vec<(WlSurface, Option<WlSurface>, TreeNode)>) -> Vec<TreeNode> {
    let mut mine = Vec::new();
    let mut i = 0;
    while i < list.len() {
        if list[i].1.as_ref() == Some(parent) {
            mine.push(list.remove(i));
        } else {
            i += 1;
        }
    }
    mine.into_iter()
        .map(|(s, _, mut n)| {
            n.children.extend(nest_popups(&s, list));
            n
        })
        .collect()
}
