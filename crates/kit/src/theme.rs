//! Theme tokens: every color, spacing, size, radius, font, shadow and timing the widgets use,
//! in a dark and a light variant. Widgets read them with [`Theme::get`] and never hardcode
//! values, so a theming pass changes them here only.
//!
//! The look is flat: areas are told apart by background shade and spacing, never by thin
//! borders or separator lines. Outlines only mark state (focus, selection, an open item).
//!
//! [`install`] (or [`install_with`] for an app's own tokens) puts both variants into an egui
//! context and sets egui's own styles from them (so stock egui widgets match). egui then
//! picks the variant: `ctx.set_theme(..)` for an explicit choice, and
//! `RawInput::system_theme` for "follow the system".
//!
//! Some groups serve particular widgets of this workspace: `me`/`agent`, `slot_*` and
//! [`Overlay`] (guilab-chat, the viewer), `key_point*`, `hover_outline`, `crop_*` and
//! `missing` (guilab-editor), `glow` (the viewer's flights).

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2, vec2};

use crate::anim::{Easing, Motion};

#[derive(Clone, Debug)]
pub struct Colors {
    /// Panels and bars.
    pub panel: Color32,
    /// Behind the main content (e.g. the session image).
    pub view_bg: Color32,
    /// Text fields and the message box.
    pub field: Color32,
    /// Menus, toasts, tooltips (set off by their shadow).
    pub popup: Color32,
    /// The focus ring of a focused field or widget.
    pub focus: Color32,
    pub text: Color32,
    /// Secondary text: shortcuts, hints, times, disabled labels.
    pub weak: Color32,
    pub strong: Color32,
    /// Selection, the send button, the selected tab's line.
    pub accent: Color32,
    pub accent_hover: Color32,
    pub accent_press: Color32,
    /// Text and icons on `accent`.
    pub on_accent: Color32,
    /// Flat widgets: hovered, pressed (or open), selected.
    pub hover_fill: Color32,
    pub press_fill: Color32,
    pub selected_fill: Color32,
    /// Destructive hovers (window close, remove badges), with `on_accent` on top.
    pub danger: Color32,
    /// The round × badge at rest.
    pub badge: Color32,
    pub badge_fg: Color32,
    /// Ring around a palette swatch that would vanish into the panel (white on light).
    pub swatch_ring: Color32,
    /// Resize handles at rest and lit.
    pub handle: Color32,
    pub handle_hot: Color32,
    /// Chat authors: the human using the UI, and the agent.
    pub me: Color32,
    pub agent: Color32,
    /// Editor: glow under a hovered stroke; key points, hover outline.
    pub glow: Color32,
    /// Viewer: the light from the open image's slot to the image (white on dark, black on light).
    pub beam: Color32,
    pub key_point: Color32,
    pub key_point_hot: Color32,
    /// Thin ring around key points, so they read on light and dark images.
    pub key_point_ring: Color32,
    pub hover_outline: Color32,
    /// Chat: a thumbnail slot without a texture, and the wash over the thumbnail of the slot
    /// whose image is open.
    pub slot_empty: Color32,
    pub slot_open: Color32,
    /// Crop tool: shade over the canvas outside the crop, and the crop's outline.
    pub crop_dim: Color32,
    pub crop_line: Color32,
    /// A picture whose data is gone: its box and cross.
    pub missing: Color32,
}

/// Generic spacing steps (points).
#[derive(Clone, Debug)]
pub struct Space {
    pub xs: f32,
    pub sm: f32,
    pub md: f32,
    pub lg: f32,
    pub xl: f32,
}

