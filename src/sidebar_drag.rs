use crate::app::Workspace;
use crate::store::Entry;
use crate::theme::{self, Theme};
use crate::ui::{icon, ui_text};
use gpui::*;
use std::{cell::Cell, collections::{HashMap, HashSet, VecDeque}, rc::Rc, time::Instant};

const PITCH: f32 = 31.;
const INDENT: f32 = 18.;

#[derive(Clone)]
struct Row { entry: Entry, depth: usize }

#[derive(Clone, Copy)]
struct Spring { p: [f32; 3], v: [f32; 3], target: [f32; 3] }
impl Spring {
    fn new(p: [f32; 3]) -> Self { Self { p, v: [0.; 3], target: p } }
    fn step(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for i in 0..3 {
            // Substeps keep the same damped spring stable at 60 and 120 Hz.
            for _ in 0..4 {
                self.v[i] += (420. * (self.target[i] - self.p[i]) - 31. * self.v[i]) * dt / 4.;
                self.p[i] += self.v[i] * dt / 4.;
            }
            if (self.p[i] - self.target[i]).abs() < 0.05 && self.v[i].abs() < 0.1 { self.p[i] = self.target[i]; self.v[i] = 0.; }
            else { moving = true; }
        }
        moving
    }
}

pub(crate) struct Drag {
    id: String,
    start: Point<Pixels>,
    at: Point<Pixels>,
    offset: Point<Pixels>,
    started: bool,
    landing: bool,
    excluded: HashSet<String>,
    index: usize,
    depth: usize,
    initial_depth: usize,
    parent: Option<String>,
    original_index: usize,
    original_depth: usize,
    hover: Option<(String, Instant)>,
    ghost: Spring,
}

#[derive(Clone)]
struct PendingMove { id: String, parent: Option<String>, before: Option<String> }

#[derive(Default)]
pub(crate) struct SidebarDrag {
    pub active: Option<Drag>,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    springs: HashMap<String, Spring>,
    last_frame: Option<Instant>,
    pending: VecDeque<PendingMove>,
    saving: bool,
}

impl Workspace {
    fn drag_rows(&self) -> Vec<Row> {
        crate::tree::Tree::new(&self.entries).rows(|id| self.expanded.contains(id)).into_iter()
            .filter(|(e, _)| self.folder_drag.active.as_ref().is_none_or(|d| !d.started || !d.excluded.contains(&e.id)))
            .map(|(e, depth)| Row { entry: e.clone(), depth }).collect()
    }

    pub(crate) fn arm_folder_drag(&mut self, id: &str, at: Point<Pixels>, cx: &mut Context<Self>) {
        let rows = self.drag_rows();
        let Some(index) = rows.iter().position(|r| r.entry.id == id) else { return };
        let Some(bounds) = self.folder_drag.bounds.get() else { return };
        let Some(s) = self.folder_drag.springs.get(id) else { return };
        let origin = bounds.origin + point(px(s.p[0]), px(s.p[1]));
        self.folder_drag.active = Some(Drag {
            id: id.to_string(), start: at, at, offset: at - origin, started: false, landing: false,
            excluded: crate::tree::Tree::new(&self.entries).subtree(id).into_iter().collect(),
            index, depth: rows[index].depth, initial_depth: rows[index].depth, parent: rows[index].entry.meta.parent.clone(),
            original_index: index, original_depth: rows[index].depth, hover: None,
            ghost: Spring::new([f32::from(origin.x), f32::from(origin.y), s.p[2]]),
        });
        cx.notify();
    }

    pub(crate) fn move_folder_drag(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.folder_drag.active.as_mut() else { return };
        if d.landing { return; }
        d.at = at;
        if !d.started && (f32::from(at.x - d.start.x).hypot(f32::from(at.y - d.start.y)) >= 4.) {
            d.started = true;
            self.row_menu = None; self.create_menu = None;
            window.focus(&self.focus);
        }
        self.project_folder_drop();
        cx.notify();
    }

