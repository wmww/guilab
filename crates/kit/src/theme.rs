//! Theme tokens: every color, spacing, size, radius, font and timing the widgets use, in a
//! dark and a light variant. Widgets read them with [`Theme::get`] and never hardcode
//! values, so a theming pass changes them here only.
//!
//! [`install`] puts both variants into an egui context and sets egui's own styles from
//! them (so stock egui widgets match). egui then picks the variant: `ctx.set_theme(..)`
//! for an explicit choice, and `RawInput::system_theme` for "follow the system".

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
    /// Menus, toasts, tooltips.
    pub popup: Color32,
    pub popup_stroke: Color32,
    /// Thin lines: separators, image borders, idle field outlines.
    pub outline: Color32,
    /// A focused field's outline.
    pub outline_focus: Color32,
    pub text: Color32,
    /// Secondary text: shortcuts, hints, times, disabled labels.
    pub weak: Color32,
    pub strong: Color32,
    /// Selection, the send button, the selected tab's line.
    pub accent: Color32,
    pub accent_hover: Color32,
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
    /// Ring around palette swatches.
    pub swatch_ring: Color32,
    /// Resize handles at rest and lit.
    pub handle: Color32,
    pub handle_hot: Color32,
    /// Chat authors: the human using the UI, and the agent.
    pub me: Color32,
    pub agent: Color32,
    /// Editor (later phases): glow from a slot to the open image, key points, hover outline.
    pub glow: Color32,
    pub key_point: Color32,
    pub key_point_hot: Color32,
    /// Thin ring around key points, so they read on light and dark images.
    pub key_point_ring: Color32,
    pub hover_outline: Color32,
    /// Crop tool: shade over the canvas outside the crop, and the crop's outline.
    pub crop_dim: Color32,
    pub crop_line: Color32,
    pub shadow: Color32,
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
    pub menu_separator_h: f32,
    /// Gap between a menu label and its shortcut.
    pub menu_shortcut_gap: f32,
    pub toolbar_w: f32,
    pub tool_button: f32,
    pub tool_icon_pad: f32,
    pub swatch_row_h: f32,
    pub swatch_r: f32,
    pub tab_h: f32,
    pub tab_min_w: f32,
    pub tab_pad: f32,
    pub window_button_w: f32,
    /// Resize border around a window with client-side decorations.
    pub window_border: f32,
    pub send_button: f32,
    pub close_badge: f32,
    /// Thumbnail heights: queued drawings and feed images.
    pub thumb_h: f32,
    pub feed_thumb_h: f32,
    /// Largest hover preview of an image.
    pub preview_max: Vec2,
    /// Toasts: distance from the bottom, and height of one stacked slot.
    pub toast_bottom: f32,
    pub toast_slot: f32,
    /// Resize handles: grab band outside the edge, bar length and width, gap to the edge.
    pub handle_band: f32,
    pub handle_len: f32,
    pub handle_w: f32,
    pub handle_gap: f32,
    /// Feed images of the agent's own (later phases).
    pub faint_scale: f32,
    /// Extra hit distance around marks, and key point radii (later phases).
    pub hit_slop: f32,
    pub key_point_r: f32,
    pub key_point_hit_r: f32,
    /// Line widths: hairlines, icons, the focus ring.
    pub line: f32,
    pub icon_stroke: f32,
}

#[derive(Clone, Debug)]
pub struct Radius {
    pub small: f32,
    pub menu: f32,
    pub button: f32,
    pub tab: f32,
    pub field: f32,
    pub toast: f32,
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
    /// Thumbnails grow by `hover_grow` on hover, over `hover`.
    pub hover: Motion,
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
        menu_separator_h: 9.0,
        menu_shortcut_gap: 32.0,
        toolbar_w: 40.0,
        tool_button: 28.0,
        tool_icon_pad: 6.0,
        swatch_row_h: 22.0,
        swatch_r: 8.0,
        tab_h: 30.0,
        tab_min_w: 80.0,
        tab_pad: 16.0,
        window_button_w: 40.0,
        window_border: 5.0,
        send_button: 28.0,
        close_badge: 16.0,
        thumb_h: 56.0,
        feed_thumb_h: 80.0,
        preview_max: vec2(720.0, 480.0),
        toast_bottom: 20.0,
        toast_slot: 64.0,
        handle_band: 10.0,
        handle_len: 28.0,
        handle_w: 4.0,
        handle_gap: 3.0,
        faint_scale: 0.6,
        hit_slop: 6.0,
        key_point_r: 5.0,
        key_point_hit_r: 4.0,
        line: 1.0,
        icon_stroke: 1.6,
    }
}

