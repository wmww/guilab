//! Standalone use of the crate (no guilab): draws marks onto a PNG.
//!
//! `cargo run --example compose -- IN.png OUT.png [--scale S] [--no-text] MARK...`
//! MARK: `box|ellipse|line|arrow:X1,Y1,X2,Y2[:COLOR]`, `pen:X,Y;X,Y;...[:COLOR]`,
//! `text:X,Y:COLOR:TEXT`. COLOR: red, yellow, green, blue, white. Prints text notes.

use std::{fs::File, io::BufWriter};

use guilab_markup::{COLORS, Kind, Mark, Renderer};
use guilab_markup::egui::{Color32, ColorImage, pos2};

fn color(s: Option<&str>) -> Color32 {
    let s = s.unwrap_or("red");
    COLORS.iter().find(|(_, n)| n.eq_ignore_ascii_case(s)).unwrap_or_else(|| panic!("unknown color {s}")).0
}

fn nums(s: &str) -> Vec<f32> {
    s.split(',').map(|n| n.trim().parse().unwrap()).collect()
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut scale = 1.0;
    let mut bake_text = true;
    if let Some(i) = args.iter().position(|a| a == "--scale") {
        scale = args[i + 1].parse().unwrap();
        args.drain(i..i + 2);
    }
    if let Some(i) = args.iter().position(|a| a == "--no-text") {
        bake_text = false;
        args.remove(i);
    }
    let (input, output) = (&args[0], &args[1]);
    let mut marks = Vec::new();
    for m in &args[2..] {
        let (kind, rest) = m.split_once(':').expect("KIND:ARGS");
        if kind == "text" {
            let mut p = rest.splitn(3, ':');
            let xy = nums(p.next().unwrap());
            let c = color(p.next());
            let text = p.next().unwrap_or("").replace("\\n", "\n");
            marks.push(Mark { kind: Kind::Text(pos2(xy[0], xy[1]), text, guilab_markup::mark::WRAP_EMS * guilab_markup::mark::FONT * scale), color: c });
            continue;
        }
        let (pts, c) = match rest.split_once(':') {
            Some((p, c)) => (p, color(Some(c))),
            None => (rest, color(None)),
        };
        let kind = if kind == "pen" {
            Kind::Pen(pts.split(';').map(nums).map(|v| pos2(v[0], v[1])).collect())
        } else {
            let v = nums(pts);
            let (a, b) = (pos2(v[0], v[1]), pos2(v[2], v[3]));
            match kind {
                "box" => Kind::Box(a, b),
                "ellipse" => Kind::Ellipse(a, b),
                "line" => Kind::Line(a, b),
                "arrow" => Kind::Arrow(a, b),
                _ => panic!("unknown mark {kind}"),
            }
        };
        marks.push(Mark { kind, color: c });
    }

    let dec = png::Decoder::new(std::io::BufReader::new(File::open(input).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let size = [info.width as usize, info.height as usize];
    let base = match info.color_type {
        png::ColorType::Rgb => ColorImage::from_rgb(size, &buf[..info.buffer_size()]),
        png::ColorType::Rgba => ColorImage::from_rgba_unmultiplied(size, &buf[..info.buffer_size()]),
        t => panic!("unsupported PNG color type {t:?}"),
    };

    for m in &marks {
        if let Kind::Text(p, t, _) = &m.kind {
            println!("text at {},{}: {t:?}", p.x, p.y);
        }
    }
    let drawn: Vec<Mark> = marks.into_iter().filter(|m| bake_text || !matches!(m.kind, Kind::Text(..))).collect();
    let out = Renderer::default().composite(&base, &drawn, scale);

    let rgb: Vec<u8> = out.pixels.iter().flat_map(|c| {
        let [r, g, b, _] = c.to_array();
        [r, g, b]
    }).collect();
    let mut enc = png::Encoder::new(BufWriter::new(File::create(output).unwrap()), size[0] as u32, size[1] as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}
