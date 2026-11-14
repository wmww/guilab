//! Images from the agent's side: screenshots as images, rendering, `image make`, attaching.

use std::io::Write;
use std::process::Stdio;

use crate::*;

fn id(v: &serde_json::Value) -> String {
    v.as_str().unwrap().to_string()
}

#[test]
fn screenshot_render_and_make() {
    let lab = Lab::new();
    lab.run(&["new", "-s", "400x300", "--scale", "2"]);
    // A screenshot is an image; --crop sets its crop, --max only shrinks the PNG.
    let out = lab.run(&["screenshot", "--crop", "100,50 200x100", "--max", "100x100"]);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines[0].ends_with("-m100x100.png"), "{out}");
    assert!(lines[1].starts_with("image ") && lines[1].ends_with(" 400x300 scale 2, crop 100,50 200x100"), "{out}");
    assert_eq!(lines[2], "PNG shows canvas 100,50 200x100 at 0.5x, so PNG pixel x,y is canvas 100 + x/0.5, 50 + y/0.5");
    assert_eq!(read_png(lines[0]).0, 100);
    let shot = lab.json(&["screenshot"]);
    let s = id(&shot["image"]);
    assert_eq!((shot["width"].as_u64(), shot["zoom"].as_f64()), (Some(400), Some(1.0)));
    let (ok, _, err) = lab.try_run(&["screenshot", "--crop", "0,0 500x10"]);
    assert!(!ok && err.contains("isn't inside the 400x300 canvas"), "{err}");

    // make: lines from args and stdin, a file raster, numbers replace.
    let pic = lab.path("pic.png");
    write_png(&pic, 4, 2);
    let mut c = lab.cmd(&["image", "make", "--from", &s, "blue box 10,10 50x40", "-"]);
    let mut p = c.current_dir(&lab.rt).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    p.stdin.take().unwrap().write_all(b"green pen 1,1 5,5 9,1\n\nraster ./pic.png 300,200 40x20\n").unwrap();
    let out = p.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let l: Vec<&str> = text.lines().collect();
    let m = l[0].split(' ').nth(1).unwrap().to_string();
    assert_eq!(l[0], format!("image {m} 400x300 scale 2, parent {s} (your screenshot)"));
    assert_eq!(l[2], "  2 blue box 10,10 50x40");
    assert_eq!(l[3], "  3 green pen 1,1 8x4 (3 points)");
    assert!(l[4].starts_with("  4 raster r") && l[4].ends_with(" 300,200 40x20 (from pic.png)"), "{text}");
    assert!(l[5].starts_with("preview: ") && l[6] == "PNG shows canvas 0,0 400x300 at 1x", "{text}");
    assert_eq!(l[7], format!("send it: guilab done \"...\" --image {m}"));
    let listing = lab.run(&["image", &m, "--list", "--points"]);
    assert!(listing.contains("  3 green pen 1,1 5,5 9,1\n"), "{listing}");
    // A replaced element keeps its place; errors quote the line.
    let r = lab.json(&["image", "make", "--from", &m, "--drop", "3", "2 red box 0,0 10x10"]);
    assert!(r["listing"].as_str().unwrap().contains("  2 red box 0,0 10x10\n  3 raster"), "{r}");
    let (ok, _, err) = lab.try_run(&["image", "make", "--from", &m, "red box 1,1 2,2"]);
    assert!(!ok && err.contains("isn't a size"), "{err}");
    // Renders: the marks are drawn, --no-marks leaves them out, files are reused.
    let full = lab.json(&["image", &m, "--crop", "none"]);
    let path = id(&full["render"]["path"]);
    let (w, _, px, ch) = read_png(&path);
    let at = |x: usize, y: usize| &px[(y * w as usize + x) * ch..][..3];
    assert!(at(35, 10)[2] > 200 && at(35, 10)[0] < 80, "box edge not blue: {:?}", at(35, 10));
    let bare = id(&lab.json(&["image", &m, "--no-marks"])["render"]["path"]);
    assert!(bare.ends_with("-nomarks.png"));
    assert_eq!(id(&lab.json(&["image", &m, "--crop", "none"])["render"]["path"]), path);
    // Stale and wrong ids are errors.
    let (ok, _, err) = lab.try_run(&["image", "zzzz"]);
    assert!(!ok && err.contains("there's no zzzz"), "{err}");
    let ev = lab.json(&["log", "--kind", "image_made,screenshot"]);
    let kinds: Vec<&str> = ev["events"].as_array().unwrap().iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["screenshot", "screenshot", "image_made", "image_made"], "{ev}");

    // Attaching a file makes an image of it; the human sees it in the agent's message.
    lab.run(&["work", "x"]);
    let out = lab.try_run(&["done", "a file", "--image", &pic]);
    assert!(out.0, "{}", out.2);
    let ev = lab.json(&["log", "--kind", "agent_message"]);
    let img = &ev["events"][0]["images"][0];
    assert_eq!(img["size"], serde_json::json!([4, 2]));
    assert!(img["listing"].as_str().unwrap().contains("1 raster r") && img["listing"].as_str().unwrap().contains("(from pic.png)"), "{img}");
}

fn write_png(path: &str, w: u32, h: u32) {
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(f, w, h);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&vec![200u8; (w * h * 3) as usize]).unwrap();
}
