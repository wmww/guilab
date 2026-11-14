use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use crate::format::{ListOpts, parse_lines, store_listing};
use crate::geom::{Key, Mods};
use crate::id::IdGen;
use crate::*;

const G: Group = Group(1);

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(Fonts::new)
}

fn store() -> Store {
    Store::with_ids(Limits::default(), IdGen::seeded(7))
}

/// A WxH capture: a horizontal gradient so crops and placement are checkable.
fn capture(w: u32, h: u32) -> Raster {
    let mut px = Vec::new();
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 100, 255]);
        }
    }
    Raster::from_capture(w, h, px, 50_690_000)
}

fn screenshot(s: &mut Store, w: u32, h: u32, scale: f32) -> ImageId {
    let a = s.add_asset(Asset::Raster(capture(w, h)), G);
    s.add_image(None, Body::screenshot(a, [w, h], scale), Made::Screenshot { at_ms: 50_690_000, by_agent: true }, G, &[]).unwrap()
}

fn req(from: Option<ImageId>, els: &[&str]) -> MakeReq {
    MakeReq { from: from.map(|i| i.to_string()), elements: els.iter().map(|s| s.to_string()).collect(), cwd: tmp(), ..Default::default() }
}

fn mk(s: &mut Store, r: &MakeReq) -> Result<ImageId, String> {
    make(s, fonts(), r, Made::Agent, G)
}

fn list(s: &Store, id: ImageId, points: bool) -> String {
    let img = s.image(id).unwrap();
    store_listing(s, &img, fonts(), &ListOpts { points, utc_offset_s: 0 })
}

