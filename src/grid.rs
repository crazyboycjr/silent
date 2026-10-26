//! Spreadsheets as a grid: column letters across the top, row numbers down
//! the side, only the cells in view drawn, and a tab for each sheet.

use std::cell::Cell as Slot;
use std::path::PathBuf;
use std::rc::Rc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use silent::format;
use silent::memory::View;
use silent::office::sheet::{self, Grid, MAX_COLUMNS, MAX_ROWS};
use silent::vim::Motion;

use crate::content::Sheet;
use crate::theme::{Theme, progress};

const ROW: f32 = 24.;
/// The sheet tabs' height.
const TABS: f32 = 34.;
const FONT: f32 = 13.;
const PAD: f32 = 7.;
/// Characters of a cell's text the strip shows.
const CELL_TEXT: usize = 80;

pub struct SheetPane {
    path: PathBuf,
    names: Vec<String>,
    current: usize,
    grid: Rc<Grid>,
    loading: bool,
    top: f64,
    scroll_x: Pixels,
    /// The cell cursor, by row and column of the grid, once a click or the
    /// arrows have put it somewhere.
    cursor: Option<(usize, usize)>,
    /// The cells' area as last drawn, and a digit's width.
    area: Rc<Slot<Bounds<Pixels>>>,
    advance: Rc<Slot<Pixels>>,
}

impl SheetPane {
    pub fn new(sheet: Sheet, view: Option<View>) -> Self {
        let (cursor, top, side) = match view {
            Some(View::Sheet {
                cell, top, side, ..
            }) => (cell.map(|(r, c)| (r as usize, c as usize)), top, px(side)),
            _ => (None, 0., px(0.)),
        };
        Self {
            path: sheet.path,
            names: sheet.names,
            current: sheet.current,
            grid: Rc::new(sheet.grid),
            loading: false,
            top,
            scroll_x: side,
            cursor,
            area: Rc::default(),
            advance: Rc::new(Slot::new(px(7.4))),
        }
    }

    fn has_tabs(&self) -> bool {
        self.names.len() > 1
    }

    pub fn view(&self) -> Option<View> {
        let moved = self.top > 0.01 || self.scroll_x > px(0.5) || self.cursor.is_some();
        (moved || self.current > 0).then(|| View::Sheet {
            sheet: self.current,
            cell: self.cursor.map(|(r, c)| (r as u32, c as u32)),
            top: self.top,
            side: f32::from(self.scroll_x),
        })
    }

    /// `B12: late delivery`, for the cell under the cursor.
    fn cursor_text(&self) -> Option<String> {
        let (r, c) = self.cursor?;
        let address = format!(
            "{}{}",
            sheet::column_name(self.grid.origin.1 + c as u32),
            self.grid.origin.0 as usize + r + 1
        );
        let text = self
            .grid
            .cells
            .get(r)?
            .get(c)
            .map_or("", |cell| cell.text.as_str());
        let text = text.lines().next().unwrap_or_default();
        Some(match text.char_indices().nth(CELL_TEXT) {
            Some((cut, _)) => format!("{address}: {}…", &text[..cut]),
            None if text.is_empty() => address,
            None => format!("{address}: {text}"),
        })
    }

    /// `Sales · 199 rows × 7 columns`, and what was left unread.
    pub fn summary(&self) -> String {
        let name = self.names.get(self.current).cloned().unwrap_or_default();
        let (rows, columns) = self.grid.size;
        let mut text = name;
        if let Some(cell) = self.cursor_text() {
            text.push_str(" · ");
            text.push_str(&cell);
        }
        text.push_str(&format!(
            " · {} × {}",
            format::count(u64::from(rows), "row", "rows"),
            format::count(u64::from(columns), "column", "columns")
        ));
        if rows as usize > MAX_ROWS || columns as usize > MAX_COLUMNS {
            text.push_str(&format!(
                " · the first {} rows",
                format::grouped(MAX_ROWS as u64)
            ));
        }
        if self.names.len() > 1 {
            text.push_str(&format!(
                " · sheet {} of {}",
                self.current + 1,
                self.names.len()
            ));
        }
        text
    }

