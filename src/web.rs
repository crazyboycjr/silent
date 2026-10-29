//! HTML pages laid out by Blitz, as they are, with their own CSS: silent's
//! paper, ink and fonts fill in only where a page says nothing. Nothing is fetched:
//! files beside the page are read, and a picture from anywhere else becomes
//! its words. vello_cpu paints the page into pictures for gpui, on the
//! thread that holds the document.

use std::cell::RefCell;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use anyrender::ImageRenderer as _;
use anyrender_vello_cpu::VelloCpuImageRenderer;
use blitz_dom::{BaseDocument, DocumentConfig, FontContext, NodeId, Point, UserAgentStylesheet};
use blitz_html::HtmlDocument;
use blitz_traits::net::{Bytes, NetHandler, NetProvider, Request, Url};
use blitz_traits::shell::{ColorScheme, Viewport};
use gpui_kit::RenderImage;
use silent::geometry::Screen;
use silent::markup;
use silent::memory::View;
use smallvec::SmallVec;

const PAGE_CSS: &str = include_str!("page.css");

/// The column pages are read in, in logical pixels, as the text view's.
pub const COLUMN: f64 = 860.0;
/// The heights a page's window takes, in logical pixels at 100%: from 12
/// to 44 of the text view's rows.
const SHORTEST: f64 = 12.0 * 22.0 + 80.0;
const TALLEST: f64 = 44.0 * 22.0 + 80.0;
/// The device rows a strip of a page holds.
pub const STRIP: u32 = 512;

/// Whether the desktop is dark, as a window last saw it, for pages laid
/// out before their window is drawn.
static DARK: AtomicBool = AtomicBool::new(false);

pub fn set_dark(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

fn scheme(dark: bool) -> ColorScheme {
    if dark {
        ColorScheme::Dark
    } else {
        ColorScheme::Light
    }
}

/// silent's paper under a page that paints no ground of its own.
pub fn paper(dark: bool) -> [u8; 3] {
    if dark {
        [0x26, 0x21, 0x1d]
    } else {
        [0xf6, 0xf1, 0xe9]
    }
}

/// A Blitz document. It is made on a loading thread and moves, whole and
/// once, to the window's thread, which alone uses it from then on.
pub struct Doc(HtmlDocument);

// SAFETY: a Blitz document is not Send for two trait objects only (the
// compiler names no others): its HTML parser provider, Blitz's
// `DummyHtmlParserProvider`, which holds nothing, and Stylo's font metrics
// provider, Blitz's, which holds the document's font context in an
// `Arc<Mutex<_>>`. A `Doc` is moved, never shared, and Blitz's thread-local
// layout context belongs to the thread, not to the document.
unsafe impl Send for Doc {}

impl Deref for Doc {
    type Target = BaseDocument;
    fn deref(&self) -> &BaseDocument {
        &self.0
    }
}

impl DerefMut for Doc {
    fn deref_mut(&mut self) -> &mut BaseDocument {
        &mut self.0
    }
}

/// A page laid out, ready for its window.
pub struct Page {
    pub doc: Doc,
    /// Where the file is, for its relative links.
    pub dir: PathBuf,
    /// The window's size in root-window pixels, which the layout fits.
    pub size: (i32, i32),
    /// The text size it was laid out at, 1 at 100%.
    pub zoom: f32,
    pub dark: bool,
    /// It has boxes fixed to the window, so it is painted a window at a
    /// time rather than in strips that scroll.
    pub fixed: bool,
    /// Where it opens, in CSS pixels from its top.
    pub top: f64,
    /// Strips painted ahead for the first frame, by their index.
    pub strips: Vec<(u32, Arc<RenderImage>)>,
    /// The window painted ahead, for a page with fixed boxes.
    pub window: Option<Arc<RenderImage>>,
}

/// The system's fonts and Blitz's bullet glyphs, found once: as a resident
/// previewer starts, or else by the first page. Clones share what fontique
/// learns of the fonts.
static FONTS: OnceLock<Mutex<FontContext>> = OnceLock::new();

fn fonts() -> FontContext {
    let fonts = FONTS.get_or_init(|| {
        use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
        let mut fonts = FontContext {
            source_cache: SourceCache::new_shared(),
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: true,
            }),
        };
        fonts
            .collection
            .register_fonts(Blob::new(Arc::new(blitz_dom::BULLET_FONT) as _), None);
        Mutex::new(fonts)
    });
    fonts.lock().unwrap().clone()
}