fn tmp() -> PathBuf {
    let d = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/image-tests");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes a file atomically (temp file + rename): tests run in parallel, and several
/// write the same fixtures (`everything`'s rt.png and rt.svg) that others are reading.
fn put(path: impl AsRef<Path>, data: impl AsRef<[u8]>) -> std::io::Result<()> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let path = path.as_ref();
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("tmp{}-{n}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Writes a rendering for checking by eye (target/image-tests/NAME.png).
fn save(name: &str, r: &Rendered) {
    put(tmp().join(format!("{name}.png")), r.png(None)).unwrap();
}

fn rend(s: &Store, id: ImageId, opts: RenderOpts) -> Rendered {
    let img = s.image(id).unwrap();
    render(&img.body, s, fonts(), &opts).unwrap()
}

fn close(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
    a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
}

const SVG_RED: &str = r##"<svg width="40" height="20"><rect width="40" height="20" fill="#ff0000"/></svg>"##;

// ------------------------------------------------------------------ ids

#[test]
fn ids_kinds_and_errors() {
    assert!(ImageId::parse("k3f9").is_ok());
    assert_eq!(ImageId::parse("r7qa").unwrap_err().to_string(), "r7qa is a raster, not an image");
    assert_eq!(ImageId::parse("s7qa").unwrap_err().to_string(), "s7qa is an SVG, not an image");
    assert_eq!(AssetId::parse("k3f9").unwrap_err().to_string(), "k3f9 is an image, not a raster or SVG");
    assert!(ImageId::parse("k3f1").is_err(), "1 isn't in the alphabet");
    assert_eq!(ImageId::parse("K3F9").unwrap().to_string(), "k3f9");
    let s = store();
    assert_eq!(s.lookup("r7qa").unwrap_err().to_string(), "r7qa is a raster, not an image");
    assert_eq!(s.lookup("k3f9").unwrap_err().to_string(), "there's no k3f9");
}

#[test]
fn ids_are_unique_and_grow() {
    let mut s = store();
    let mut seen = std::collections::HashSet::new();
    // Raster space at 4 chars is 31^3 = 29791; past half of it ids get longer.
    for _ in 0..IdGen::space(IdKind::Raster, 4) / 2 {
        let id = s.add_asset(Asset::Raster(capture(1, 1)), G);
        assert_eq!(id.kind(), IdKind::Raster);
        assert!(seen.insert(id));
    }
    assert!(seen.iter().all(|i| i.as_str().len() == 4));
    let id = s.add_asset(Asset::Raster(capture(1, 1)), G);
    assert_eq!(id.as_str().len(), 5);
    let id = screenshot(&mut s, 2, 2, 1.0);
    assert!(!id.as_str().starts_with(['r', 's']));
}

// ------------------------------------------------------------------ format

#[test]
fn numbers_print_compactly() {
    use crate::format::num;
    assert_eq!(num(12.0), "12");
    assert_eq!(num(12.34), "12.3");
    assert_eq!(num(-0.04), "0");
    assert_eq!(num(-3.25), "-3.3");
    assert_eq!(num(0.96), "1");
}

/// An image with every kind of element.
fn everything(s: &mut Store) -> ImageId {
    let shot = screenshot(s, 400, 300, 2.0);
    let svg = tmp().join("rt.svg");
    put(&svg, SVG_RED).unwrap();
    let r = MakeReq {
        crop: Some("10,20 300x200".into()),
        color: Some("#102030".into()),
        ..req(
            Some(shot),
            &[
                "blue box 26,18 52x9.5",
                "yellow ellipse 101,61 30x11",
                "green line 80,27 to 80,30",
                "white arrow 5,5 to 60,70",
                "#7f00ff pen 10,10 20,25.5 30,12",
                "black pen 50,50",
                r#"text 82,26 "30px gap, \"should\" be 16\\n""#,
                r#"red text 10,200 size 24 width 120 "two\nlines and some wrapping words""#,
                "raster ./rt.png 4,4 24x12 from 6,3 12x6",
                "svg ./rt.svg 60,4 64x32",
                "svg <svg width=\"8\" height=\"8\"><circle cx=\"4\" cy=\"4\" r=\"4\" fill=\"blue\"/></svg> 200,4",
            ],
        )
    };
    // A plain PNG file for the raster line.
    let png = render(&Body { background: Some(Color::GREEN), ..Body::new([30, 20], 1.0) }, s, fonts(), &RenderOpts::default()).unwrap();
    put(tmp().join("rt.png"), strip_mark(&png.png(None))).unwrap();
    mk(s, &r).unwrap()
}

/// Re-encodes a PNG without its text chunks (what an outside tool would do).
fn strip_mark(png: &[u8]) -> Vec<u8> {
    let dec = png::Decoder::new(std::io::Cursor::new(png));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, info.width, info.height);
    enc.set_color(info.color_type);
    enc.set_depth(info.bit_depth);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(&buf[..info.buffer_size()]).unwrap();
    w.finish().unwrap();
    out
}

#[test]
fn listing_shape() {
    let mut s = store();
    let id = everything(&mut s);
    let l = list(&s, id, false);
    let lines: Vec<&str> = l.lines().collect();
    let parent = s.image(id).unwrap().parent.unwrap();
    assert_eq!(lines[0], format!("image {id} 400x300 scale 2, parent {parent} (your screenshot), background #102030, crop 10,20 300x200"));
    assert!(lines[1].starts_with("  1 raster r") && lines[1].ends_with(" 0,0 400x300 (screenshot 14:04:50)"), "{}", lines[1]);
    assert_eq!(lines[2], "  2 blue box 26,18 52x9.5");
    assert_eq!(lines[6], "  6 #7f00ff pen 10,10 20x15.5 (3 points)");
    assert_eq!(lines[7], "  7 black pen 50,50 0x0 (1 point)");
    assert!(lines[8].starts_with(r#"  8 red text 82,26 "30px gap, \"should\" be 16\\n" (box 74,18 "#), "{}", lines[8]);
    assert!(lines[9].starts_with(r#"  9 red text 10,200 size 24 width 120 "two\nlines"#), "{}", lines[9]);
    assert!(lines[10].contains(" 4,4 24x12 from 6,3 12x6 (from rt.png)"), "{}", lines[10]);
    assert!(lines[11].starts_with("  11 svg s") && lines[11].ends_with(" 60,4 64x32 (from rt.svg)"), "{}", lines[11]);
    assert!(lines[12].ends_with(" 200,4 8x8"), "inline SVG has no note: {}", lines[12]);
}

/// print → parse → print is identical, for every kind of line.
#[test]
fn listing_round_trips() {
    let mut s = store();
    let id = everything(&mut s);
    let first = list(&s, id, true);
    let img = s.image(id).unwrap();
    // Feed the element lines (without numbers) back in, on top of nothing: --from the
    // source (same canvas, scale, crop, background) with every element dropped.
    let lines: Vec<String> = first.lines().skip(1).map(|l| l.trim().split_once(' ').unwrap().1.to_string()).collect();
    let n = img.body.elements.len() as u32;
    let r = MakeReq { drop: (1..=n).collect(), ..req(Some(id), &lines.iter().map(|s| s.as_str()).collect::<Vec<_>>()) };
    let again = mk(&mut s, &r).unwrap();
    let tail = |l: &str| l.lines().skip(1).collect::<Vec<_>>().join("\n");
    let second = list(&s, again, true);
    assert_eq!(tail(&first), tail(&second));

    // Numbered lines that copy the listing (rect-form pens included) change nothing.
    let plain = list(&s, id, false);
    let numbered: Vec<String> = plain.lines().skip(1).map(|l| l.trim().to_string()).collect();
    let same = mk(&mut s, &req(Some(id), &numbered.iter().map(|s| s.as_str()).collect::<Vec<_>>())).unwrap();
    assert_eq!(tail(&list(&s, same, true)), tail(&first));
}

#[test]
fn doc_examples_parse() {
    let doc = include_str!("../../../skill/SKILL.md");
    // Every numbered line of every listing block in the doc.
    let mut n = 0;
    for l in doc.lines() {
        let t = l.trim();
        if t.split_whitespace().next().is_some_and(|w| w.parse::<u32>().is_ok()) && l.starts_with("  ") {
            parse_lines(t).unwrap_or_else(|e| panic!("{t}: {e}"));
            n += 1;
        }
    }
    assert!(n >= 5, "found {n} lines");
    for t in [
        "blue box 10,10 200x50",
        "svg <svg width=\"20\" height=\"20\"><circle cx=\"10\" cy=\"10\" r=\"9\" fill=\"red\"/></svg> 40,40",
        "5 red text 640,410 \"much too big\"",
        "ellipse 10,10 20x20",
        "arrow 412,300 -> 640,410",
        "arrow 412,300 → 640,410",
        "BOX 1,2,3,4",
        "raster r7qa 0,0 1280x720",
        "raster ./mock.png 40,40 240x120 from 600,380 120x60",
        "svg s3pa 600,40 64x64",
        "svg mock.svg 0,0",
        "svg <svg viewBox=\"0 0 10 10\"><text x='1'>a b</text></svg> 5,5 20x20",
        "text 1,1 width 100 size 30 \"x\"",
        "6 green pen 1180,380 140x260",
        "6 green pen 1180,380,140,260",
    ] {
        parse_lines(t).unwrap_or_else(|e| panic!("{t}: {e}"));
    }
}

#[test]
fn parse_errors_say_what_is_wrong() {
    let err = |t: &str| parse_lines(t).unwrap_err().to_string();
    assert_eq!(
        err("red box 600,380,100"),
        "in 'red box 600,380,100': \"600,380,100\" isn't a point; expected X,Y like 600,380 (box X,Y WxH)"
    );
    assert_eq!(err("red box 600,380 100,60"), "in 'red box 600,380 100,60': \"100,60\" isn't a size; expected WxH like 100x60 (box X,Y WxH)");
    assert_eq!(err("box 600,380"), "in 'box 600,380': a size must follow \"600,380\"; expected WxH like 100x60 (box X,Y WxH)");
    assert_eq!(err("arrow 1,2 3,4"), "in 'arrow 1,2 3,4': expected \"to\" between the points, not \"3,4\" (arrow X,Y to X,Y)");
    assert_eq!(err("arrow 1,2"), "in 'arrow 1,2': \"to X,Y\" is missing after \"1,2\" (arrow X,Y to X,Y)");
    assert!(err("purple box 1,2 3x4").contains("\"purple\" isn't a color or an element; expected red, yellow, green, blue, white, black, #rrggbb, or box"));
    assert!(err("red circle 1,2 3x4").contains("\"circle\" isn't a mark; expected box, ellipse"));
    assert!(err("text 1,2 hello").contains("expected the text in double quotes, like \"hello\", not \"hello\""));
    assert!(err("text 1,2 \"hello").contains("the closing \" of the text is missing"));
    assert!(err("text 1,2 size big \"a\"").contains("\"big\" isn't a positive number for size"));
    assert!(err("box 1,2 3x4 extra").contains("unexpected \"extra\" after the end of the box"));
    assert!(err("raster k3f9 0,0").contains("k3f9 is an image, not a raster"));
    assert!(err("raster s3pa 0,0").contains("s3pa is an SVG, not a raster; write svg s3pa"));
    assert!(err("svg r3pa 0,0").contains("r3pa is a raster, not an SVG; write raster r3pa"));
    assert!(err("raster mock 0,0").contains("\"mock\" isn't a raster id or a file path (paths need a / or an extension"));
    assert!(err("red raster r7qa 0,0").contains("raster takes no color"));
    assert!(err("svg <svg><rect/> 0,0").contains("inline SVG has no matching </svg>"));
    assert!(err("box 1,2 3x4 (note) more").contains("must be last on the line"));
    assert!(err("0 box 1,2 3x4").contains("element numbers start at 1"));
    assert!(err("raster r7qa 0,0 from 1.5,0 2x2").contains("whole source pixels"));
}

#[test]
fn trailing_notes_and_multiline_input() {
    let l = parse_lines("2 blue box 1,2 3x4 (box (nested) note)\n\n  red text 1,1 \"a (not a note)\" (box 0,0 1x1)\nsvg <svg width='2'\nheight='2'></svg> 0,0").unwrap();
    assert_eq!(l.len(), 3);
    assert_eq!(l[0].num, Some(2));
    match &l[1].spec {
        format::Spec::Mark { mark: format::Mark::Text { text, .. }, .. } => assert_eq!(text, "a (not a note)"),
        o => panic!("{o:?}"),
    }
    assert!(matches!(&l[2].spec, format::Spec::Svg { src: format::Source::Inline(t), .. } if t.contains('\n')));
}

// ------------------------------------------------------------------ make

#[test]
fn make_replace_drop_append() {
    let mut s = store();
    let shot = screenshot(&mut s, 200, 100, 1.0);
    let a = mk(&mut s, &req(Some(shot), &["blue box 10,10 20x20", "green line 0,0 to 5,5", "pen 1,1 3,3 5,1"])).unwrap();
    let b = mk(&mut s, &MakeReq { drop: vec![3], ..req(Some(a), &["2 yellow box 50,50 10x10", "4 pen 0,0 8x4", "arrow 1,1 to 9,9"]) }).unwrap();
    let l = list(&s, b, true);
    let lines: Vec<&str> = l.lines().skip(1).map(str::trim).collect();
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[1], "2 yellow box 50,50 10x10");
    // Pen 1,1..5,3 (4x2) fit into 0,0 8x4: each axis on its own.
    assert_eq!(lines[2], "3 red pen 0,0 4,4 8,0");
    assert_eq!(lines[3], "4 red arrow 1,1 to 9,9");
    let img = s.image(b).unwrap();
    assert_eq!(img.parent, Some(a));
    assert_eq!(img.body.canvas, [200, 100]);
}

#[test]
fn make_errors() {
    let mut s = store();
    let shot = screenshot(&mut s, 200, 100, 1.0);
    let a = mk(&mut s, &req(Some(shot), &["blue box 10,10 20x20"])).unwrap();
    let e = |s: &mut Store, r: MakeReq| mk(s, &r).unwrap_err();
    assert!(e(&mut s, req(None, &["3 box 1,1 2x2"])).contains("there's no --from"));
    assert!(e(&mut s, req(Some(a), &["9 box 1,1 2x2"])).contains(&format!("image {a} has 2 elements; there's no element 9")));
    assert!(e(&mut s, req(Some(a), &["2 box 1,1 2x2", "2 box 1,1 2x2"])).contains("element 2 is replaced twice"));
    assert!(e(&mut s, MakeReq { drop: vec![2], ..req(Some(a), &["2 box 1,1 2x2"]) }).contains("both dropped and replaced"));
    assert!(e(&mut s, MakeReq { drop: vec![5], ..req(Some(a), &[]) }).contains("--drop 5:"));
    assert!(e(&mut s, req(Some(a), &["pen 1,1 4x4"])).contains("only works when replacing a pen"));
    assert!(e(&mut s, req(Some(a), &["2 pen 1,1 4x4"])).contains("that element is a box"));
    assert!(e(&mut s, MakeReq { crop: Some("150,0 100x10".into()), ..req(Some(a), &[]) }).contains("reaches outside the 200x100 canvas"));
    assert!(e(&mut s, MakeReq { crop: Some("1,2,3".into()), ..req(Some(a), &[]) }).contains("crop \"1,2,3\" isn't a rect; expected 'X,Y WxH' or none"));
    assert!(e(&mut s, req(None, &[])).contains("nothing to draw"));
    assert!(e(&mut s, req(None, &["raster ./missing.png 0,0"])).contains("can't read"));
    assert!(e(&mut s, req(None, &["raster r222 0,0"])).contains("there's no r222"));
    assert!(e(&mut s, req(Some(ImageId::parse("k3f9").unwrap()), &[])).contains("there's no k3f9"));
    assert!(e(&mut s, MakeReq { color: Some("teal".into()), ..req(Some(a), &[]) }).contains("--color \"teal\" isn't a color"));
}

#[test]
fn make_from_scratch_defaults() {
    let mut s = store();
    let id = mk(&mut s, &req(None, &["box 10,10 100x50", "text 0,0 \"hi\"\nline 0,0 to 20,30"])).unwrap();
    let img = s.image(id).unwrap();
    // Canvas: bounding box from 0,0 including stroke and text backing.
    assert_eq!(img.body.canvas, [112, 62]);
    assert_eq!(img.body.scale, 1.0);
    assert_eq!(img.body.elements.len(), 3);
    assert!(img.parent.is_none());
    let id = mk(&mut s, &MakeReq { canvas: Some([300, 200]), color: Some("white".into()), crop: Some("0,0,100,100".into()), ..req(None, &[]) }).unwrap();
    let l = list(&s, id, false);
    assert_eq!(l, format!("image {id} 300x200, background white, crop 0,0 100x100"));
    let un = mk(&mut s, &MakeReq { crop: Some("none".into()), color: Some("none".into()), ..req(Some(id), &[]) }).unwrap();
    assert_eq!(list(&s, un, false), format!("image {un} 300x200, parent {id}"));
}

#[test]
fn text_default_size_follows_scale() {
    let mut s = store();
    let shot = screenshot(&mut s, 400, 300, 2.0);
    let a = mk(&mut s, &req(Some(shot), &["text 1,1 \"a\"", "text 1,50 size 18 \"b\""])).unwrap();
    let img = s.image(a).unwrap();
    assert!(matches!(img.body.elements[1].kind, Kind::Text { size, .. } if size == 36.0));
    assert!(matches!(img.body.elements[2].kind, Kind::Text { size, .. } if size == 18.0));
    let l = list(&s, a, false);
    assert!(l.contains("  2 red text 1,1 \"a\" (box"), "{l}");
    assert!(l.contains("  3 red text 1,50 size 18 \"b\""), "{l}");
}

// ------------------------------------------------------------------ assets

#[test]
fn rendered_pngs_are_refused() {
    let mut s = store();
    let id = screenshot(&mut s, 20, 10, 1.0);
    let r = rend(&s, id, RenderOpts::default());
    let p = tmp().join("refuse.png");
    put(&p, r.png(Some(id))).unwrap();
    match load_file(&p) {
        Err(e @ LoadError::Rendering(_)) => assert_eq!(e.to_string(), format!("that file is a rendering of image {id}; use --from {id}")),
        o => panic!("{o:?}"),
    }
    let e = mk(&mut s, &req(None, &["raster ./refuse.png 0,0"])).unwrap_err();
    assert!(e.contains(&format!("rendering of image {id}; use --from {id}")), "{e}");
    // Drafts are marked too.
    put(&p, r.png(None)).unwrap();
    assert!(matches!(load_file(&p), Err(LoadError::Rendering(d)) if d == "draft"));
    // With the chunk stripped (an outside tool), it loads.
    put(&p, strip_mark(&r.png(Some(id)))).unwrap();
    let a = load_file(&p).unwrap();
    assert_eq!(a.as_raster().unwrap().size(), [20, 10]);
}

#[test]
fn loads_png_variants_and_svg() {
    let d = tmp();
    // Grayscale+alpha PNG: half-transparent mid grey.
    let p = d.join("ga.png");
    {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, 2, 1);
        enc.set_color(png::ColorType::GrayscaleAlpha);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(&[200, 128, 10, 255]).unwrap();
        w.finish().unwrap();
        put(&p, out).unwrap();
    }
    let a = load_file(&p).unwrap();
    let r = a.as_raster().unwrap();
    assert_eq!(&r.premultiplied()[..8], &[100, 100, 100, 128, 10, 10, 10, 255]);
    assert_eq!(*r.origin(), Origin::File { name: "ga.png".into() });
    put(d.join("x.svg"), "<svg viewBox='0 0 30 10'><rect width='30' height='10'/></svg>").unwrap();
    let a = load_file(&d.join("x.svg")).unwrap();
    assert_eq!(a.natural_size(), Size { w: 30.0, h: 10.0 });
    put(d.join("junk.png"), "hello").unwrap();
    assert!(load_file(&d.join("junk.png")).unwrap_err().to_string().contains("not a PNG, JPEG or SVG"));
}

#[test]
fn loads_jpeg() {
    // A tiny 8x8 baseline JPEG (solid colour), made with ImageMagick.
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/red8.jpg");
    let a = load_file(&p).unwrap();
    let r = a.as_raster().unwrap();
    assert_eq!(r.size(), [8, 8]);
    let px = &r.premultiplied()[..4];
    assert!(px[0] > 200 && px[1] < 40 && px[2] < 40 && px[3] == 255, "{px:?}");
}

// ------------------------------------------------------------------ render

#[test]
fn render_box_edge_and_background() {
    let mut s = store();
    let id = mk(&mut s, &MakeReq { canvas: Some([60, 40]), color: Some("white".into()), ..req(None, &["blue box 10,10 30x20"]) }).unwrap();
    let r = rend(&s, id, RenderOpts::default());
    save("box", &r);
    assert_eq!(r.size(), [60, 40]);
    assert_eq!(r.pixel(10, 20), Color::BLUE.0, "on the left edge (stroke 3 px centred on x=10)");
    assert_eq!(r.pixel(25, 10), Color::BLUE.0, "top edge");
    assert_eq!(r.pixel(25, 20), Color::WHITE.0, "inside is empty");
    assert_eq!(r.pixel(5, 5), Color::WHITE.0);
    assert!(close(r.pixel(12, 20), Color::WHITE.0, 0), "1.5 px stroke half ends before x=12");
    // marks: false leaves only the background.
    let r = rend(&s, id, RenderOpts { marks: false, ..Default::default() });
    assert_eq!(r.pixel(10, 20), Color::WHITE.0);
}

#[test]
fn render_text_glyphs_and_backing() {
    let mut s = store();
    let id = mk(&mut s, &MakeReq { canvas: Some([200, 60]), color: Some("white".into()), ..req(None, &["yellow text 10,10 \"Hello\""]) }).unwrap();
    let r = rend(&s, id, RenderOpts::default());
    save("text", &r);
    let img = s.image(id).unwrap();
    let bounds = img.body.elements[0].bounds(1.0, fonts()).round_out();
    let (mut text_px, mut dark_px) = (0, 0);
    for y in bounds.y as u32..bounds.bottom() as u32 {
        for x in bounds.x as u32..bounds.right() as u32 {
            let p = r.pixel(x, y);
            if close(p, Color::YELLOW.0, 30) {
                text_px += 1;
            }
            if p[0] < 110 && p[2] < 110 {
                dark_px += 1;
            }
        }
    }
    assert!(text_px > 40, "glyphs drawn: {text_px}");
    assert!(dark_px > 200, "dark backing drawn: {dark_px}");
    assert_eq!(r.pixel(bounds.right() as u32 + 3, 20), Color::WHITE.0, "nothing right of the box");
    // Black text gets a light backing.
    let id = mk(&mut s, &MakeReq { canvas: Some([200, 60]), color: Some("black".into()), ..req(None, &["black text 10,10 \"Hi\""]) }).unwrap();
    let r = rend(&s, id, RenderOpts::default());
    assert!(r.pixel(8, 8)[0] > 150, "light backing: {:?}", r.pixel(8, 8));
}

#[test]
fn text_wraps_at_width() {
    let f = fonts();
    let l = f.layout("one two three four five", 18.0, Some(80.0));
    assert!(l.lines.len() >= 3, "{:?}", l.lines.iter().map(|l| &l.text).collect::<Vec<_>>());
    assert!(l.lines.iter().all(|x| x.w <= 80.0));
    assert!(l.lines.iter().all(|x| !x.text.starts_with(' ') && !x.text.ends_with(' ')));
    let l = f.layout("a\nb", 18.0, None);
    assert_eq!(l.lines.len(), 2);
    assert_eq!(l.size[1], 2.0 * l.line_height);
    // A word longer than the width breaks inside.
    let l = f.layout("abcdefghijklmnopqrstuvwxyz", 18.0, Some(50.0));
    assert!(l.lines.len() > 2 && l.lines.iter().all(|x| x.w <= 50.0));
    // Emoji come from the fallback fonts.
    let l = f.layout("ok 😀", 18.0, None);
    assert!(l.lines[0].glyphs.iter().any(|g| g.face > 0));
}

#[test]
fn render_raster_exact_crop_and_max() {
    let mut s = store();
    let shot = screenshot(&mut s, 200, 100, 1.0);
    // Whole-pixel placement at 1x is pixel exact.
    let r = rend(&s, shot, RenderOpts::default());
    assert_eq!(r.pixel(37, 81), [37, 81, 100, 255]);
    // Image crop.
    let c = mk(&mut s, &MakeReq { crop: Some("50,20 100x40".into()), ..req(Some(shot), &[]) }).unwrap();
    let r = rend(&s, c, RenderOpts::default());
    assert_eq!(r.size(), [100, 40]);
    assert_eq!(r.shows, IRect::new(50, 20, 100, 40));
    assert_eq!(r.pixel(0, 0), [50, 20, 100, 255]);
    // Region outside the image crop, clipped to the canvas.
    let r = rend(&s, c, RenderOpts { region: Region::Rect(IRect::new(150, 50, 100, 100)), ..Default::default() });
    assert_eq!(r.shows, IRect::new(150, 50, 50, 50));
    assert_eq!(r.pixel(0, 0), [150, 50, 100, 255]);
    assert_eq!(rend(&s, c, RenderOpts { region: Region::Canvas, ..Default::default() }).size(), [200, 100]);
    let img = s.image(c).unwrap();
    assert_eq!(
        render(&img.body, &s, fonts(), &RenderOpts { region: Region::Rect(IRect::new(300, 0, 5, 5)), ..Default::default() }).unwrap_err(),
        RenderError::Outside(IRect::new(300, 0, 5, 5))
    );
    // max shrinks, never enlarges.
    let r = rend(&s, shot, RenderOpts { max: Some([50, 50]), ..Default::default() });
    assert_eq!((r.size(), r.zoom), ([50, 25], 0.25));
    let p = r.pixel(10, 10);
    assert!(close(p, [41, 41, 100, 255], 3), "box-filtered mean around canvas 40..44: {p:?}");
    let r = rend(&s, shot, RenderOpts { max: Some([1000, 1000]), ..Default::default() });
    assert_eq!((r.size(), r.zoom), ([200, 100], 1.0));
}

#[test]
fn render_raster_source_crop_scaled() {
    let mut s = store();
    let shot = screenshot(&mut s, 200, 100, 1.0);
    let src = s.image(shot).unwrap().body.elements[0].kind.asset().unwrap();
    let id = mk(&mut s, &MakeReq { canvas: Some([100, 100]), ..req(None, &[&format!("raster {src} 10,10 40x20 from 100,50 20x10")]) }).unwrap();
    let r = rend(&s, id, RenderOpts::default());
    save("raster-from", &r);
    assert_eq!(r.pixel(5, 5)[3], 0, "transparent outside");
    let p = r.pixel(30, 20);
    // Canvas 30,20 is source 100 + 20/2, 50 + 10/2.
    assert!(close(p, [110, 55, 100, 255], 3), "{p:?}");
    // Default size of `from` is the from-rect's size.
    let id = mk(&mut s, &req(None, &[&format!("raster {src} 0,0 from 100,50 20x10")])).unwrap();
    assert_eq!(s.image(id).unwrap().body.canvas, [20, 10]);
}

#[test]
fn render_svg_opaque_element() {
    let mut s = store();
    put(tmp().join("red.svg"), SVG_RED).unwrap();
    let id = mk(
        &mut s,
        &MakeReq {
            canvas: Some([120, 60]),
            ..req(None, &["svg ./red.svg 10,10", "svg <svg viewBox='0 0 2 2'><rect width='1' height='2' fill=\"#00f\"/></svg> 60,10 40x40"])
        },
    )
    .unwrap();
    let r = rend(&s, id, RenderOpts::default());
    save("svg", &r);
    assert_eq!(r.pixel(30, 20), [255, 0, 0, 255]);
    assert_eq!(r.pixel(55, 35)[3], 0, "outside the 40x20 svg");
    assert_eq!(r.pixel(70, 20), [0, 0, 255, 255], "left half of the inline svg");
    assert_eq!(r.pixel(90, 20)[3], 0, "right half is empty");
    let l = list(&s, id, false);
    assert!(l.contains(" 10,10 40x20 (from red.svg)"), "{l}");
    // Pictures stay with marks off.
    assert_eq!(rend(&s, id, RenderOpts { marks: false, ..Default::default() }).pixel(30, 20), [255, 0, 0, 255]);
    // svg_pixmap renders at any size.
    let img = s.image(id).unwrap();
    let a = s.asset(img.body.elements[0].kind.asset().unwrap()).unwrap();
    let p = svg_pixmap(a.as_svg().unwrap(), [80, 40]);
    assert_eq!(p.pixel(79, 39), [255, 0, 0, 255]);
}

#[test]
fn huge_renders_are_shrunk() {
    let mut s = store();
    let id = mk(&mut s, &MakeReq { canvas: Some([16384, 16384]), ..req(None, &[]) }).unwrap();
    let r = rend(&s, id, RenderOpts::default());
    assert!(r.size()[0] as u64 * r.size()[1] as u64 <= model::MAX_PIXELS && r.zoom < 0.5, "{r:?}");
}

#[test]
fn svg_text_uses_system_fonts() {
    let svg = "<svg width='60' height='30'><text x='2' y='24' font-size='24' fill='black'>Hi</text></svg>";
    let doc = SvgDoc::parse(svg, Origin::File { name: "t".into() }, None).unwrap();
    let r = svg_pixmap(&doc, [60, 30]);
    let inked = r.premultiplied().as_chunks::<4>().0.iter().filter(|p| p[3] > 128).count();
    // No system fonts at all (bare containers) renders nothing; say so rather than fail.
    if doc.tree().fontdb().is_empty() {
        eprintln!("no system fonts; skipping");
        return;
    }
    assert!(inked > 20, "{inked}");
}

/// A busy annotated screenshot, for checking by eye (target/image-tests/annotated.png).
#[test]
fn render_annotated_sample() {
    let mut s = store();
    let shot = screenshot(&mut s, 640, 400, 2.0);
    let id = mk(
        &mut s,
        &req(
            Some(shot),
            &[
                "blue box 40,40 200x80",
                "yellow ellipse 300,60 160x90",
                "green line 40,200 to 240,200",
                "red arrow 500,300 to 330,140",
                "white pen 60,260 90,300 140,280 180,330 230,290",
                "text 300,250 \"too big 😀\"",
                "blue text 40,320 size 20 width 200 \"wrapped text that goes on and on\"",
                "black text 480,30 \"dark\"",
            ],
        ),
    )
    .unwrap();
    let r = rend(&s, id, RenderOpts::default());
    save("annotated", &r);
    let r = rend(&s, id, RenderOpts { max: Some([200, 200]), ..Default::default() });
    save("annotated-small", &r);
    assert_eq!(r.size(), [200, 125]);
}

// ------------------------------------------------------------------ store

#[test]
fn expiry_and_holds() {
    let mut s = Store::with_ids(Limits { idle: Duration::from_secs(60), budget: usize::MAX }, IdGen::seeded(3));
    let a = screenshot(&mut s, 10, 10, 1.0);
    let b = screenshot(&mut s, 10, 10, 1.0);
    let child = mk(&mut s, &req(Some(a), &["box 1,1 2x2"])).unwrap();
    s.hold(b);
    let now = Instant::now();
    assert_eq!(s.sweep(now), 0);
    s.image(child).unwrap();
    // a: idle, but its raster is still used by child.
    let freed = s.sweep(now + Duration::from_secs(61));
    assert_eq!(freed, 400, "a and child expire, with a's raster; b is held");
    assert!(matches!(s.image(a), Err(Lookup::Expired(_))));
    assert_eq!(s.lookup(&a.to_string()).unwrap_err().to_string(), format!("{a} has expired (unused images are dropped after a while)"));
    assert!(s.image(b).is_ok());
    s.release(b);
    s.sweep(now + Duration::from_secs(200));
    assert!(s.is_empty() && s.bytes() == 0);
    // Ids are never reused.
    for _ in 0..50 {
        let n = screenshot(&mut s, 1, 1, 1.0);
        assert!(n != a && n != b && n != child);
    }
}

#[test]
fn parent_note_expired() {
    let mut s = Store::with_ids(Limits { idle: Duration::from_secs(10), budget: usize::MAX }, IdGen::seeded(4));
    let a = screenshot(&mut s, 10, 10, 1.0);
    let b = mk(&mut s, &req(Some(a), &["box 1,1 2x2"])).unwrap();
    s.hold(b);
    assert!(list(&s, b, false).starts_with(&format!("image {b} 10x10, parent {a} (your screenshot)")));
    s.sweep(Instant::now() + Duration::from_secs(11));
    assert!(list(&s, b, false).starts_with(&format!("image {b} 10x10, parent {a} (expired)")));
    // The raster survives through b.
    rend(&s, b, RenderOpts::default());
}

#[test]
fn budget_evicts_lru_unheld_then_oldest_held() {
    // Each 10x10 screenshot is 400 bytes; budget fits 2.
    let mut s = Store::with_ids(Limits { idle: Duration::from_secs(3600), budget: 800 }, IdGen::seeded(5));
    let ids: Vec<ImageId> = (0..4).map(|_| screenshot(&mut s, 10, 10, 1.0)).collect();
    s.hold(ids[0]);
    s.hold(ids[1]);
    std::thread::sleep(Duration::from_millis(5));
    s.image(ids[2]).unwrap(); // ids[3] is now the least recently used unheld
    s.sweep(Instant::now());
    assert_eq!(s.bytes(), 800);
    assert!(s.image(ids[3]).is_err() && s.image(ids[2]).is_err(), "unheld go first");
    assert!(s.image(ids[0]).is_ok() && s.image(ids[1]).is_ok());
    // Held alone over budget: oldest held goes.
    let more = screenshot(&mut s, 10, 10, 1.0);
    s.hold(more);
    s.sweep(Instant::now());
    assert!(s.image(ids[0]).is_err());
    assert!(s.image(ids[1]).is_ok() && s.image(more).is_ok());
}

#[test]
fn assets_refcounts_groups_and_carried() {
    let mut s = store();
    let shot = screenshot(&mut s, 10, 10, 1.0);
    let raster = s.image(shot).unwrap().body.elements[0].kind.asset().unwrap();
    // A draft carries its assets; after the store evicts them, add_image re-adds them.
    let carried = vec![(raster, s.asset(raster).unwrap())];
    let body = (*s.image(shot).unwrap().body).clone();
    s.drop_group(G);
    assert!(s.asset(raster).is_none());
    assert_eq!(s.add_image(None, body.clone(), Made::Human, Group(2), &[]).unwrap_err(), StoreError::MissingAsset { element: 1, asset: raster });
    let id = s.add_image(Some(shot), body, Made::Human, Group(2), &carried).unwrap();
    assert!(s.asset(raster).is_some());
    assert_eq!(s.image(id).unwrap().body.elements[0].kind.asset(), Some(raster));
    // Unused assets go at the next sweep.
    let lone = s.add_asset(Asset::Raster(capture(5, 5)), G);
    s.sweep(Instant::now());
    assert!(s.asset(lone).is_none());
    assert!(matches!(s.get_asset(lone), Err(Lookup::Expired(_))));
    // Wrong asset kind.
    let svg = s.add_asset(Asset::Svg(Box::new(SvgDoc::parse(SVG_RED, Origin::File { name: "x".into() }, None).unwrap())), G);
    let mut b = Body::new([10, 10], 1.0);
    b.elements.push(Element::picture(Kind::Raster { asset: svg, dst: Rect::new(0.0, 0.0, 1.0, 1.0), src: None }));
    assert!(matches!(s.add_image(None, b, Made::Agent, G, &[]), Err(StoreError::WrongAsset { .. })));
    s.drop_group(Group(2));
    s.drop_group(G);
    assert_eq!(s.bytes(), 0);
}

#[test]
fn limits_are_checked() {
    let mut s = store();
    let mut b = Body::new([10, 10], 1.0);
    b.elements = vec![Element::new(Color::RED, Kind::Box(Rect::new(0.0, 0.0, 1.0, 1.0))); model::MAX_ELEMENTS + 1];
    assert!(s.add_image(None, b, Made::Agent, G, &[]).unwrap_err().to_string().contains("too many"));
    let b = Body::new([20_000, 10], 1.0);
    assert!(s.add_image(None, b, Made::Agent, G, &[]).unwrap_err().to_string().contains("out of range"));
    let b = Body { crop: Some(IRect::new(5, 5, 10, 10)), ..Body::new([10, 10], 1.0) };
    assert!(s.add_image(None, b, Made::Agent, G, &[]).unwrap_err().to_string().contains("isn't inside"));
    let mut b = Body::new([10, 10], 1.0);
    b.elements.push(Element::new(Color::RED, Kind::Pen(vec![pt(0.0, 0.0); model::MAX_PEN_POINTS + 1])));
    assert!(s.add_image(None, b, Made::Agent, G, &[]).unwrap_err().to_string().contains("pen points"));
}

// ------------------------------------------------------------------ geometry and edits

#[test]
fn hit_testing() {
    let f = fonts();
    let bx = Element::new(Color::RED, Kind::Box(Rect::new(10.0, 10.0, 100.0, 50.0)));
    assert!(bx.hit(pt(10.0, 30.0), 2.0, 1.0, f));
    assert!(bx.hit(pt(13.0, 30.0), 2.0, 1.0, f), "within tol + half stroke");
    assert!(!bx.hit(pt(50.0, 30.0), 2.0, 1.0, f), "boxes are hit on their outline only");
    let el = Element::new(Color::RED, Kind::Ellipse(Rect::new(0.0, 0.0, 100.0, 50.0)));
    assert!(el.hit(pt(100.0, 25.0), 2.0, 1.0, f));
    assert!(el.hit(pt(50.0, 1.0), 2.0, 1.0, f));
    assert!(!el.hit(pt(50.0, 25.0), 2.0, 1.0, f));
    let ar = Element::new(Color::RED, Kind::Arrow(pt(0.0, 0.0), pt(100.0, 0.0)));
    assert!(ar.hit(pt(88.0, 5.0), 0.0, 1.0, f), "head is wider than the shaft");
    assert!(!ar.hit(pt(10.0, 5.0), 0.0, 1.0, f));
    let tx = Element::new(Color::RED, Kind::Text { at: pt(10.0, 10.0), size: 18.0, width: None, text: "hello".into() });
    assert!(tx.hit(pt(8.0, 8.0), 0.0, 1.0, f), "inside the backing");
    let pic = Element::picture(Kind::Svg { asset: AssetId::parse("s222").unwrap(), dst: Rect::new(0.0, 0.0, 10.0, 10.0) });
    assert!(pic.hit(pt(5.0, 5.0), 0.0, 1.0, f));
}

#[test]
fn key_points_and_drags() {
    let f = fonts();
    let bx = Element::new(Color::RED, Kind::Box(Rect::new(10.0, 10.0, 100.0, 50.0)));
    let k = bx.key_points(1.0, f);
    assert_eq!(k.len(), 4);
    assert_eq!((k[2].key, k[2].at), (Key::Corner(2), pt(110.0, 60.0)));
    // Free resize for marks; Shift keeps the aspect ratio.
    let d = bx.drag_key(Key::Corner(2), pt(210.0, 70.0), Mods::default(), 1.0, f);
    assert_eq!(d.kind, Kind::Box(Rect::new(10.0, 10.0, 200.0, 60.0)));
    let d = bx.drag_key(Key::Corner(2), pt(210.0, 70.0), Mods { shift: true }, 1.0, f);
    assert_eq!(d.kind, Kind::Box(Rect::new(10.0, 10.0, 200.0, 100.0)));
    // Can't cross the opposite corner.
    let d = bx.drag_key(Key::Corner(0), pt(500.0, 500.0), Mods::default(), 1.0, f);
    assert_eq!(d.kind, Kind::Box(Rect::new(106.0, 56.0, 4.0, 4.0)));
    // Pictures keep their aspect ratio by default.
    let pic = Element::picture(Kind::Svg { asset: AssetId::parse("s222").unwrap(), dst: Rect::new(0.0, 0.0, 40.0, 20.0) });
    let d = pic.drag_key(Key::Corner(2), pt(80.0, 20.0), Mods::default(), 1.0, f);
    assert_eq!(d.kind, Kind::Svg { asset: AssetId::parse("s222").unwrap(), dst: Rect::new(0.0, 0.0, 80.0, 40.0) });
    // Line ends.
    let ln = Element::new(Color::RED, Kind::Line(pt(0.0, 0.0), pt(10.0, 10.0)));
    assert_eq!(ln.key_points(1.0, f)[1].key, Key::End(1));
    assert_eq!(ln.drag_key(Key::End(1), pt(5.0, 50.0), Mods::default(), 1.0, f).kind, Kind::Line(pt(0.0, 0.0), pt(5.0, 50.0)));
    // Text scales by height, anchored at the opposite corner.
    let tx = Element::new(Color::RED, Kind::Text { at: pt(100.0, 100.0), size: 20.0, width: Some(200.0), text: "hi".into() });
    let fr = tx.frame(1.0, f);
    let to = pt(fr.min.x - 50.0, fr.min.y - (fr.size.h - 8.0)); // double the inner height, dragging top-left
    let d = tx.drag_key(Key::Corner(0), to, Mods::default(), 1.0, f);
    let Kind::Text { size, width, .. } = d.kind.clone() else { panic!() };
    assert!((size - 40.0).abs() < 0.01 && (width.unwrap() - 400.0).abs() < 0.05, "{size} {width:?}");
    let nf = d.frame(1.0, f);
    assert!((nf.max().x - fr.max().x).abs() < 1.0 && (nf.max().y - fr.max().y).abs() < 1.0, "anchored: {nf:?} vs {fr:?}");
    // Translate.
    assert_eq!(ln.translated(pt(1.0, 2.0)).kind, Kind::Line(pt(1.0, 2.0), pt(11.0, 12.0)));
}

#[test]
fn display_list_order_and_base_lock() {
    let mut s = store();
    let shot = screenshot(&mut s, 100, 100, 1.0);
    let id = mk(&mut s, &req(Some(shot), &["arrow 0,0 to 50,50", "text 1,1 \"x\""])).unwrap();
    let img = s.image(id).unwrap();
    let dl = display_list(&img.body, fonts(), true);
    let kinds: Vec<(usize, &str)> = dl
        .iter()
        .map(|(i, p)| {
            (*i, match p {
                Prim::Raster { .. } => "raster",
                Prim::Stroke { .. } => "stroke",
                Prim::Fill { .. } => "fill",
                Prim::RoundRect { .. } => "backing",
                Prim::Text { .. } => "text",
                _ => "?",
            })
        })
        .collect();
    assert_eq!(kinds, vec![(0, "raster"), (1, "stroke"), (1, "fill"), (2, "backing"), (2, "text")]);
    assert_eq!(display_list(&img.body, fonts(), false).len(), 1);
    assert!(img.body.is_base(0) && !img.body.is_base(1));
}

#[test]
fn doc_apply_revert() {
    let mut s = store();
    let shot = screenshot(&mut s, 100, 100, 1.0);
    let orig = (*s.image(shot).unwrap().body).clone();
    let mut d = Doc::from_body(orig.clone());
    let e1 = d.add_edit(Element::new(Color::BLUE, Kind::Box(Rect::new(1.0, 1.0, 5.0, 5.0))));
    d.apply(&e1);
    let k = d.keys()[1];
    let e2 = d.replace_edit(k, Element::new(Color::GREEN, Kind::Line(pt(0.0, 0.0), pt(1.0, 1.0)))).unwrap();
    d.apply(&e2);
    let e3 = d.crop_edit(Some(IRect::new(0, 0, 50, 50)));
    d.apply(&e3);
    let e4 = d.remove_edit(d.keys()[0]).unwrap();
    d.apply(&e4);
    assert_eq!(d.body().elements.len(), 1);
    assert_eq!(d.body().elements[0].color, Color::GREEN);
    assert_eq!(d.rev(), 4);
    for e in [&e4, &e3, &e2, &e1] {
        d.revert(e);
    }
    assert_eq!(*d.body(), orig);
}

/// Copy-on-edit redo replays edits on a fresh doc: keys made afterwards must stay unique.
#[test]
fn doc_replay_keeps_keys_unique() {
    let mut s = store();
    let shot = screenshot(&mut s, 100, 100, 1.0);
    let body = (*s.image(shot).unwrap().body).clone();
    let bx = |x: f32| Element::new(Color::BLUE, Kind::Box(Rect::new(x, 1.0, 5.0, 5.0)));
    let mut a = Doc::from_body(body.clone());
    let e = a.add_edit(bx(1.0));
    a.apply(&e);
    let mut b = Doc::from_body(body);
    b.apply(&e);
    let e2 = b.add_edit(bx(20.0));
    b.apply(&e2);
    let mut keys = b.keys().to_vec();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), 3, "{:?}", b.keys());
}
