//! An archive's entries as a tree, in gpui-component's tree view. Coming
//! back to an archive within the hour opens the same folders, selects the
//! same entry and scrolls to the same place.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::rc::Rc;

use gpui_kit::component::list::ListItem;
use gpui_kit::component::tree::{TreeItem, TreeState, tree};
use gpui_kit::*;
use silent::format;
use silent::memory::View;

use crate::content::Archive;
use crate::icons;
use crate::theme::Theme;

/// gpui-component is set up only when an archive is shown, so that other
/// previews do not pay for it.
struct ComponentsReady;
impl Global for ComponentsReady {}

pub struct ArchivePane {
    tree: Entity<TreeState>,
    /// The top level; clones share their open state with the tree's.
    roots: Vec<TreeItem>,
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
    pub fn new(
        archive: Archive,
        view: Option<View>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        let (top, selected, open) = match view {
            Some(View::Tree {
                top,
                selected,
                open,
            }) => (
                top,
                selected,
                Some(open.into_iter().collect::<HashSet<_>>()),
            ),
            _ => (0, None, None),
        };
        let mut sizes = HashMap::new();
        let roots = items(&root, "", 0, &mut sizes, open.as_ref());
        let tree = cx.new(|cx| {
            let mut state = TreeState::new(cx).items(roots.clone());
            if let Some(ix) = selected.and_then(|id| state.index_of(&id.into())) {
                state.set_selected_index(Some(ix), cx);
            }
            state
                .scroll_handle()
                .0
                .borrow()
                .base_handle
                .set_offset(point(px(0.), px(-(top as f32))));
            state
        });
        Self {
            tree,
            roots,
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

    /// The open folders, the selection and the scroll; nothing when they
    /// are as a first look shows them.
    pub fn view(&self, cx: &App) -> Option<View> {
        let state = self.tree.read(cx);
        let selected = state.selected_item().map(|item| item.id.to_string());
        let top = -f32::from(state.scroll_handle().0.borrow().base_handle.offset().y);
        let mut open = Vec::new();
        let mut first_look = true;
        let mut pending: Vec<(&TreeItem, usize)> = self.roots.iter().map(|i| (i, 0)).collect();
        while let Some((item, depth)) = pending.pop() {
            if !item.is_folder() {
                continue;
            }
            first_look &= item.is_expanded() == (depth == 0);
            if item.is_expanded() {
                open.push(item.id.to_string());
                pending.extend(item.children.iter().map(|c| (c, depth + 1)));
            }
        }
        let top = top.max(0.).round() as usize;
        (!first_look || selected.is_some() || top > 0).then_some(View::Tree {
            top,
            selected,
            open,
        })
    }

    /// The tree takes the keys after a click anywhere in the archive.
    fn engage(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.tree.update(cx, |tree, cx| tree.focus(window, cx));
        window.prevent_default();
    }
}

/// Folders first, then files, each by name. The folders in `open` start
/// open; without it, those of the top level do.
fn items(
    node: &Node,
    prefix: &str,
    depth: usize,
    sizes: &mut HashMap<SharedString, u64>,
    open: Option<&HashSet<String>>,
) -> Vec<TreeItem> {
    let mut children: Vec<(&String, &Node)> = node.children.iter().collect();
    children.sort_by_key(|(name, child)| (!child.dir, name.to_lowercase()));
    children
        .into_iter()
        .map(|(name, child)| {
            let id = format!("{prefix}{name}");
            let mut item = TreeItem::new(id.clone(), name.clone());
            if child.dir {
                let path = format!("{id}/");
                let expanded = open.map_or(depth == 0, |open| open.contains(&id));
                item = item
                    .children(items(child, &path, depth + 1, sizes, open))
                    .expanded(expanded);
            } else {
                sizes.insert(id.into(), child.size);
            }
            item
        })
        .collect()
}

impl Render for ArchivePane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| this.engage(window, cx)),
            )
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