/// Blitz's user-agent sheet and silent's, parsed once.
fn sheets() -> Vec<UserAgentStylesheet> {
    static PAGE: OnceLock<UserAgentStylesheet> = OnceLock::new();
    let ours = PAGE.get_or_init(|| UserAgentStylesheet::parse(PAGE_CSS));
    vec![UserAgentStylesheet::default_sheet(), ours.clone()]
}

/// Readies, in the background and once, what the first page would wait
/// for: the system's fonts for text and for SVG, the user-agent sheets, and
/// a page laid out and painted through every step.
pub fn prepare() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("pages".into()).spawn(|| {
            let _ = fonts();
            sheets();
            blitz_dom::util::prepare_svg_fonts();
            let primer = "<!DOCTYPE html><h1 id=page>Page</h1><p>Some <em>text</em> with \
                          <code>code</code>, a <a href=#page>link</a>.</p><ul><li>one</ul>";
            let viewport = Viewport::new(800, 600, 1.0, ColorScheme::Light);
            let mut doc = document(primer, None, viewport);
            doc.resolve(0.0);
            let _ = paint(&mut doc, 0.0, 800, 600, paper(false));
        });
    });
}

/// Reads `file:` and `data:` URLs on the spot and fetches nothing else.
/// Every request is answered, a refused or missing one with no bytes, so
/// that no stylesheet holds the page back.
struct LocalOnly;

impl NetProvider for LocalOnly {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let url = request.url;
        let bytes = match url.scheme() {
            "file" => url
                .to_file_path()
                .ok()
                .and_then(|path| std::fs::read(path).ok()),
            "data" => data_url(url.as_str()),
            _ => None,
        };
        handler.bytes(url.to_string(), Bytes::from(bytes.unwrap_or_default()));
    }
}

fn data_url(url: &str) -> Option<Vec<u8>> {
    use base64::Engine as _;
    let (meta, payload) = url.strip_prefix("data:")?.split_once(',')?;
    let payload: Vec<u8> = percent_encoding::percent_decode_str(payload).collect();
    if meta.ends_with(";base64") {
        let payload: Vec<u8> = payload
            .into_iter()
            .filter(|b| !b.is_ascii_whitespace())
            .collect();
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()
    } else {
        Some(payload)
    }
}

fn document(html: &str, base: Option<&Url>, viewport: Viewport) -> Doc {
    let config = DocumentConfig {
        viewport: Some(viewport),
        base_url: base.map(Url::to_string),
        ua_stylesheets: sheets(),
        net_provider: Some(Arc::new(LocalOnly)),
        font_ctx: Some(fonts()),
        ..Default::default()
    };
    let mut doc = Doc(HtmlDocument::from_html(html, config));
    words_for_pictures(&mut doc);
    doc
}

/// Every picture from away from this machine becomes its alternative text,
/// as in the text view; Blitz itself would draw nothing where it failed.
fn words_for_pictures(doc: &mut BaseDocument) {
    let mut found = Vec::new();
    for (id, node) in doc.tree().iter() {
        let Some(element) = node.element_data() else {
            continue;
        };
        if &*element.name.local != "img" {
            continue;
        }
        let attr = |name: &str| {
            element
                .attrs()
                .iter()
                .find(|a| &*a.name.local == name)
                .map(|a| a.value.clone())
        };
        if markup::remote(&attr("src").unwrap_or_default()) {
            found.push((id, attr("alt").unwrap_or_default()));
        }
    }
    if found.is_empty() {
        return;
    }
    let mut mutator = doc.mutate();
    for (id, alt) in &found {
        if alt.trim().is_empty() {
            mutator.remove_node(*id);
        } else {
            let text = mutator.create_text_node(alt);
            mutator.replace_node_with(*id, &[text]);
        }
    }
}

/// The window of a page `height_css` CSS pixels tall at `zoom`: the column,
/// as tall as the page up to the text view's tallest, in root-window pixels.
pub fn window_size(height_css: f64, zoom: f32, screen: Screen) -> (i32, i32) {
    let zoom = f64::from(zoom);
    let height = height_css.clamp(SHORTEST, TALLEST);
    screen.card(COLUMN * zoom, height * zoom, 0.0)
}

