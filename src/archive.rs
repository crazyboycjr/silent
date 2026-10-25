//! An archive's entries as a tree, in gpui-component's tree view.

use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::*;
use silent::format;

use crate::content::Archive;
use crate::icons;
use crate::theme::Theme;

/// gpui-component is set up only when an archive is shown, so that other
/// previews do not pay for it.
struct ComponentsReady;
impl Global for ComponentsReady {}

pub struct ArchivePane {
    tree: Entity<TreeState>,
    sizes: Rc<HashMap<SharedString, u64>>,
    files: u64,
    bytes: u64,
}

#[derive(Default)]
struct Node {
    size: u64,
    dir: bool,
    children: BTreeMap<String, Node>,
}

impl ArchivePane {
    pub fn new(archive: Archive, window: &mut Window, cx: &mut Context<Self>) -> Self {
        if !cx.has_global::<ComponentsReady>() {
            gpui_kit::component::init(cx);
            cx.set_global(ComponentsReady);
        }
        // The rows' hover and selection follow the desktop's light or dark.
        gpui_kit::component::Theme::change(window.appearance(), Some(window), cx);
        let mut root = Node::default();
        for entry in &archive.entries {
            let mut node = &mut root;
            let parts: Vec<&str> = entry.path.split('/').filter(|p| !p.is_empty()).collect();
            for (i, part) in parts.iter().enumerate() {
                node = node.children.entry((*part).to_owned()).or_default();
                if i + 1 < parts.len() {
                    node.dir = true;
                }
            }
            node.dir |= entry.dir;
            if !entry.dir {
                node.size = entry.size;
            }
        }
        let mut sizes = HashMap::new();
        let items = items(&root, "", 0, &mut sizes);
        let tree = cx.new(|cx| TreeState::new(cx).items(items));
        Self {
            tree,
            sizes: Rc::new(sizes),
            files: archive.files,
            bytes: archive.bytes,
        }
    }

    /// `1,203 files · 12.4 MB unpacked`.
    pub fn summary(&self) -> String {
        format!(
            "{} · {} unpacked",
            format::count(self.files, "file", "files"),
            format::size(self.bytes)
        )
    }
}

/// Folders first, then files, each by name; the top level starts open.
fn items(
    node: &Node,
    prefix: &str,
    depth: usize,
    sizes: &mut HashMap<SharedString, u64>,
) -> Vec<TreeItem> {
    let mut children: Vec<(&String, &Node)> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| (!child.dir, name.to_lowercase()));
    children
        .into_iter()
        .map(|(name, child)| {
            let id: SharedString = format!("{prefix}{name}").into();
            let mut item = TreeItem::new(id.clone(), name.clone());
            if child.dir {
                let path = format!("{prefix}{name}/");
                item = item
                    .children(items(child, &path, depth + 1, sizes))
                    .expanded(depth == 0);
            } else {
                sizes.insert(id, child.size);
            }
            item
        })
        .collect()
}

impl Render for ArchivePane {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let sizes = self.sizes.clone();
        let rows = tree(&self.tree, move |ix, entry, _, _, _| {
            let item = entry.item();
            let (folder, open) = (entry.is_folder(), entry.is_expanded());
            let marker = match (folder, open) {
                (true, true) => "▾",
                (true, false) => "▸",
                _ => "",
            };
            let (icon, color) = icons::for_entry(&item.label, folder, open);
            let size = sizes
                .get(&item.id)
                .map(|s| format::size(*s))
                .unwrap_or_default();
            ListItem::new(ix).child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .w_full()
                    .pl(px(entry.depth() as f32 * 18.))
                    .child(
                        div()
                            .w(px(16.))
                            .flex_none()
                            .text_color(theme.muted)
                            .child(marker),
                    )
                    .child(
                        svg()
                            .path(icon)
                            .flex_none()
                            .size(px(16.))
                            .mr(px(8.))
                            .text_color(color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(theme.text)
                            .child(item.label.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .pl(px(12.))
                            .text_color(theme.muted)
                            .child(size),
                    ),
            )
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.surface)
            .text_color(theme.text)
            .text_size(px(13.5))
            .child(
                div()
                    .flex_none()
                    .px(px(22.))
                    .pt(px(16.))
                    .pb(px(8.))
                    .text_color(theme.muted)
                    .child(self.summary()),
            )
            .child(div().flex_1().min_h_0().px(px(10.)).pb(px(10.)).child(rows))
    }
}
