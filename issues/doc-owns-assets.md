# Make a Doc carry its assets (drafts, painting and freeze from one place)

**Land before phase 6** (it shapes the viewer's `Draft` and the editor's `Canvas`).

## Problem
The planned viewer `Draft { doc, parent, carried: Vec<(AssetId, Arc<Asset>)> }` keeps the assets a draft uses *beside* the doc, and the editor paints from `Canvas.assets: &dyn Assets` (lib.rs:106), which the viewer can only point at the read-only `&Store`. Three ways this goes wrong:
- An asset that enters the store for a draft (phase 7 File → Insert, or any picture an agent image had) has no image referencing it yet. `Store::sweep` drops unreferenced assets within a second (store.rs:298). The editor then paints the picture as the gray "missing" box (paint.rs:338). The planned `carried` list fixes freezing but not painting, unless the viewer writes its own layered `Assets`.
- The glue must keep `carried` in sync with every edit, undo and redo (insert, remove, undo of a remove…). That's an invariant nothing enforces, and a missed asset fails at send with `StoreError::MissingAsset`.
- `Editor::insert(doc, Picture, natural, ..)` (lib.rs:263) restates what the asset already knows. `Picture::Raster(id_of_an_svg)` builds an element that fails at send (`WrongAsset`). The id prefix and the `Asset` already give both the kind and the natural size.

## Shape
- `Doc` holds `assets: HashMap<AssetId, Arc<Asset>>` and `impl Assets for Doc`. `Doc::open(&Image, &dyn Assets) -> Result<Doc, MissingAsset>` (or `from_body` + assets) fills it, and `Doc::carried()` hands it to `Store::add_image(.., carried)`.
- `Edit::Add`/`Replace` of a picture carry the `Arc<Asset>`. Or simpler: the doc keeps every asset it ever saw (cheap, since they're `Arc`s) and prunes on `into_body`.
- `Editor::insert(doc, id: AssetId, asset: Arc<Asset>, center, fit) -> Edit`: kind and natural size come from the asset. Delete `Picture`.
- `Canvas.assets` goes away: with a doc the editor paints from the doc, and without one it draws only pending marks (no pictures).
- The viewer's `Draft` is then `{ key, doc, parent }`, and `DemoHost` drops its asset plumbing.

Update `image-system-design.md` (Edits, guilab-editor API, Workspace/Draft, "Holds").