    fn project_folder_drop(&mut self) {
        let Some(bounds) = self.folder_drag.bounds.get() else { return };
        let rows = self.drag_rows();
        let Some(d) = self.folder_drag.active.as_mut().filter(|d| d.started && !d.landing) else { return };
        let y = f32::from(d.at.y - d.offset.y - bounds.top()) + 15.;
        let desired = (d.initial_depth as i32 + (f32::from(d.at.x - d.start.x) / INDENT).round() as i32).max(0) as usize;
        // Hit the visible folder row with the pointer, including collapsed folders.
        // Accept immediately; the dwell timer only controls automatic expansion.
        let pointer_y = f32::from(d.at.y - bounds.top());
        let folder = rows.iter().enumerate().find(|(i, r)| {
            let top = self.folder_drag.springs.get(&r.entry.id).map_or(
                (*i + usize::from(*i >= d.index)) as f32 * PITCH, |s| s.p[1]);
            r.entry.meta.folder.is_some() && pointer_y > top + 5. && pointer_y < top + 26.
                && f32::from(d.at.x - bounds.left()) >= r.depth as f32 * INDENT
        });
        if let Some((i, r)) = folder {
            if d.hover.as_ref().is_none_or(|(id, _)| *id != r.entry.id) {
                d.hover = Some((r.entry.id.clone(), Instant::now()));
                d.initial_depth = r.depth + 1; d.start.x = d.at.x;
            }
            d.index = i + 1;
            d.depth = r.depth + 1;
            d.parent = Some(r.entry.id.clone());
            if d.hover.as_ref().is_some_and(|(_, t)| t.elapsed().as_millis() >= 400) {
                self.expanded.insert(r.entry.id.clone());
            }
            return;
        }
        let slot_y = d.index as f32 * PITCH;
        let in_slot = y >= slot_y && y <= slot_y + PITCH;
        d.hover = None;
        let index = if in_slot { d.index } else {
            rows.iter().enumerate().filter(|(i, _)| y > (*i + usize::from(*i >= d.index)) as f32 * PITCH + 15.).count()
        };
        let prev = index.checked_sub(1).and_then(|i| rows.get(i));
        let max = prev.map_or(0, |r| r.depth + usize::from(r.entry.meta.folder.is_some()));
        let min = rows.get(index).map_or(0, |r| r.depth);
        let depth = desired.clamp(min.min(max), max);
        let parent = if depth == 0 { None } else { rows[..index].iter().rev().find(|r| r.depth + 1 == depth && r.entry.meta.folder.is_some()).map(|r| r.entry.id.clone()) };
        d.index = index; d.depth = depth; d.parent = parent;
    }

    pub(crate) fn folder_drop_target(&self, id: &str) -> bool {
        self.folder_drag.active.as_ref().is_some_and(|d| {
            let b = self.list_scroll.bounds();
            d.started && d.parent.as_deref() == Some(id)
                && d.at.x >= b.left() && d.at.x <= b.right() && d.at.y >= b.top() && d.at.y <= b.bottom()
        })
    }

    pub(crate) fn end_folder_drag(&mut self, cancel: bool, cx: &mut Context<Self>) {
        let rows = self.drag_rows();
        let Some(d) = self.folder_drag.active.as_mut() else { return };
        if !d.started { self.folder_drag.active = None; cx.notify(); return; }
        if d.landing { return; }
        let b = self.list_scroll.bounds();
        let cancel = cancel || d.at.x < b.left() || d.at.x > b.right() || d.at.y < b.top() || d.at.y > b.bottom();
        let before = rows.get(d.index).filter(|r| r.entry.meta.parent == d.parent).map(|r| r.entry.id.clone());
        let movement = PendingMove { id: d.id.clone(), parent: d.parent.clone(), before };
        let depth = if cancel { d.original_depth } else { d.depth };
        let index = if cancel { d.original_index } else { d.index };
        // Drop the overlay immediately: normal row color and hit testing resume this frame.
        // Seed the real row at the pointer so it settles without a second visual swap.
        let bounds = self.folder_drag.bounds.get();
        let mut spring = d.ghost;
        if let Some(b) = bounds { spring.p[0] -= f32::from(b.left()); spring.p[1] -= f32::from(b.top()); }
        spring.target = [depth as f32 * INDENT, index as f32 * PITCH, spring.p[2]];
        self.folder_drag.springs.insert(movement.id.clone(), spring);
        self.folder_drag.active = None;
        self.folder_drag.springs.remove("drop-placeholder");
        if !cancel {
            if let Some(parent) = &movement.parent { self.expanded.insert(parent.clone()); }
            self.apply_tree_move(&movement);
            self.folder_drag.pending.push_back(movement);
            self.save_next_tree_move(cx);
            self.sync_sub_pages(cx);
        }
        cx.notify();
    }

    pub(crate) fn reapply_pending_tree_moves(&mut self) {
        for movement in self.folder_drag.pending.clone() { self.apply_tree_move(&movement); }
    }

    fn apply_tree_move(&mut self, movement: &PendingMove) {
        let tree = crate::tree::Tree::new(&self.entries);
        let mut siblings: Vec<String> = tree.children(movement.parent.as_deref()).iter()
            .filter(|e| e.id != movement.id).map(|e| e.id.clone()).collect();
        let pos = movement.before.as_ref().and_then(|id| siblings.iter().position(|key| key == id)).unwrap_or(siblings.len());
        siblings.insert(pos, movement.id.clone());
        for e in &mut self.entries {
            if e.id == movement.id { e.meta.parent = movement.parent.clone(); }
            if let Some(i) = siblings.iter().position(|id| *id == e.id) {
                if movement.parent.is_none() { e.meta.root_order = Some(i as u64 + 1); }
                else { e.meta.order = i as u64 + 1; e.meta.root_order = None; }
            }
        }
    }

