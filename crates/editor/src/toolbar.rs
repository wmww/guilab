//! The vertical tool strip: tools, then the palette.

use egui::{Align2, Color32, FontId, Painter, Rect, Shape, Stroke, Ui, vec2};
use guilab_image::model::PALETTE;

use crate::paint::c32;
use crate::{Editor, Tool};

impl Editor {
    /// Tool buttons and swatches, top to bottom. Returns the new tool when it changed.
    pub fn toolbar(&mut self, ui: &mut Ui) -> Option<Tool> {
        let t = guilab_kit::Theme::get(ui.ctx());
        let before = self.tool;
        ui.spacing_mut().item_spacing = vec2(0.0, t.space.sm);
        for tool in Tool::ALL {
            let stroke = t.size.icon_stroke;
            let tip = match tool.key() {
                Some(k) => format!("{} ({})", tool.name(), k.name()),
                None => tool.name().to_string(),
            };
            if guilab_kit::tool_button(ui, self.tool == tool, &tip, |p, r, c| tool_icon(p, r, tool, c, stroke)).clicked() {
                self.tool = tool;
            }
        }
        ui.separator();
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

fn tool_icon(p: &Painter, r: Rect, t: Tool, c: Color32, width: f32) {
    let s = Stroke::new(width, c);
    match t {
        Tool::Interact => {
            // An arrow cursor.
            let o = r.left_top() + vec2(2.0, 0.0);
            let pts = [o, o + vec2(0.0, 14.0), o + vec2(4.0, 10.5), o + vec2(7.0, 16.0), o + vec2(9.0, 15.0), o + vec2(6.5, 9.5), o + vec2(11.0, 9.5)];
            p.add(Shape::closed_line(pts.to_vec(), s));
        }
        Tool::Move => {
            let c0 = r.center();
            let h = r.width() / 2.0;
            for d in [vec2(1.0, 0.0), vec2(-1.0, 0.0), vec2(0.0, 1.0), vec2(0.0, -1.0)] {
                let tip = c0 + d * h;
                let n = vec2(-d.y, d.x);
                p.line_segment([c0, tip - d * 2.0], s);
                p.add(Shape::convex_polygon(vec![tip, tip - d * 4.5 + n * 3.5, tip - d * 4.5 - n * 3.5], c, Stroke::NONE));
            }
        }
        Tool::Pen => {
            // A pencil, tip at the bottom left.
            let u = vec2(1.0, -1.0).normalized();
            let n = vec2(u.y, -u.x) * 2.6;
            let tip = r.left_bottom() + vec2(0.5, -0.5);
            let len = r.width() * 1.3;
            let (a, b) = (tip + u * 5.0, tip + u * len);
            p.add(Shape::closed_line(vec![tip, a + n, b + n, b - n, a - n], s));
            p.line_segment([a + n, a - n], s);
            p.line_segment([b - u * 3.5 + n, b - u * 3.5 - n], s);
        }
        Tool::Box => {
            p.rect_stroke(r.shrink(1.0), 0.0, s, egui::StrokeKind::Middle);
        }
        Tool::Ellipse => {
            p.add(Shape::ellipse_stroke(r.center(), vec2(r.width() / 2.0, r.height() / 2.0 * 0.75), s));
        }
        Tool::Line => {
            p.line_segment([r.left_bottom(), r.right_top()], s);
        }
        Tool::Arrow => {
            let (a, b) = (r.left_bottom(), r.right_top());
            p.line_segment([a, b - vec2(3.0, -3.0)], s);
            p.add(Shape::convex_polygon(vec![b, b + vec2(-8.0, 2.0), b + vec2(-2.0, 8.0)], c, Stroke::NONE));
        }
        Tool::Text => {
            p.text(r.center(), Align2::CENTER_CENTER, "T", FontId::proportional(r.height() + 2.0), c);
        }
        Tool::Crop => {
            // Two overlapping L shapes.
            let (a, b) = (r.min + vec2(4.0, 0.0), r.max - vec2(4.0, 0.0));
            p.add(Shape::line(vec![a, egui::pos2(a.x, b.y - 4.0), egui::pos2(r.max.x, b.y - 4.0)], s));
            p.add(Shape::line(vec![egui::pos2(r.min.x, a.y + 4.0), egui::pos2(b.x, a.y + 4.0), b], s));
        }
    }
}