    fn widths(&self) -> Vec<Pixels> {
        let digit = self.advance.get();
        self.grid
            .widths
            .iter()
            .map(|w| digit * (*w).clamp(4, 40) as f32 + px(PAD * 2.))
            .collect()
    }

    fn rows(&self) -> f64 {
        f64::from((self.area.get().size.height / px(ROW)).max(1.))
    }

    fn clamp(&mut self) {
        let max_top = (self.grid.cells.len() as f64 - self.rows() + 1.).max(0.);
        self.top = self.top.clamp(0., max_top);
        let total: Pixels = self.widths().into_iter().sum();
        let max_x = (total - self.area.get().size.width + px(PAD)).max(px(0.));
        self.scroll_x = self.scroll_x.clamp(px(0.), max_x);
    }

    /// Page Up and Down, Home and End and the like move the view.
    pub fn motion(&mut self, motion: Motion, cx: &mut Context<Self>) {
        let page = (self.rows() - 2.).max(1.);
        match motion {
            Motion::PageDown(k) => self.top += page * k as f64,
            Motion::PageUp(k) => self.top -= page * k as f64,
            Motion::Top => self.top = 0.,
            Motion::Bottom => self.top = f64::MAX,
            _ => return,
        }
        self.clamp();
        cx.notify();
    }

    /// Moves the cell cursor by `columns` and `rows`, and the view to keep
    /// it in sight. The first move puts it on the top left cell in view.
    pub fn move_cursor(&mut self, columns: i32, rows: i32, cx: &mut Context<Self>) {
        let (height, width) = (self.grid.cells.len(), self.grid.widths.len());
        if height == 0 || width == 0 {
            return;
        }
        let (r, c) = match self.cursor {
            Some((r, c)) => (
                r.saturating_add_signed(rows as isize),
                c.saturating_add_signed(columns as isize),
            ),
            None => (self.top.ceil() as usize, self.first_column()),
        };
        let (r, c) = (r.min(height - 1), c.min(width - 1));
        self.cursor = Some((r, c));
        self.reveal(r, c);
        cx.notify();
    }

    /// The first column whole in view.
    fn first_column(&self) -> usize {
        let mut x = px(0.);
        for (c, w) in self.widths().into_iter().enumerate() {
            if x >= self.scroll_x {
                return c;
            }
            x += w;
        }
        0
    }

    /// Scrolls so that cell `r`, `c` is in view.
    fn reveal(&mut self, r: usize, c: usize) {
        let rows = self.rows().floor().max(1.);
        let row = r as f64;
        if row < self.top {
            self.top = row;
        } else if row + 1. > self.top + rows {
            self.top = row + 1. - rows;
        }
        let widths = self.widths();
        let left: Pixels = widths[..c].iter().copied().sum();
        let right = left + widths[c];
        let shown = self.area.get().size.width;
        if left < self.scroll_x {
            self.scroll_x = left;
        } else if right > self.scroll_x + shown {
            self.scroll_x = right - shown + px(PAD);
        }
        self.clamp();
    }

    /// The cell under a window position, when it is on the cells.
    fn cell_at(&self, position: Point<Pixels>) -> Option<(usize, usize)> {
        let area = self.area.get();
        if !area.contains(&position) {
            return None;
        }
        let row = (self.top + f64::from((position.y - area.top()) / ROW)).floor() as usize;
        let mut x = area.left() - self.scroll_x;
        let column = self.widths().into_iter().position(|w| {
            x += w;
            position.x < x
        })?;
        (row < self.grid.cells.len()).then_some((row, column))
    }

    fn press(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        if let Some(cell) = self.cell_at(event.position) {
            self.cursor = Some(cell);
            cx.notify();
        }
    }