/// Fixed sizes of particular widgets (points).
#[derive(Clone, Debug)]
pub struct Sizes {
    pub menu_bar_h: f32,
    /// Horizontal padding of a menu title.
    pub menu_title_pad: f32,
    pub menu_item_h: f32,
    /// Horizontal padding inside menu rows.
    pub menu_item_pad: f32,
    pub menu_min_w: f32,
    /// Room for a radio/check mark before a menu label.
    pub menu_mark_w: f32,
    pub menu_heading_h: f32,
    /// The gap between groups of menu rows.
    pub menu_gap: f32,
    /// Radio ring radius and its dot, check box side.
    pub menu_radio_r: f32,
    pub menu_radio_dot: f32,
    pub menu_check: f32,
    /// Gap between a menu label and its shortcut.
    pub menu_shortcut_gap: f32,
    pub toolbar_w: f32,
    pub tool_button: f32,
    pub tool_icon_pad: f32,
    /// Gap between the tools and the palette.
    pub toolbar_gap: f32,
    pub swatch_row_h: f32,
    pub swatch_r: f32,
    /// A hovered swatch grows by this; the selection ring sits this far out.
    pub swatch_hover: f32,
    pub swatch_ring_gap: f32,
    pub tab_h: f32,
    pub tab_min_w: f32,
    pub tab_pad: f32,
    /// The accent line on the selected tab.
    pub tab_line: f32,
    pub window_button_w: f32,
    /// Half the size of a title bar button's glyph.
    pub window_glyph: f32,
    /// Resize border around a window with client-side decorations.
    pub window_border: f32,
    pub send_button: f32,
    pub close_badge: f32,
    /// Chat thumbnail height (feed and composer).
    pub feed_thumb_h: f32,
    /// Toasts: distance from the bottom, the gap between stacked toasts, padding.
    pub toast_bottom: f32,
    pub toast_gap: f32,
    pub toast_pad: Vec2,
    /// Chat panel: default, min and max height at the bottom; width on the side.
    pub chat_h: [f32; 3],
    pub chat_w: [f32; 3],
    /// Resize handles: grab band outside the edge, bar length and width, gap to the edge.
    pub handle_band: f32,
    pub handle_len: f32,
    pub handle_w: f32,
    pub handle_gap: f32,
    /// Faint feed images (the agent's working screenshots): size factor and opacity.
    pub faint_scale: f32,
    pub faint_opacity: f32,
    /// Thumbnails wider (or taller) than this aspect ratio are center-cropped to it.
    pub thumb_aspect_max: f32,
    /// The message box grows with its text up to this many lines, then scrolls.
    pub composer_lines: f32,
    /// Width of the feed's "jump to the latest message" button.
    pub jump_button: f32,
    /// The status line's dots.
    pub status_dot_r: f32,
    /// Extra hit distance around marks, and key point radii.
    pub hit_slop: f32,
    pub key_point_r: f32,
    pub key_point_hit_r: f32,
    /// A hot key point grows by this.
    pub key_point_grow: f32,
    /// Editor: the glow under a hovered stroke (extra width each side), the outline around
    /// a hovered text or picture.
    pub hover_halo: f32,
    pub hover_outline_w: f32,
    /// The open image's light, for a slot scrolled out of view: the band along the feed's
    /// edge (depth, feathered ends).
    pub glow_edge: f32,
    pub glow_feather: f32,
    /// Line widths: hairlines (crop outline, key point rings), icons, bold icons (arrows on
    /// round buttons, check marks), rings marking state (selection, the open slot), focus.
    pub line: f32,
    pub icon_stroke: f32,
    pub icon_bold: f32,
    pub ring: f32,
    pub focus_w: f32,
    /// egui's own widgets: the minimum interactive size, and tooltip width.
    pub interact: Vec2,
    pub tooltip_w: f32,
}

#[derive(Clone, Debug)]
pub struct Radius {
    pub small: f32,
    pub menu: f32,
    pub button: f32,
    pub tab: f32,
    pub field: f32,
    pub toast: f32,
    pub thumb: f32,
}

/// Drop shadows.
#[derive(Clone, Debug)]
pub struct Shadows {
    /// Menus, toasts, tooltips.
    pub popup: Shadow,
    /// Small floating buttons (jump to the latest).
    pub float: Shadow,
    /// A hovered thumbnail lifted over its neighbors.
    pub lifted: Shadow,
    /// An image flying across the window (at its strongest, mid-air).
    pub flight: Shadow,
}

#[derive(Clone, Debug)]
pub struct Fonts {
    pub body: FontId,
    pub small: FontId,
    /// Menu shortcuts.
    pub shortcut: FontId,
    pub toast: FontId,
    pub heading: FontId,
    pub mono: FontId,
    pub mono_small: FontId,
}