/// A viewport `size` root-window pixels, at the screen's scale and `zoom`.
pub fn viewport(size: (i32, i32), scale: f64, zoom: f32, dark: bool) -> Viewport {
    let mut viewport = Viewport::new(
        size.0.max(1) as u32,
        size.1.max(1) as u32,
        scale as f32,
        scheme(dark),
    );
    viewport.set_zoom(zoom);
    viewport
}

/// The page's height in CSS pixels.
pub fn height(doc: &BaseDocument) -> f64 {
    doc.try_root_element().map_or(0.0, |root| {
        let layout = root.final_layout();
        f64::from(
            layout
                .size
                .height
                .max(layout.scrollable_overflow_rect.bottom),
        )
    })
}

/// Whether a box is fixed to the window, which strips scrolling with the
/// page would repeat. Blitz lays out sticky boxes as relative ones.
fn has_fixed(doc: &BaseDocument) -> bool {
    doc.tree()
        .iter()
        .any(|(_, node)| node.is_out_of_flow() && node.taffy_position() == taffy::Position::Fixed)
}

/// What opening a page did, when it did not open.
pub enum Failed {
    /// Its layout took longer than it was given; it goes on alone.
    Slow,
    Unreadable,
}

/// HTML `text` laid out as a page, in its window for `screen` at `zoom`, scrolled
/// to where `view` left it, with what its first frame shows painted. Past
/// `within`, it is left to finish on its own thread, unused.
pub fn open(
    text: String,
    path: &Path,
    screen: Screen,
    zoom: f32,
    view: Option<&View>,
    within: Option<Duration>,
) -> Result<Page, Failed> {
    let started = Instant::now();
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let base = Url::from_file_path(&path).map_err(|_| Failed::Unreadable)?;
    let dir = path.parent().map(Path::to_owned).unwrap_or_default();
    let dark = DARK.load(Ordering::Relaxed);
    let tallest = window_size(f64::INFINITY, zoom, screen);
    let scale = screen.scale;
    let (send, receive) = mpsc::sync_channel(1);
    let laying = std::thread::Builder::new()
        .name("page".into())
        .spawn(move || {
            let mut doc = document(&text, Some(&base), viewport(tallest, scale, zoom, dark));
            doc.resolve(0.0);
            crate::trace::mark("page laid out");
            // A short page gets a short window, laid out again at its size.
            let size = window_size(height(&doc), zoom, screen);
            if size != tallest {
                doc.set_viewport(viewport(size, scale, zoom, dark));
                doc.resolve(0.0);
            }
            let _ = send.send((doc, size));
        })
        .map_err(|_| Failed::Unreadable)?;
    let (mut doc, size) = match within {
        Some(within) => receive
            .recv_timeout(within.saturating_sub(started.elapsed()))
            .map_err(|_| Failed::Slow)?,
        None => receive.recv().map_err(|_| Failed::Unreadable)?,
    };
    let _ = laying.join();
    let fixed = has_fixed(&doc);
    let view_height = f64::from(size.1) / (scale * f64::from(zoom));
    let most = (height(&doc) - view_height).max(0.0);
    let top = match view {
        Some(View::Page { within }) => (f64::from(*within) * height(&doc)).clamp(0.0, most),
        _ => 0.0,
    };
    let device = |css: f64| (css * scale * f64::from(zoom)).round() as u32;
    let (width, height_px) = (size.0.max(1) as u32, size.1.max(1) as u32);
    let mut strips = Vec::new();
    let mut window = None;
    if fixed {
        window = Some(paint(&mut doc, top, width, height_px, paper(dark)));
    } else {
        let end = device(height(&doc)).div_ceil(STRIP).max(1);
        let first = (device(top) / STRIP).min(end - 1);
        let last = (device(top) + height_px).div_ceil(STRIP).min(end);
        for strip in first..last {
            let at = f64::from(strip * STRIP) / (scale * f64::from(zoom));
            strips.push((strip, paint(&mut doc, at, width, STRIP, paper(dark))));
        }
    }
    doc.set_viewport_scroll(Point { x: 0.0, y: top });
    crate::trace::mark("page painted");
    Ok(Page {
        doc,
        dir,
        size,
        zoom,
        dark,
        fixed,
        top,
        strips,
        window,
    })
}

thread_local! {
    /// The painter, kept for the next picture of the same size: it holds a
    /// pool of threads and the pictures it has converted.
    static PAINTER: RefCell<Option<((u32, u32), VelloCpuImageRenderer)>> =
        const { RefCell::new(None) };
}