    fn save_next_tree_move(&mut self, cx: &mut Context<Self>) {
        if self.folder_drag.saving { return; }
        let Some(movement) = self.folder_drag.pending.front().cloned() else { return };
        self.folder_drag.saving = true;
        let mut writer = self.store.tree_writer();
        let task = cx.background_executor().spawn(async move {
            let result = writer.move_in_tree(&movement.id, movement.parent.as_deref(), movement.before.as_deref());
            let restored = if result.is_err() { Some(writer.list()) } else { None };
            (result, restored)
        });
        cx.spawn(async move |this, cx| {
            let (result, restored) = task.await;
            let _ = this.update(cx, |w, cx| {
                w.folder_drag.pending.pop_front();
                w.folder_drag.saving = false;
                if let Err(e) = result {
                    // Restore only hierarchy, preserving edits made while the write was pending.
                    if let Some(restored) = restored {
                        for entry in &mut w.entries {
                            if let Some(saved) = restored.iter().find(|e| e.id == entry.id) {
                                entry.meta.parent = saved.meta.parent.clone();
                                entry.meta.order = saved.meta.order;
                                entry.meta.root_order = saved.meta.root_order;
                            }
                        }
                    }
                    for movement in w.folder_drag.pending.clone() { w.apply_tree_move(&movement); }
                    w.sync_sub_pages(cx);
                    w.show_toast_kind(crate::ui::ToastKind::Error, &e.to_string(), cx);
                }
                w.save_next_tree_move(cx);
                cx.notify();
            });
        }).detach();
    }