#[derive(Clone, Debug)]
pub struct Motions {
    /// Something flying across the window (an image to the editor and back).
    pub whoosh: Motion,
    /// A slot opening or closing in a row.
    pub slot_grow: Motion,
    /// Fades: toasts appearing, cross-fades.
    pub fade: Motion,
    /// A view's automatic zoom and pan changes (a new zoom mode, re-centering).
    pub view: Motion,
    /// Thumbnails grow by `hover_grow` on hover, over `hover`.
    pub hover: Motion,
    /// egui's own animations: popups fading in, collapsing.
    pub ui_secs: f32,
    /// Double click: the second press within this many seconds and points of the first.
    pub double_click_secs: f64,
    pub double_click_dist: f32,
    pub hover_grow: f32,
    /// Seconds before a hover reacts / a tooltip shows.
    pub hover_delay: f32,
    pub tooltip_delay: f32,
    /// Seconds a transient toast stays up.
    pub toast_secs: f64,
}

/// Colors of an input overlay drawn over app content.
#[derive(Clone, Copy, Debug)]
pub struct OverlayPalette {
    pub pointer: Color32,
    pub touch: Color32,
    /// Opacity of the glow layers.
    pub glow: f32,
}

/// An overlay showing input over app content. Its palette follows the content under the
/// pointer (dark or light), not the theme, so it's the same in both themes.
#[derive(Clone, Debug)]
pub struct Overlay {
    pub on_dark: OverlayPalette,
    pub on_light: OverlayPalette,
    pub keys: Color32,
    /// Behind key labels (alpha scaled by their fade).
    pub key_chip: Color32,
    pub key_font: f32,
    /// Sizes (points): the pointer's glow radius, its press flash, a press ripple's start
    /// and growth, a touch ring, the hover trail and drag path widths, overlay strokes,
    /// scroll chevrons (distance, its growth, arm length), the margin of the key stream.
    pub glow_r: f32,
    pub press_r: f32,
    pub ripple_r: [f32; 2],
    pub touch_r: f32,
    pub trail_w: f32,
    pub drag_w: f32,
    pub stroke: f32,
    pub chevron: [f32; 3],
    pub margin: f32,
    /// Content under the pointer brighter than this (luma 0..255) is light.
    pub light_above: f32,
    /// Seconds: palette cross-fade, hover trail and dropped drags, press ripples, key
    /// labels and their fade-out, the glow after the pointer leaves, scroll marks.
    pub palette_fade: f64,
    pub trail: f64,
    pub ripple: f64,
    pub keys_secs: f64,
    pub key_fade: f64,
    pub leave: f64,
    pub scroll: f64,
}

fn overlay() -> Overlay {
    Overlay {
        on_dark: OverlayPalette { pointer: Color32::from_rgb(110, 180, 255), touch: Color32::from_rgb(120, 230, 150), glow: 0.14 },
        on_light: OverlayPalette { pointer: Color32::from_rgb(20, 95, 215), touch: Color32::from_rgb(20, 145, 70), glow: 0.2 },
        keys: Color32::from_rgb(200, 145, 255),
        key_chip: Color32::from_black_alpha(180),
        key_font: 42.5,
        glow_r: 14.0,
        press_r: 9.0,
        ripple_r: [6.0, 18.0],
        touch_r: 18.0,
        trail_w: 2.0,
        drag_w: 2.75,
        stroke: 2.0,
        chevron: [16.0, 14.0, 5.0],
        margin: 14.0,
        light_above: 185.0,
        palette_fade: 0.3,
        trail: 0.5,
        ripple: 0.45,
        keys_secs: 3.0,
        key_fade: 0.4,
        leave: 0.3,
        scroll: 0.5,
    }
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub dark: bool,
    pub colors: Colors,
    pub space: Space,
    pub size: Sizes,
    pub radius: Radius,
    pub font: Fonts,
    pub shadow: Shadows,
    pub motion: Motions,
    pub overlay: Overlay,
}

const SPACE: Space = Space { xs: 2.0, sm: 4.0, md: 8.0, lg: 12.0, xl: 16.0 };