/// The page's device rows from `top` CSS pixels down, `width` × `height`,
/// painted on `paper` into a picture for gpui. The document's scroll is as
/// it was after.
pub fn paint(
    doc: &mut BaseDocument,
    top: f64,
    width: u32,
    height: u32,
    paper: [u8; 3],
) -> Arc<RenderImage> {
    let scale = doc.viewport().scale_f64();
    let kept = doc.viewport_scroll();
    doc.set_viewport_scroll(Point { x: 0.0, y: top });
    let mut pixels = Vec::new();
    PAINTER.with(|painter| {
        let mut painter = painter.borrow_mut();
        if painter
            .as_ref()
            .is_none_or(|(size, _)| *size != (width, height))
        {
            *painter = Some(((width, height), VelloCpuImageRenderer::new(width, height)));
        }
        let (_, renderer) = painter.as_mut().unwrap();
        renderer.reset();
        renderer.render_to_vec(
            |scene| blitz_paint::paint_scene(scene, doc, scale, width, height, 0, 0),
            &mut pixels,
        );
    });
    doc.set_viewport_scroll(kept);
    // Premultiplied RGBA over the paper, as the BGRA gpui takes.
    for px in pixels.chunks_exact_mut(4) {
        let under = 255 - u32::from(px[3]);
        let over = |c: u8, p: u8| (u32::from(c) + u32::from(p) * under / 255).min(255) as u8;
        let (r, g, b) = (
            over(px[0], paper[0]),
            over(px[1], paper[1]),
            over(px[2], paper[2]),
        );
        px.copy_from_slice(&[b, g, r, 255]);
    }
    let frame = image::RgbaImage::from_raw(width, height, pixels).expect("a whole picture");
    Arc::new(RenderImage::new(SmallVec::from_elem(
        image::Frame::new(frame),
        1,
    )))
}