    pub(crate) fn render_folder_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let now = Instant::now();
        let dt = self.folder_drag.last_frame.replace(now).map_or(1. / 120., |last| now.duration_since(last).as_secs_f32().min(0.032));
        self.project_folder_drop();
        let rows = self.drag_rows();
        let width = self.sidebar_w(window) - 2. * crate::sidebar::SB_X;
        let active = self.folder_drag.active.as_ref().filter(|d| d.started);
        let slot = active.map(|d| (d.index, d.depth));
        let mut container = div().relative().w_full().h(px((rows.len() + usize::from(slot.is_some())) as f32 * PITCH))
            .child({ let b = self.folder_drag.bounds.clone(); canvas(move |bounds, _, _| b.set(Some(bounds)), |_, _, _, _| {}).absolute().inset_0() });
        let receiving = rows.iter().position(|r| self.folder_drop_target(&r.entry.id));
        let receiving_end = receiving.map(|start| rows.iter().enumerate().skip(start + 1)
            .find(|(_, r)| r.depth <= rows[start].depth).map_or(rows.len(), |(i, _)| i));
        let mut moving = false;
        let mut row_elements = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            let y = (i + usize::from(slot.is_some_and(|(index, _)| i >= index))) as f32 * PITCH;
            let x = r.depth as f32 * INDENT;
            let inset = if receiving.zip(receiving_end).is_some_and(|(start, end)| i >= start && i < end) { 8. } else { 0. };
            let target = [x, y, width - x - inset];
            let spring = self.folder_drag.springs.entry(r.entry.id.clone()).or_insert_with(|| Spring::new(target));
            spring.target = target; moving |= spring.step(dt);
            let [x, y, w] = spring.p;
            row_elements.push(div().absolute().left(px(x)).top(px(y)).w(px(w))
                .child(self.doc_row(&r.entry, "doc", 0, r.entry.meta.folder.is_some(), cx)).into_any_element());
        }
        // A single rounded surface encloses the receiving folder's entire visible subtree.
        if let Some(start) = receiving {
            let depth = rows[start].depth;
            let end = rows.iter().enumerate().skip(start + 1).find(|(_, r)| r.depth <= depth).map_or(rows.len(), |(i, _)| i);
            let top = self.folder_drag.springs[&rows[start].entry.id].p[1];
            let mut bottom = rows[start..end].iter().map(|r| self.folder_drag.springs[&r.entry.id].p[1] + crate::sidebar::ROW_H).fold(top, f32::max);
            if let Some((index, _)) = slot {
                if index > start && index <= end { bottom = bottom.max(index as f32 * PITCH + crate::sidebar::ROW_H); }
            }
            let x = depth as f32 * INDENT;
            container = container.child(div().absolute().left(px(x)).top(px(top)).w(px(width - x)).h(px(bottom - top))
                .rounded(px(crate::sidebar::ROW_R)).bg(Theme::sidebar_hover())
                .border_1().border_dashed().border_color(Theme::text_disabled()));
        }
        container = container.children(row_elements);
        if let Some((index, depth)) = slot {
            let target = [depth as f32 * INDENT, index as f32 * PITCH, width - depth as f32 * INDENT - if receiving.is_some() { 8. } else { 0. }];
            let spring = self.folder_drag.springs.entry("drop-placeholder".into()).or_insert_with(|| Spring::new(target));
            spring.target = target; moving |= spring.step(dt);
            let [x, y, w] = spring.p;
            container = container.child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(crate::sidebar::ROW_H)).rounded(px(crate::sidebar::ROW_R)).bg(Theme::sidebar_selected()));
            if let Some(d) = &mut self.folder_drag.active {
                if let Some(b) = self.folder_drag.bounds.get() {
                    d.ghost.target = if d.landing { [f32::from(b.left()) + target[0], f32::from(b.top()) + target[1], target[2]] }
                        else { [f32::from(d.at.x - d.offset.x), f32::from(d.at.y - d.offset.y), target[2]] };
                }
                let unsettled = if d.landing { d.ghost.step(dt) } else {
                    // The lifted row follows the pointer exactly; only layout and landing spring.
                    d.ghost.p = d.ghost.target; d.ghost.v = [0.; 3]; false
                };
                moving |= unsettled;
                if d.landing && !unsettled { self.folder_drag.active = None; self.folder_drag.springs.remove("drop-placeholder"); moving = true; }
            }
        } else { self.folder_drag.springs.remove("drop-placeholder"); }
        if let Some(d) = self.folder_drag.active.as_ref().filter(|d| d.started && !d.landing) {
            let b = self.list_scroll.bounds();
            let delta = if d.at.y < b.top() + px(24.) { 1. } else if d.at.y > b.bottom() - px(24.) { -1. } else { 0. };
            if delta != 0. && d.at.x >= b.left() && d.at.x <= b.right() {
                let mut offset = self.list_scroll.offset(); offset.y += px(delta * dt * 420.); self.list_scroll.set_offset(offset); moving = true;
            }
            moving |= d.hover.as_ref().is_some_and(|(id, t)| !self.expanded.contains(id) && t.elapsed().as_millis() < 400);
        }
        if moving { window.request_animation_frame(); }
        container.into_any_element()
    }

    fn sidebar_click(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.entries.iter().any(|e| e.id == id && e.meta.folder.is_some()) {
            if !self.expanded.remove(id) { self.expanded.insert(id.to_string()); }
            cx.notify();
        } else { self.open(id, window, cx); }
    }

    pub(crate) fn track_sidebar_press(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        if !self.folder_drag.active.as_ref().is_some_and(|d| !d.started) { return; }
        if ev.pressed_button == Some(MouseButton::Left) { self.move_folder_drag(ev.position, window, cx); }
        else { self.folder_drag.active = None; }
    }

    pub(crate) fn finish_sidebar_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let click = self.folder_drag.active.as_ref().filter(|d| !d.started).map(|d| d.id.clone());
        if let Some(id) = click { self.folder_drag.active = None; self.sidebar_click(&id, window, cx); }
    }

    pub(crate) fn render_folder_drag_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.folder_drag.active.as_ref().filter(|d| d.started)?;
        let mut layer = div().id("folder-drag-overlay").absolute().inset_0().occlude()
            .on_mouse_move(cx.listener(|w, ev: &MouseMoveEvent, window, cx| {
                if ev.pressed_button != Some(MouseButton::Left) { w.end_folder_drag(true, cx); return; }
                w.move_folder_drag(ev.position, window, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|w, _, window, cx| {
                let click = w.folder_drag.active.as_ref().filter(|d| !d.started).map(|d| d.id.clone());
                if let Some(id) = click { w.folder_drag.active = None; w.sidebar_click(&id, window, cx); }
                else { w.end_folder_drag(false, cx); }
            }));
        if d.started && let Some(e) = self.entries.iter().find(|e| e.id == d.id) {
            let [x, y, width] = d.ghost.p;
            layer = layer.child(div().absolute().left(px(x)).top(px(y)).w(px(width)).h(px(crate::sidebar::ROW_H)).px(px(6.)).rounded(px(crate::sidebar::ROW_R))
                .bg(theme::raised(Theme::bg_body())).shadow(theme::shadow_panel()).flex().items_center().gap(px(9.))
                .child(icon(if e.meta.folder.is_some() { "folder" } else { "doc" }, 18., Theme::icon()))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().text_size(px(13.)).child(ui_text(crate::settings::display_title(&e.title)))));
        }
        Some(deferred(layer).with_priority(5).into_any_element())
    }
}
