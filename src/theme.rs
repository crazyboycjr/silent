//! Warm neutrals for light and dark desktops.

use gpui_kit::{App, Global, Hsla, Window, WindowAppearance, rgb, rgba};
use silent::detect::Kind;

#[derive(Clone, Copy)]
pub struct Theme {
    /// Cards.
    pub surface: Hsla,
    /// Behind transparent images.
    pub backdrop: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    /// Behind selected text.
    pub selection: Hsla,
    /// The hairline between text and the minimap docked beside it.
    pub rule: Hsla,
    /// A keycap's face and its edge.
    pub key: Hsla,
    pub key_edge: Hsla,
    /// Small quiet words that must stay readable, 4.5:1 on the surface.
    pub caption: Hsla,
}

impl Theme {
    pub fn of(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self {
                surface: rgb(0x26211d).into(),
                backdrop: rgb(0x1c1916).into(),
                text: rgb(0xede5d9).into(),
                muted: rgb(0xa5998b).into(),
                selection: rgba(0x9a6a3f8c).into(),
                rule: rgba(0xa5998b40).into(),
                key: rgb(0x342d27).into(),
                key_edge: rgba(0xa5998b66).into(),
                caption: rgb(0xa5998b).into(),
            },
            _ => Self {
                surface: rgb(0xf6f1e9).into(),
                backdrop: rgb(0xebe4d8).into(),
                text: rgb(0x2b2520).into(),
                muted: rgb(0x85786b).into(),
                selection: rgba(0xe8b07a80).into(),
                rule: rgba(0x85786b40).into(),
                key: rgb(0xfffcf7).into(),
                key_edge: rgba(0x85786b73).into(),
                caption: rgb(0x71665b).into(),
            },
        }
    }
}

/// The warm amber of played time and of the scrubbers' thumbs.
pub fn progress() -> Hsla {
    rgb(0xe39a63).into()
}

/// The colour a kind of file is drawn in on its card.
pub fn accent(kind: Kind) -> Hsla {
    rgb(match kind {
        Kind::Image => 0xc9733a,
        Kind::Video => 0xb4553f,
        Kind::Audio => 0xa35d7e,
        Kind::Pdf => 0xb8443a,
        Kind::Text => 0x5f8063,
        Kind::Archive | Kind::Compressed => 0x9a7340,
        Kind::Binary => 0x7a6a5c,
        Kind::Document => 0x4f78a8,
        Kind::Spreadsheet => 0x4b8a58,
        Kind::Presentation => 0xc8693a,
        Kind::Folder => 0xd4a24c,
        Kind::Other => 0x8b7e70,
    })
    .into()
}

struct ComponentsReady;
impl Global for ComponentsReady {}

/// gpui-component, set up once and following the window's light or dark:
/// its tree and its text view take their colours from it.
pub fn components(window: &mut Window, cx: &mut App) {
    if !cx.has_global::<ComponentsReady>() {
        gpui_kit::component::init(cx);
        cx.set_global(ComponentsReady);
    }
    gpui_kit::component::Theme::change(window.appearance(), Some(window), cx);
}
