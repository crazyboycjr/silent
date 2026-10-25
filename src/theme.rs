//! Warm neutrals for light and dark desktops.

use gpui_kit::{Hsla, WindowAppearance, rgb, rgba};
use silent::detect::Kind;

#[derive(Clone, Copy)]
pub struct Theme {
    /// Cards.
    pub surface: Hsla,
    /// Behind transparent images.
    pub backdrop: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub strip: Hsla,
    pub strip_text: Hsla,
}

impl Theme {
    pub fn of(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                surface: rgb(0x26211d).into(),
                backdrop: rgb(0x1c1916).into(),
                text: rgb(0xede5d9).into(),
                muted: rgb(0xa5998b).into(),
                strip: rgba(0x0f0c0ac0).into(),
                strip_text: rgb(0xf3ebdf).into(),
            },
            _ => Self {
                surface: rgb(0xf6f1e9).into(),
                backdrop: rgb(0xebe4d8).into(),
                text: rgb(0x2b2520).into(),
                muted: rgb(0x85786b).into(),
                strip: rgba(0x1d1813b8).into(),
                strip_text: rgb(0xf8f2e9).into(),
            },
        }
    }
}

/// The colour a kind of file is drawn in on its card.
pub fn accent(kind: Kind) -> Hsla {
    rgb(match kind {
        Kind::Image => 0xc9733a,
        Kind::Video => 0xb4553f,
        Kind::Audio => 0xa35d7e,
        Kind::Pdf => 0xb8443a,
        Kind::Text => 0x5f8063,
        Kind::Archive => 0x9a7340,
        Kind::Folder => 0xd4a24c,
        Kind::Other => 0x8b7e70,
    })
    .into()
}