fn sizes() -> Sizes {
    Sizes {
        menu_bar_h: 24.0,
        menu_title_pad: 10.0,
        menu_item_h: 26.0,
        menu_item_pad: 12.0,
        menu_min_w: 200.0,
        menu_mark_w: 20.0,
        menu_heading_h: 20.0,
        menu_gap: 6.0,
        menu_radio_r: 6.0,
        menu_radio_dot: 3.5,
        menu_check: 12.0,
        menu_shortcut_gap: 32.0,
        toolbar_w: 40.0,
        tool_button: 28.0,
        tool_icon_pad: 6.0,
        toolbar_gap: 12.0,
        swatch_row_h: 22.0,
        swatch_r: 8.0,
        swatch_hover: 1.0,
        swatch_ring_gap: 3.0,
        tab_h: 30.0,
        tab_min_w: 80.0,
        tab_pad: 16.0,
        tab_line: 2.0,
        window_button_w: 40.0,
        window_glyph: 5.0,
        window_border: 5.0,
        send_button: 28.0,
        close_badge: 16.0,
        feed_thumb_h: 80.0,
        toast_bottom: 20.0,
        toast_gap: 8.0,
        toast_pad: vec2(16.0, 10.0),
        chat_h: [240.0, 90.0, 800.0],
        chat_w: [360.0, 220.0, 1200.0],
        handle_band: 10.0,
        handle_len: 28.0,
        handle_w: 4.0,
        handle_gap: 3.0,
        faint_scale: 0.6,
        faint_opacity: 0.6,
        thumb_aspect_max: 3.0,
        composer_lines: 8.0,
        jump_button: 28.0,
        status_dot_r: 3.0,
        hit_slop: 6.0,
        key_point_r: 5.0,
        key_point_hit_r: 4.0,
        key_point_grow: 1.0,
        hover_halo: 3.0,
        hover_outline_w: 2.0,
        glow_edge: 18.0,
        glow_feather: 24.0,
        line: 1.0,
        icon_stroke: 1.6,
        icon_bold: 2.0,
        ring: 2.0,
        focus_w: 1.5,
        interact: vec2(40.0, 18.0),
        tooltip_w: 480.0,
    }
}

const RADIUS: Radius = Radius { small: 2.0, menu: 6.0, button: 4.0, tab: 6.0, field: 10.0, toast: 10.0, thumb: 4.0 };

fn fonts() -> Fonts {
    Fonts {
        body: FontId::proportional(14.0),
        small: FontId::proportional(12.0),
        shortcut: FontId::proportional(13.0),
        toast: FontId::proportional(15.0),
        heading: FontId::proportional(18.0),
        mono: FontId::monospace(13.0),
        mono_small: FontId::monospace(11.5),
    }
}

fn motion() -> Motions {
    Motions {
        whoosh: Motion { secs: 0.22, easing: Easing::OutCubic },
        slot_grow: Motion { secs: 0.16, easing: Easing::OutCubic },
        fade: Motion { secs: 0.2, easing: Easing::OutCubic },
        view: Motion { secs: 0.16, easing: Easing::OutCubic },
        hover: Motion { secs: 0.1, easing: Easing::OutCubic },
        ui_secs: 1.0 / 12.0,
        double_click_secs: 0.3,
        double_click_dist: 6.0,
        hover_grow: 1.15,
        hover_delay: 0.0,
        tooltip_delay: 0.5,
        toast_secs: 4.0,
    }
}

fn shadows(color: Color32) -> Shadows {
    let s = |y: i8, blur: u8| Shadow { offset: [0, y], blur, spread: 0, color };
    Shadows { popup: s(3, 12), float: s(2, 8), lifted: s(4, 16), flight: s(6, 24) }
}

const fn gray(v: u8) -> Color32 {
    Color32::from_gray(v)
}