const RADIUS: Radius = Radius { small: 2.0, menu: 6.0, button: 4.0, tab: 6.0, field: 10.0, toast: 10.0 };

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
        hover: Motion { secs: 0.1, easing: Easing::OutCubic },
        hover_grow: 1.15,
        hover_delay: 0.0,
        tooltip_delay: 0.5,
        toast_secs: 4.0,
    }
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
                popup: gray(36),
                popup_stroke: gray(58),
                outline: gray(58),
                outline_focus: gray(105),
                text: gray(218),
                weak: gray(140),
                strong: gray(250),
                accent: Color32::from_rgb(53, 132, 228),
                accent_hover: Color32::from_rgb(78, 152, 240),
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
                key_point: gray(240),
                key_point_hot: Color32::from_rgb(110, 170, 255),
                key_point_ring: Color32::from_black_alpha(170),
                hover_outline: Color32::from_rgba_unmultiplied(110, 170, 255, 140),
                crop_dim: Color32::from_black_alpha(150),
                crop_line: gray(240),
                shadow: Color32::from_black_alpha(96),
            },
            space: SPACE,
            size: sizes(),
            radius: RADIUS,
            font: fonts(),
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
                popup_stroke: gray(198),
                outline: gray(200),
                outline_focus: gray(140),
                text: gray(36),
                weak: gray(112),
                strong: gray(8),
                accent: Color32::from_rgb(28, 113, 216),
                accent_hover: Color32::from_rgb(20, 98, 196),
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
                key_point: gray(255),
                key_point_hot: Color32::from_rgb(30, 100, 210),
                key_point_ring: Color32::from_black_alpha(150),
                hover_outline: Color32::from_rgba_unmultiplied(30, 100, 210, 140),
                crop_dim: Color32::from_black_alpha(130),
                crop_line: gray(255),
                shadow: Color32::from_black_alpha(40),
            },
            space: SPACE,
            size: sizes(),
            radius: RADIUS,
            font: fonts(),
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
    /// scroll bars) follow the tokens.
    pub fn style(&self, base: &egui::Style) -> egui::Style {
        let c = &self.colors;
        let mut s = base.clone();
        s.visuals = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        let v = &mut s.visuals;
        let r = CornerRadius::same(self.radius.button as u8);
        let line = self.size.line;
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = c.panel;
        w.noninteractive.weak_bg_fill = c.panel;
        w.noninteractive.bg_stroke = Stroke::new(line, c.outline);
        w.noninteractive.fg_stroke = Stroke::new(line, c.text);
        w.inactive.bg_fill = c.hover_fill;
        w.inactive.weak_bg_fill = c.hover_fill;
        w.inactive.bg_stroke = Stroke::NONE;
        w.inactive.fg_stroke = Stroke::new(line, c.text);
        w.hovered.bg_fill = c.press_fill;
        w.hovered.weak_bg_fill = c.press_fill;
        w.hovered.bg_stroke = Stroke::new(line, c.outline);
        w.hovered.fg_stroke = Stroke::new(1.5, c.strong);
        w.active.bg_fill = c.selected_fill;
        w.active.weak_bg_fill = c.selected_fill;
        w.active.bg_stroke = Stroke::new(line, c.outline_focus);
        w.active.fg_stroke = Stroke::new(2.0, c.strong);
        w.open = w.active;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
            wv.expansion = 0.0;
        }
        v.selection.bg_fill = c.accent;
        // Text on a selection (selected labels): readable on the accent.
        v.selection.stroke = Stroke::new(line, c.on_accent);
        v.hyperlink_color = c.accent;
        v.weak_text_color = Some(c.weak);
        v.faint_bg_color = c.hover_fill;
        v.extreme_bg_color = c.view_bg;
        v.text_edit_bg_color = Some(c.field);
        v.code_bg_color = c.hover_fill;
        v.panel_fill = c.panel;
        v.window_fill = c.popup;
        v.window_stroke = Stroke::new(line, c.popup_stroke);
        v.window_corner_radius = CornerRadius::same(self.radius.menu as u8);
        v.menu_corner_radius = CornerRadius::same(self.radius.menu as u8);
        let shadow = Shadow { offset: [0, 3], blur: 12, spread: 0, color: c.shadow };
        v.window_shadow = shadow;
        v.popup_shadow = shadow;
        v.interact_cursor = Some(egui::CursorIcon::Default);

        let sp = &mut s.spacing;
        sp.item_spacing = vec2(self.space.md, self.space.sm);
        sp.button_padding = vec2(self.space.md, self.space.xs + 1.0);
        sp.menu_margin = Margin::symmetric(0, self.space.sm as i8);
        sp.window_margin = Margin::same(self.space.md as i8);
        sp.interact_size = vec2(40.0, 18.0);
        sp.tooltip_width = 480.0;

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

/// Installs both themes into `ctx` (egui styles and tokens). Call once at startup.
pub fn install(ctx: &egui::Context) {
    for (which, theme) in [(egui::Theme::Dark, Theme::dark()), (egui::Theme::Light, Theme::light())] {
        let style = theme.style(&ctx.style_of(which));
        ctx.set_style_of(which, style);
        ctx.data_mut(|d| d.insert_temp(key(which), Arc::new(theme)));
    }
    // Without a system preference, "follow the system" falls back to dark.
    ctx.options_mut(|o| o.fallback_theme = egui::Theme::Dark);
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