/// The selected text, its blocks a line each.
pub fn selected_text(doc: &BaseDocument) -> Option<String> {
    let mut out = String::new();
    for (node_id, start, end) in doc.get_text_selection_ranges() {
        let Some(layout) = text_of(doc, node_id) else {
            continue;
        };
        let Some(part) = layout.get(start..end) else {
            continue;
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(part);
    }
    (!out.trim().is_empty()).then_some(out)
}

fn text_of(doc: &BaseDocument, node_id: NodeId) -> Option<&str> {
    let layout = doc
        .get_node(node_id)?
        .element_data()?
        .inline_layout_data
        .as_ref()?;
    Some(&layout.text)
}

/// Selects every text of the page, from its first block to its last.
pub fn select_all(doc: &mut BaseDocument) {
    let mut roots = Vec::new();
    collect_inline_roots(doc, doc.root_node().id, &mut roots);
    let (Some(&first), Some(&last)) = (roots.first(), roots.last()) else {
        return;
    };
    let end = text_of(doc, last).map_or(0, str::len);
    doc.set_text_selection(first, 0, last, end);
}

/// The blocks with text in the order they are laid out.
fn collect_inline_roots(doc: &BaseDocument, node_id: NodeId, out: &mut Vec<NodeId>) {
    let Some(node) = doc.get_node(node_id) else {
        return;
    };
    if text_of(doc, node_id).is_some_and(|text| !text.trim().is_empty()) {
        out.push(node_id);
    }
    let children = node.layout_children.borrow();
    let children = children.as_deref().unwrap_or(&node.children);
    for &child in children {
        collect_inline_roots(doc, child, out);
    }
}

/// What a click on a page point does.
pub enum Click {
    /// Follow this link as written.
    Link(String),
    /// Open or close the `<details>` of this summary.
    Details(NodeId),
}

/// The link or summary at page point `(x, y)`, in CSS pixels.
pub fn click_at(doc: &BaseDocument, x: f32, y: f32) -> Option<Click> {
    let hit = doc.hit(x, y)?;
    let mut next = Some(hit.node_id);
    while let Some(id) = next {
        let node = doc.get_node(id)?;
        if let Some(element) = node.element_data() {
            match &*element.name.local {
                "a" => {
                    if let Some(href) = element.attrs().iter().find(|a| &*a.name.local == "href") {
                        return Some(Click::Link(href.value.clone()));
                    }
                }
                "summary" => {
                    let details = node.parent?;
                    let parent = doc.get_node(details)?;
                    let first_summary = parent.children.iter().copied().find(|&child| {
                        doc.get_node(child)
                            .and_then(|c| c.element_data())
                            .is_some_and(|e| &*e.name.local == "summary")
                    });
                    let is_details = parent
                        .element_data()
                        .is_some_and(|e| &*e.name.local == "details");
                    if is_details && first_summary == Some(id) {
                        return Some(Click::Details(details));
                    }
                }
                _ => {}
            }
        }
        next = node.parent;
    }
    None
}

/// Where in the page a link into it goes, in CSS pixels from the top:
/// `#name`, or the page's own URL with `#name`.
pub fn fragment_top(doc: &BaseDocument, href: &str) -> Option<f64> {
    let fragment = match href.strip_prefix('#') {
        Some(fragment) => fragment.to_owned(),
        None => {
            let url = doc.url().join(href).ok()?;
            let mut page = url.clone();
            page.set_fragment(None);
            let mut own = doc.url().clone();
            own.set_fragment(None);
            if page != own {
                return None;
            }
            url.fragment()?.to_owned()
        }
    };
    if fragment.is_empty() || fragment == "top" {
        return Some(0.0);
    }
    let decoded = percent_encoding::percent_decode_str(&fragment).decode_utf8_lossy();
    // An empty `<a name>` has no box of its own: its parent's place is it.
    let mut next = Some(doc.get_fragment_target(&decoded)?);
    while let Some(node) = next {
        if let Some(rect) = doc.get_client_bounding_rect(node) {
            return Some(rect.y + doc.viewport_scroll().y);
        }
        next = doc.get_node(node)?.parent;
    }
    None
}

/// The block under page point `(x, y)` and how far down it the point is,
/// from 0 to 1, to hold it in place while the page is laid out again.
pub fn place_at(doc: &BaseDocument, x: f32, y: f32) -> Option<(NodeId, f64)> {
    let hit = doc.hit(x, y)?;
    let mut next = Some(hit.node_id);
    while let Some(id) = next {
        let node = doc.get_node(id)?;
        if node.element_data().is_some()
            && let Some(rect) = doc.get_client_bounding_rect(id)
            && rect.height > 0.0
        {
            let top = rect.y + doc.viewport_scroll().y;
            let within = ((f64::from(y) - top) / rect.height).clamp(0.0, 1.0);
            return Some((id, within));
        }
        next = node.parent;
    }
    None
}

/// The top of `node` and its height, in CSS pixels from the page's top.
pub fn span_of(doc: &BaseDocument, node: NodeId) -> Option<(f64, f64)> {
    let rect = doc.get_client_bounding_rect(node)?;
    Some((rect.y + doc.viewport_scroll().y, rect.height))
}

/// The word or the block at page point `(x, y)`: `block` for the whole of
/// it, as a triple click asks.
pub fn select_at(doc: &mut BaseDocument, x: f32, y: f32, block: bool) -> bool {
    let Some((root, at)) = doc.find_text_position(x, y) else {
        return false;
    };
    let Some(text) = text_of(doc, root) else {
        return false;
    };
    let (start, end) = if block {
        (0, text.len())
    } else {
        word(text, at)
    };
    doc.set_text_selection(root, start, root, end);
    true
}

/// The word around byte `at` of `text`: letters, digits and underscores,
/// or else the one character there.
fn word(text: &str, at: usize) -> (usize, usize) {
    let at = (0..=at.min(text.len()))
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0);
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let start = text[..at]
        .char_indices()
        .rev()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(at, |(i, _)| i);
    let end = text[at..]
        .char_indices()
        .take_while(|&(_, c)| is_word(c))
        .last()
        .map_or(at, |(i, c)| at + i + c.len_utf8());
    if start == end {
        let next = text[at..].chars().next().map_or(0, char::len_utf8);
        return (at, at + next);
    }
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_letters_digits_and_underscores() {
        let text = "let max_len = 42;";
        assert_eq!(word(text, 6), (4, 11));
        assert_eq!(word(text, 4), (4, 11));
        assert_eq!(word(text, 15), (14, 16));
        // Between words, the one character there.
        assert_eq!(word(text, 12), (12, 13));
        assert_eq!(word("défini", 3), (0, 7));
    }

    #[test]
    fn data_urls_decode() {
        assert_eq!(data_url("data:text/plain,a%20b").unwrap(), b"a b");
        assert_eq!(data_url("data:text/plain;base64,aGk=").unwrap(), b"hi");
    }
}