impl Theme {
    pub fn dark() -> Theme {
        Theme {
            dark: true,
            colors: Colors {
                panel: gray(30),
                view_bg: gray(20),
                field: gray(38),
                popup: gray(40),
                focus: gray(105),
                text: gray(218),
                weak: gray(140),
                strong: gray(250),
                accent: Color32::from_rgb(53, 132, 228),
                accent_hover: Color32::from_rgb(78, 152, 240),
                accent_press: Color32::from_rgb(40, 110, 200),
                on_accent: Color32::WHITE,
                hover_fill: gray(50),
                press_fill: gray(62),
                selected_fill: gray(66),
                danger: Color32::from_rgb(196, 43, 28),
                badge: Color32::from_black_alpha(180),
                badge_fg: Color32::WHITE,
                swatch_ring: gray(90),
                handle: gray(100),
                handle_hot: gray(235),
                me: Color32::from_rgb(110, 170, 255),
                agent: Color32::from_rgb(120, 210, 140),
                glow: Color32::from_rgba_unmultiplied(110, 170, 255, 60),
                beam: Color32::from_white_alpha(38),
                key_point: gray(240),
                key_point_hot: Color32::from_rgb(110, 170, 255),
                key_point_ring: Color32::from_black_alpha(170),
                hover_outline: Color32::from_rgba_unmultiplied(110, 170, 255, 140),
                slot_empty: gray(24),
                slot_open: Color32::from_white_alpha(110),
                crop_dim: Color32::from_black_alpha(150),
                crop_line: gray(240),
                missing: gray(128),
            },
            space: SPACE,
            size: sizes(),
            radius: RADIUS,
            font: fonts(),
            shadow: shadows(Color32::from_black_alpha(110)),
            motion: motion(),
            overlay: overlay(),
        }
    }

    pub fn light() -> Theme {
        Theme {
            dark: false,
            colors: Colors {
                panel: gray(242),
                view_bg: gray(212),
                field: gray(255),
                popup: gray(252),
                focus: gray(150),
                text: gray(36),
                weak: gray(112),
                strong: gray(8),
                accent: Color32::from_rgb(28, 113, 216),
                accent_hover: Color32::from_rgb(20, 98, 196),
                accent_press: Color32::from_rgb(14, 80, 165),
                on_accent: Color32::WHITE,
                hover_fill: gray(226),
                press_fill: gray(212),
                selected_fill: gray(214),
                danger: Color32::from_rgb(196, 43, 28),
                badge: Color32::from_black_alpha(170),
                badge_fg: Color32::WHITE,
                swatch_ring: gray(150),
                handle: gray(150),
                handle_hot: gray(40),
                me: Color32::from_rgb(30, 100, 210),
                agent: Color32::from_rgb(25, 135, 60),
                glow: Color32::from_rgba_unmultiplied(30, 100, 210, 50),
                beam: Color32::from_black_alpha(26),
                key_point: gray(255),
                key_point_hot: Color32::from_rgb(30, 100, 210),
                key_point_ring: Color32::from_black_alpha(150),
                hover_outline: Color32::from_rgba_unmultiplied(30, 100, 210, 140),
                slot_empty: gray(228),
                slot_open: Color32::from_black_alpha(80),
                crop_dim: Color32::from_black_alpha(130),
                crop_line: gray(255),
                missing: gray(128),
            },
            space: SPACE,
            size: sizes(),
            radius: RADIUS,
            font: fonts(),
            shadow: shadows(Color32::from_black_alpha(48)),
            motion: motion(),
            overlay: overlay(),
        }
    }

    pub fn of(theme: egui::Theme) -> Theme {
        match theme {
            egui::Theme::Dark => Theme::dark(),
            egui::Theme::Light => Theme::light(),
        }
    }

    /// The installed theme egui currently uses (dark if [`install`] wasn't called).
    pub fn get(ctx: &egui::Context) -> Arc<Theme> {
        let which = ctx.theme();
        ctx.data(|d| d.get_temp::<Arc<Theme>>(key(which))).unwrap_or_else(|| Arc::new(Theme::of(which)))
    }

