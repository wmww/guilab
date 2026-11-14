# Images: agent doc text (tested)

The "Images" section for `skill/SKILL.md`, tested on 6 fresh agents (see `image-system-design.md` "Text format"). Phase 3 copies it into the skill doc and deletes this note. Keep the wording when you trim it: each sentence answers a question a tester asked.

---

## Images
Screenshots, the human's drawings and images you send are *images*: an id (like `k3f9`), a canvas size, an optional crop, and a numbered list of elements, drawn in order (later ones on top). Images never change. Every edit makes a new image with a new id, and its `parent` is the image it came from.

```
image m2x8 1280x720 scale 2, parent k3f9, crop 300,200 700x400
  1 raster r7qa 0,0 1280x720 (screenshot 14:05:12)
  2 red box 600,380 100x60
  3 red arrow 412,300 to 640,410
  4 green pen 300,500 120x60 (23 points)
  5 red text 640,410 "too big" (box 632,402 98x52)
```
- All coordinates are canvas pixels, even when the image is cropped or rendered smaller. Canvas pixels are screenshot pixels and `guilab input` coordinates. `X,Y WxH` is a rectangle: its top-left corner and size.
- `scale 2` is the UI scale of the screen the image came from. Line widths and the default text size are multiplied by it, so marks look the same size as on screen. Images you make from scratch have scale 1.
- `crop` is the part of the canvas the image shows. Elements outside it are kept, just not shown.
- `(...)` at the end of a line is extra information, such as a text's box or a pen's point count. It's ignored when you write it.

### Reading
`guilab image ID` renders the image to PNG and prints the path, plus which part of the canvas the PNG shows. `--crop 'X,Y WxH'` renders that part of the canvas instead of the image's own crop. It may reach outside the crop, and `--crop none` renders the whole canvas. `--max WxH` shrinks the result. A crop at full size shows more detail than a shrunk whole image, for less context. `--list` prints the element list instead of rendering.

### Element lines (you read and write the same syntax)
- Marks. COLOR is red, yellow, green, blue, white, black or #rrggbb, default red.
  - `[COLOR] box X,Y WxH` and `[COLOR] ellipse X,Y WxH`. The ellipse fills that rectangle.
  - `[COLOR] line X,Y to X,Y` and `[COLOR] arrow X,Y to X,Y`. The arrow points at the second point.
  - `[COLOR] pen X,Y X,Y X,Y ...` is a freehand stroke through these points. Listings show only its bounding rectangle `X,Y WxH`; `--list --points` shows the points.
  - `[COLOR] text X,Y [size N] [width N] "TEXT"`. X,Y is the text's top-left. `size` is the font size in pixels; the default is 18 × scale, and a size you give is used as is. `width` wraps the text at that many pixels; by default it doesn't wrap. Inside the quotes, `\n` is a newline and `\"` a quote. Text is drawn on a dark backing, so it's readable on any background.
  - Options you leave out take their defaults. Listings also leave out options that have their default.
- Pictures:
  - `raster ID|FILE X,Y [WxH] [from X,Y WxH]` places the pixels of a raster or of a PNG/JPEG file at X,Y. Raster ids start with `r`, and every screenshot is a raster. By default one source pixel is one canvas pixel. WxH scales it. `from` takes only that part of the source, in the source's pixels, and the default size is then that part's size.
  - `svg ID|FILE|<svg ...</svg> X,Y [WxH]` is an SVG picture, drawn and moved as a unit. SVG ids start with `s`. Inline SVG runs from `<svg` to the matching `</svg>` and may contain spaces and quotes; `xmlns` is optional. The size defaults to the SVG's own width/height or viewBox.
  - FILE paths must contain a `/` or an extension, e.g. `./mock.png`. Files are read once, when the image is made. A PNG that guilab rendered is refused; use its image id instead.

### Making images
```
guilab image make [--from ID] [--canvas WxH] [--color C] [--crop 'X,Y WxH' | --crop none] [--drop N,N...] [ELEMENT...]
```
It prints the new image's listing and the path of a preview PNG. Check the preview before sending.
- `--from ID` starts from a copy of image ID: its canvas, scale, crop and elements.
  - An element that starts with a number replaces that element of ID, in place. Copy its line and change what you need. A pen line with a new `X,Y WxH` moves and scales that stroke into the rectangle, each axis on its own.
  - Elements without a number are added on top.
  - `--drop 3,5` removes elements by number.
  - Numbers always refer to ID's listing, and the new image is numbered afresh. Don't drop and replace the same number.
  - `--crop` replaces the crop.
- Without `--from`, `--canvas` sets the size. By default the canvas is just big enough for the elements, counting from 0,0. `--color` sets the background; the default is transparent.
- Each ELEMENT is one shell argument, e.g. `'blue box 10,10 200x50'` or `'5 red text 640,410 "much too big"'`. A lone `-` reads more element lines from stdin, one per line, at that position. Numbered lines work there too.
- To send an image to the human: `guilab done "MESSAGE" --image ID`.

When the human sends a drawing, `poll` shows its listing and a command that renders it cropped to the marks. Run it: the listing says where the marks are, the pixels show what they point at.
