//! The vim profile's text engine (piece 3). Task 4 fills this in.

pub mod buf;
mod class;
mod class_table;
pub mod settings;

pub use buf::{BodyBuf, GuiSel, OneLineBuf, Paint, Pos, TextBuf};
