//! guilab images: model, ids, store with expiry, the text format, geometry, the display
//! list and a CPU renderer. No egui; the editor paints the same display list.

pub mod asset;
pub mod draw;
pub mod edit;
pub mod format;
pub mod geom;
pub mod id;
pub mod make;
pub mod model;
pub mod render;
pub mod store;
pub mod text;

pub use asset::{Asset, LoadError, Origin, Raster, SvgDoc, load_bytes, load_file};
pub use draw::{Prim, display_list, element_prims};
pub use edit::{Doc, DocId, Edit, EditError, ElemKey, Op};
pub use id::{AssetId, IdError, IdKind, ImageId};
pub use make::{MakeReq, make};
pub use model::{Body, Color, Element, IRect, Image, Kind, Made, Pt, Rect, Size, pt};
pub use render::{Layout, Region, RenderError, RenderOpts, Rendered, layout, render, svg_pixmap};
pub use store::{Assets, Group, Limits, Lookup, NoAssets, Store, StoreError};
pub use text::{Fonts, TextLayout};

#[cfg(test)]
mod tests;