    /// egui's style for this theme: stock widgets (buttons, drag values, labels, tooltips,
    /// scroll bars, panels) follow the tokens, flat: no strokes around widgets, panels or
    /// popups, and separators are blank space.
    pub fn style(&self, base: &egui::Style) -> egui::Style {
        let c = &self.colors;
        let sz = &self.size;
        let mut s = base.clone();
        s.visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        let v = &mut s.visuals;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c.panel;
        w.noninteractive.weak_bg_fill = c.panel;
        w.noninteractive.fg_stroke = Stroke::new(sz.line, c.text);
        w.inactive.bg_fill = c.hover_fill;
        w.inactive.weak_bg_fill = c.hover_fill;
        w.inactive.fg_stroke = Stroke::new(sz.line, c.text);
        w.hovered.bg_fill = c.press_fill;
        w.hovered.weak_bg_fill = c.press_fill;
        w.hovered.fg_stroke = Stroke::new(sz.icon_stroke, c.strong);
        w.active.bg_fill = c.selected_fill;
        w.active.weak_bg_fill = c.selected_fill;
        w.active.fg_stroke = Stroke::new(sz.icon_bold, c.strong);
        w.open = w.active;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.bg_stroke = Stroke::NONE;
            wv.corner_radius = CornerRadius::same(self.radius.button as u8);
            wv.expansion = 0.0;
        }
        v.selection.bg_fill = c.accent;
        // Text on a selection (selected labels): readable on the accent. Also a focused
        // text field's ring.
        v.selection.stroke = Stroke::new(sz.focus_w, c.on_accent);
        v.hyperlink_color = c.accent;
        v.weak_text_color = Some(c.weak);
        v.faint_bg_color = c.hover_fill;
        v.extreme_bg_color = c.view_bg;
        v.text_edit_bg_color = Some(c.field);
        v.code_bg_color = c.hover_fill;
        v.panel_fill = c.panel;
        v.window_fill = c.popup;
        v.window_stroke = Stroke::NONE;
        v.window_corner_radius = CornerRadius::same(self.radius.menu as u8);
        v.menu_corner_radius = CornerRadius::same(self.radius.menu as u8);
        v.window_shadow = self.shadow.popup;
        v.popup_shadow = self.shadow.popup;
        v.interact_cursor = Some(egui::CursorIcon::Default);

        let sp = &mut s.spacing;
        sp.item_spacing = vec2(self.space.md, self.space.sm);
        sp.button_padding = vec2(self.space.md, self.space.sm);
        sp.menu_margin = Margin::symmetric(0, self.space.sm as i8);
        sp.window_margin = Margin::same(self.space.md as i8);
        sp.interact_size = sz.interact;
        sp.tooltip_width = sz.tooltip_w;

        s.animation_time = self.motion.ui_secs;
        s.interaction.tooltip_delay = self.motion.tooltip_delay;
        s.interaction.show_tooltips_only_when_still = true;

        let f = &self.font;
        s.text_styles = [
            (TextStyle::Small, f.small.clone()),
            (TextStyle::Body, f.body.clone()),
            (TextStyle::Button, f.body.clone()),
            (TextStyle::Heading, f.heading.clone()),
            (TextStyle::Monospace, f.mono.clone()),
        ]
        .into();
        debug_assert!(matches!(f.body.family, FontFamily::Proportional));
        s
    }
}

fn key(t: egui::Theme) -> egui::Id {
    egui::Id::new(("guilab-kit-theme", t == egui::Theme::Dark))
}

/// Installs this toolkit's default themes into `ctx` (egui styles and tokens). Call once at
/// startup.
pub fn install(ctx: &egui::Context) {
    install_with(ctx, Theme::dark(), Theme::light());
}

/// Installs an app's own dark and light tokens.
pub fn install_with(ctx: &egui::Context, dark: Theme, light: Theme) {
    // Input timing isn't per variant: the dark one's.
    let (secs, dist) = (dark.motion.double_click_secs, dark.motion.double_click_dist);
    for (which, theme) in [(egui::Theme::Dark, dark), (egui::Theme::Light, light)] {
        let style = theme.style(&ctx.style_of(which));
        ctx.set_style_of(which, style);
        ctx.data_mut(|d| d.insert_temp(key(which), Arc::new(theme)));
    }
    ctx.options_mut(|o| {
        o.input_options.max_double_click_delay = secs;
        o.input_options.max_click_dist = dist;
        // Without a system preference, "follow the system" falls back to dark.
        o.fallback_theme = egui::Theme::Dark;
    });
}

/// The user's choice in a View menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemePref {
    pub const ALL: [(ThemePref, &'static str); 3] = [(ThemePref::System, "System"), (ThemePref::Light, "Light"), (ThemePref::Dark, "Dark")];

    pub fn egui(self) -> egui::ThemePreference {
        match self {
            ThemePref::System => egui::ThemePreference::System,
            ThemePref::Light => egui::ThemePreference::Light,
            ThemePref::Dark => egui::ThemePreference::Dark,
        }
    }
}