    /// Shows sheet `index`, read in the background.
    pub fn show_sheet(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.current || index >= self.names.len() || self.loading {
            return;
        }
        self.current = index;
        self.loading = true;
        let (path, name) = (self.path.clone(), self.names[index].clone());
        cx.spawn(async move |this, cx| {
            let grid = cx
                .background_executor()
                .spawn(async move { sheet::read(&path, &name) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.loading = false;
                this.grid = Rc::new(grid.unwrap_or_default());
                this.top = 0.;
                this.scroll_x = px(0.);
                this.cursor = None;
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn step_sheet(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.names.len();
        if count > 1 {
            let next = (self.current + if forward { 1 } else { count - 1 }) % count;
            self.show_sheet(next, cx);
        }
    }

    fn scroll(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(ROW));
        self.top -= f64::from(delta.y / ROW);
        self.scroll_x -= delta.x;
        self.clamp();
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for SheetPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(window.appearance());
        let grid = self.grid.clone();
        let widths = self.widths();
        let (top, scroll_x, cursor) = (self.top, self.scroll_x, self.cursor);
        let (area, advance) = (self.area.clone(), self.advance.clone());
        let last_row = grid.origin.0 as usize + grid.cells.len();
        let gutter = px(format::grouped(last_row as u64).len().max(2) as f32 * 8. + PAD * 2.);
        let mut style = window.text_style();
        style.font_family = "Noto Sans".into();
        style.font_size = px(FONT).into();
        style.color = theme.text;
        let quiet = TextStyle {
            color: theme.muted,
            ..style.clone()
        };
        let rule = theme.muted.opacity(0.22);
        let header = theme.muted.opacity(0.1);

        let paint = move |bounds: Bounds<Pixels>, _: (), window: &mut Window, cx: &mut App| {
            let system = window.text_system().clone();
            let digit = system.shape_line("0".into(), px(FONT), &[style.to_run(1)], None);
            advance.set(digit.width());
            let cells = Bounds::from_corners(
                point(bounds.left() + gutter, bounds.top() + px(ROW)),
                bounds.bottom_right(),
            );
            area.set(cells);
            let row_height = px(ROW);
            let first = top.floor() as usize;
            let rows = (f32::from(cells.size.height) / ROW).ceil() as usize + 1;
            let y_of = |r: usize| cells.top() + row_height * ((r as f64 - top) as f32);
            let shape = |text: &str, style: &TextStyle| {
                let line = text.lines().next().unwrap_or_default().to_owned();
                let len = line.len();
                system.shape_line(line.into(), px(FONT), &[style.to_run(len)], None)
            };
            // Columns in view, with their left edges.
            let mut columns = Vec::new();
            let mut x = cells.left() - scroll_x;
            for (c, w) in widths.iter().enumerate() {
                if x + *w > cells.left() && x < cells.right() {
                    columns.push((c, x, *w));
                }
                x += *w;
            }
            window.paint_quad(fill(
                Bounds::from_corners(bounds.origin, point(bounds.right(), cells.top())),
                header,
            ));
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(bounds.left(), cells.top()),
                    point(cells.left(), bounds.bottom()),
                ),
                header,
            ));
            window.with_content_mask(Some(ContentMask { bounds: cells }), |window| {
                for r in first..(first + rows).min(grid.cells.len()) {
                    let y = y_of(r);
                    window.paint_quad(fill(
                        Bounds::new(
                            point(cells.left(), y + row_height - px(1.)),
                            size(cells.size.width, px(1.)),
                        ),
                        rule,
                    ));
                    for &(c, x, w) in &columns {
                        let Some(cell) = grid.cells[r].get(c) else {
                            continue;
                        };
                        if cell.text.is_empty() {
                            continue;
                        }
                        let shaped = shape(&cell.text, &style);
                        let cell_bounds = Bounds::new(point(x, y), size(w, row_height));
                        let tx = if cell.number {
                            x + w - px(PAD) - shaped.width()
                        } else {
                            x + px(PAD)
                        };
                        window.with_content_mask(
                            Some(ContentMask {
                                bounds: cell_bounds,
                            }),
                            |window| {
                                let _ = shaped.paint(
                                    point(tx, y + (row_height - px(FONT * 1.35)) / 2.),
                                    px(FONT * 1.35),
                                    TextAlign::Left,
                                    None,
                                    window,
                                    cx,
                                );
                            },
                        );
                    }
                }
                for &(_, x, w) in &columns {
                    window.paint_quad(fill(
                        Bounds::new(
                            point(x + w - px(1.), cells.top()),
                            size(px(1.), cells.size.height),
                        ),
                        rule,
                    ));
                }
                if let Some((r, c)) = cursor
                    && let Some(&(_, x, w)) = columns.iter().find(|(column, ..)| *column == c)
                {
                    let cell = Bounds::new(
                        point(x - px(1.), y_of(r) - px(1.)),
                        size(w + px(1.), row_height + px(1.)),
                    );
                    window.paint_quad(quad(
                        cell,
                        px(0.),
                        progress().opacity(0.1),
                        px(2.),
                        progress(),
                        BorderStyle::Solid,
                    ));
                }
            });
            // Column letters and row numbers stay put while the cells scroll.
            let letters = Bounds::from_corners(
                point(cells.left(), bounds.top()),
                point(bounds.right(), cells.top()),
            );
            window.with_content_mask(Some(ContentMask { bounds: letters }), |window| {
                for &(c, x, w) in &columns {
                    if cursor.is_some_and(|(_, column)| column == c) {
                        let under = Bounds::new(point(x, bounds.top()), size(w, row_height));
                        window.paint_quad(fill(under, progress().opacity(0.2)));
                    }
                    let name = sheet::column_name(grid.origin.1 + c as u32);
                    let shaped = shape(&name, &quiet);
                    let origin = point(
                        x + (w - shaped.width()) / 2.,
                        bounds.top() + (row_height - px(FONT * 1.35)) / 2.,
                    );
                    let _ =
                        shaped.paint(origin, px(FONT * 1.35), TextAlign::Left, None, window, cx);
                }
            });
            let numbers = Bounds::from_corners(
                point(bounds.left(), cells.top()),
                point(cells.left(), bounds.bottom()),
            );
            window.with_content_mask(Some(ContentMask { bounds: numbers }), |window| {
                for r in first..(first + rows).min(grid.cells.len()) {
                    if cursor.is_some_and(|(row, _)| row == r) {
                        let under = Bounds::new(
                            point(bounds.left(), y_of(r)),
                            size(cells.left() - bounds.left(), row_height),
                        );
                        window.paint_quad(fill(under, progress().opacity(0.2)));
                    }
                    let label = (grid.origin.0 as usize + r + 1).to_string();
                    let shaped = shape(&label, &quiet);
                    let origin = point(
                        cells.left() - px(PAD) - shaped.width(),
                        y_of(r) + (row_height - px(FONT * 1.35)) / 2.,
                    );
                    let _ =
                        shaped.paint(origin, px(FONT * 1.35), TextAlign::Left, None, window, cx);
                }
            });
        };

        let tabs = self.has_tabs().then(|| {
            div()
                .flex_none()
                .flex()
                .flex_row()
                .gap(px(2.))
                .px(px(8.))
                .h(px(TABS))
                .items_center()
                .bg(header)
                .border_t_1()
                .border_color(rule)
                .children(self.names.iter().enumerate().map(|(i, name)| {
                    let active = i == self.current;
                    div()
                        .id(i)
                        .px(px(12.))
                        .py(px(4.))
                        .rounded(px(5.))
                        .text_size(px(12.5))
                        .text_color(if active { theme.text } else { theme.muted })
                        .when(active, |tab| {
                            tab.bg(theme.surface).font_weight(FontWeight::SEMIBOLD)
                        })
                        .cursor(CursorStyle::PointingHand)
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.show_sheet(i, cx);
                        }))
                        .child(name.clone())
                }))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.surface)
            .font_family("Noto Sans")
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .on_scroll_wheel(
                        cx.listener(|this, event: &ScrollWheelEvent, _, cx| this.scroll(event, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| this.press(event, cx)),
                    )
                    .child(canvas(|_, _, _| (), paint).size_full()),
            )
            .children(tabs)
    }
}
