//! The vertical tool strip: tools, then the palette.

use egui::{Align2, Color32, FontId, Painter, Rect, Shape, Stroke, Ui, vec2};
use guilab_image::model::PALETTE;

use crate::paint::c32;
use crate::{Editor, Tool};

impl Editor {
    /// Tool buttons ([`Editor::tools`]) and swatches, top to bottom. Returns the new tool when
    /// it changed.
    pub fn toolbar(&mut self, ui: &mut Ui) -> Option<Tool> {
        let t = guilab_kit::Theme::get(ui.ctx());
        let before = self.tool;
        ui.spacing_mut().item_spacing = vec2(0.0, t.space.sm);
        for tool in self.tools.clone() {
            let stroke = t.size.icon_stroke;
            let name = self.tool_name(tool);
            let tip = match tool.key() {
                Some(k) => format!("{name} ({})", k.name()),
                None => name.to_string(),
            };
            if guilab_kit::tool_button(ui, self.tool == tool, &tip, |p, r, c| tool_icon(p, r, tool, c, stroke)).clicked() {
                self.tool = tool;
            }
        }
        ui.add_space(t.size.toolbar_gap - t.space.sm);
        for (name, c) in PALETTE {
            let mut label = name.to_string();
            label[..1].make_ascii_uppercase();
            if guilab_kit::swatch(ui, c32(c), self.color == c, &label).clicked() {
                self.pick_color(c);
            }
        }
        (self.tool != before).then_some(self.tool)
    }
}

/// Paints a tool's icon into `r`. Drawn on a 16-unit grid scaled to `r`, so icons follow
/// the theme's button size.
fn tool_icon(p: &Painter, r: Rect, t: Tool, c: Color32, width: f32) {
    let s = Stroke::new(width, c);
    let k = r.width() / 16.0;
    let at = |x: f32, y: f32| r.min + vec2(x, y) * k;
    match t {
        Tool::Interact => {
            // An arrow cursor.
            let pts = [(2.0, 0.0), (2.0, 14.0), (6.0, 10.5), (9.0, 16.0), (11.0, 15.0), (8.5, 9.5), (13.0, 9.5)];
            p.add(Shape::closed_line(pts.iter().map(|&(x, y)| at(x, y)).collect(), s));
        }
        Tool::Move => {
            let c0 = r.center();
            let h = r.width() / 2.0;
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = c0 + d * h;
                let n = vec2(-d.y, d.x);
                p.line_segment([c0, tip - d * 2.0 * k], s);
                p.add(Shape::convex_polygon(vec![tip, tip - d * 4.5 * k + n * 3.5 * k, tip - d * 4.5 * k - n * 3.5 * k], c, Stroke::NONE));
            }
        }
        Tool::Pen => {
            // A pencil, tip at the bottom left.
            let u = vec2(1.0, -1.0).normalized();
            let n = vec2(u.y, -u.x) * 2.6 * k;
            let tip = at(0.5, 15.5);
            let len = r.width() * 1.3;
            let (a, b) = (tip + u * 5.0 * k, tip + u * len);
            p.add(Shape::closed_line(vec![tip, a + n, b + n, b - n, a - n], s));
            p.line_segment([a + n, a - n], s);
            p.line_segment([b - u * 3.5 * k + n, b - u * 3.5 * k - n], s);
        }
        Tool::Box => {
            p.rect_stroke(r.shrink(k), 0.0, s, egui::StrokeKind::Middle);
        }
        Tool::Ellipse => {
            p.add(Shape::ellipse_stroke(r.center(), vec2(r.width() / 2.0, r.height() / 2.0 * 0.75), s));
        }
        Tool::Line => {
            p.line_segment([r.left_bottom(), r.right_top()], s);
        }
        Tool::Arrow => {
            let (a, b) = (r.left_bottom(), r.right_top());
            p.line_segment([a, b - vec2(3.0, -3.0) * k], s);
            p.add(Shape::convex_polygon(vec![b, b + vec2(-8.0, 2.0) * k, b + vec2(-2.0, 8.0) * k], c, Stroke::NONE));
        }
        Tool::Text => {
            p.text(r.center(), Align2::CENTER_CENTER, "T", FontId::proportional(18.0 * k), c);
        }
        Tool::Crop => {
            // Two overlapping L shapes.
            p.add(Shape::line(vec![at(4.0, 0.0), at(4.0, 12.0), at(16.0, 12.0)], s));
            p.add(Shape::line(vec![at(0.0, 4.0), at(12.0, 4.0), at(12.0, 16.0)], s));
        }
    }
}
