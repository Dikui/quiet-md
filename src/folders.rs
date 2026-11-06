use crate::app::{View, Workspace};
use crate::settings::{display_title, tr};
use crate::theme::Theme;
use crate::ui::{icon, popover, ui_text};
use gpui::*;

impl Workspace {
    pub(crate) fn new_root_doc(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.save_before(cx, |w, window, cx| w.new_root_doc(window, cx)) { return; }
        match self.store.create() {
            Ok((id, _)) => { self.entries = self.store.list(); self.sort(); self.folded[1] = false; self.open(&id, window, cx); }
            Err(e) => self.show_toast_kind(crate::ui::ToastKind::Error, &e.to_string(), cx),
        }
        cx.notify();
    }

    pub(crate) fn copy_folder(&mut self, id: &str, cx: &mut Context<Self>) -> Option<String> {
        let ids = crate::tree::Tree::new(&self.entries).subtree(id);
        let mut copied = std::collections::HashMap::<String, String>::new();
        for key in ids {
            let Some(entry) = self.entries.iter().find(|e| e.id == key).cloned() else { continue };
            let parent = entry.meta.parent.as_ref().map(|p| copied.get(p).cloned().unwrap_or_else(|| p.clone()));
            let result = if entry.meta.folder.is_some() {
                let title = if key == id { crate::localized_format!("{} 副本", "{} copy", entry.title) } else { entry.title };
                self.store.create_folder(&title, parent.as_deref())
            } else {
                self.store.load(&key).and_then(|doc| self.store.create_from(&doc)).and_then(|new| {
                    self.store.set_parent(&new, parent.as_deref())?;
                    self.store.set_width(&new, entry.meta.width)?;
                    Ok(new)
                })
            };
            match result {
                Ok(new) => { copied.insert(key, new); }
                Err(e) => { self.show_toast_kind(crate::ui::ToastKind::Error, &e.to_string(), cx); return None; }
            }
        }
        copied.remove(id)
    }

    pub(crate) fn creation_parent(&self) -> Option<String> {
        if self.view == View::Folder { self.folder_current.clone() }
        else if self.view == View::Doc { self.current.as_ref().and_then(|id| self.entries.iter().find(|e| &e.id == id)).and_then(|e| e.meta.parent.clone()) }
        else { None }
    }

    pub(crate) fn open_folder(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let target = id.to_string();
        if !self.save_before(cx, move |w, window, cx| { w.open_folder(&target, window, cx); }) { return false; }
        self.editor = None;
        self.current = None;
        self.folder_current = Some(id.to_string());
        self.view = View::Folder;
        self.row_menu = None;
        self.create_menu = None;
        self.doc_menu = false;
        self.expanded.insert(id.to_string());
        self.reveal_in_tree(id);
        window.focus(&self.focus);
        cx.notify();
        true
    }

    pub(crate) fn new_folder(&mut self, parent: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.create_menu = None;
        match self.store.create_folder(tr("未命名文件夹", "Untitled folder"), parent.as_deref()) {
            Ok(id) => {
                self.entries = self.store.list(); self.sort();
                if let Some(p) = parent { self.expanded.insert(p); }
                self.folded[1] = false;
                self.start_rename(&id, window, cx);
            }
            Err(e) => self.show_toast_kind(crate::ui::ToastKind::Error, &e.to_string(), cx),
        }
        cx.notify();
    }

    pub(crate) fn render_create_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (parent, pos) = self.create_menu.clone()?;
        let folder_parent = parent.clone();
        let row = |id: &'static str, glyph: &'static str, label: &'static str| crate::ui::menu_row(id, SharedString::from(id)).child(icon(glyph, 16., Theme::icon())).child(label);
        Some(deferred(anchored().position(pos).snap_to_window_with_margin(px(8.)).child(
            popover().id("create-items").occlude().w(px(180.)).p(px(5.))
                .on_mouse_down_out(cx.listener(|w, _, _, cx| { w.create_menu = None; cx.notify(); }))
                .child(row("create-document", "doc", tr("新建文档", "New document")).on_click(cx.listener(move |w, _, window, cx| {
                    w.create_menu = None;
                    if let Some(p) = &parent { w.new_child(p, window, cx); }
                    else {
                        // A top-level plus always creates at the top level.
                        let old = w.view; w.view = View::Home; w.new_doc(window, cx);
                        if w.view == View::Home { w.view = old; }
                    }
                })))
                .child(row("create-folder", "folder", tr("新建文件夹", "New folder")).on_click(cx.listener(move |w, _, window, cx| w.new_folder(folder_parent.clone(), window, cx))))
        )).with_priority(3).into_any_element())
    }

    pub(crate) fn render_folder(&self, cx: &mut Context<Self>) -> AnyElement {
        let id = self.folder_current.clone().unwrap_or_default();
        let title = self.entries.iter().find(|e| e.id == id).map(|e| display_title(&e.title)).unwrap_or_else(|| tr("文件夹", "Folder").to_string());
        let mut breadcrumbs = div().flex().items_center().gap(px(8.)).text_size(px(12.)).text_color(Theme::text_caption())
            .child(div().id("folder-home").cursor_pointer().child(tr("我的文档", "My documents")).on_click(cx.listener(|w, _, _, cx| w.show_view(View::Home, cx))));
        for e in self.ancestors(&id) {
            let target = e.id.clone();
            breadcrumbs = breadcrumbs.child("/").child(div().id(SharedString::from(format!("folder-path-{}", e.id))).cursor_pointer().child(ui_text(display_title(&e.title)))
                .on_click(cx.listener(move |w, _, window, cx| w.open(&target, window, cx))));
        }
        let folder = id.clone();
        let doc = id.clone();
        let children: Vec<_> = crate::tree::Tree::new(&self.entries).children(Some(&id)).iter().map(|e| (*e).clone()).collect();
        let mut list = div().id("folder-contents").flex_1().min_h_0().overflow_y_scroll().px(px(36.)).py(px(12.));
        if children.is_empty() { list = list.child(div().py(px(80.)).text_center().text_size(px(14.)).text_color(Theme::text_caption()).child(tr("这个文件夹还是空的", "This folder is empty"))); }
        for e in children {
            let target = e.id.clone();
            list = list.child(div().id(SharedString::from(format!("folder-entry-{}", e.id))).h(px(44.)).px(px(12.)).rounded(px(8.)).flex().items_center().gap(px(12.))
                .cursor_pointer().hover(|s| s.bg(Theme::fill_hover())).text_size(px(14.))
                .child(icon(if e.meta.folder.is_some() { "folder" } else { "doc" }, 18., Theme::icon()))
                .child(ui_text(display_title(&e.title))).on_click(cx.listener(move |w, _, window, cx| w.open(&target, window, cx))));
        }
        div().size_full().flex().flex_col()
            .child(crate::trash::page_header(self, tr("文件夹", "Folder"), 26., cx))
            .child(div().px(px(48.)).pt(px(28.)).pb(px(24.)).child(breadcrumbs)
                .child(div().mt(px(24.)).flex().items_center().gap(px(12.)).child(icon("folder", 28., Theme::icon())).child(div().text_size(px(28.)).font_weight(FontWeight::SEMIBOLD).child(ui_text(title)))))
            .child(div().px(px(48.)).pb(px(16.)).flex().gap(px(10.))
                .child(crate::trash::secondary_button("folder-new-doc", tr("新建文档", "New document")).on_click(cx.listener(move |w, _, window, cx| w.new_child(&doc, window, cx))))
                .child(crate::trash::secondary_button("folder-new-folder", tr("新建文件夹", "New folder")).on_click(cx.listener(move |w, _, window, cx| w.new_folder(Some(folder.clone()), window, cx)))))
            .child(list).into_any_element()
    }

    pub(crate) fn folder_row_menu(&self, m: crate::app::RowMenu, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let (doc, folder, rename, mv, del) = (m.id.clone(), m.id.clone(), m.id.clone(), m.id.clone(), m.id.clone());
        let row = |id: &'static str, glyph: &'static str, label: &'static str| crate::ui::menu_row(id, SharedString::from(id)).child(icon(glyph, 16., Theme::icon())).child(label);
        let pos = if m.pos.x < px(0.) { point(-m.pos.x, m.pos.y + px(22.)) } else { m.pos };
        vec![deferred(anchored().position(pos).snap_to_window_with_margin(px(8.)).child(popover().id("folder-row-menu").occlude().w(px(190.)).p(px(5.))
            .on_mouse_down_out(cx.listener(|w, _, _, cx| { w.row_menu = None; cx.notify(); }))
            .child(row("fr-new-doc", "doc", tr("新建文档", "New document")).on_click(cx.listener(move |w, _, window, cx| { w.row_menu = None; w.new_child(&doc, window, cx); })))
            .child(row("fr-new-folder", "folder", tr("新建文件夹", "New folder")).on_click(cx.listener(move |w, _, window, cx| { w.row_menu = None; w.new_folder(Some(folder.clone()), window, cx); })))
            .child(row("fr-rename", "square-pen", tr("重命名", "Rename")).on_click(cx.listener(move |w, _, window, cx| w.start_rename(&rename, window, cx))))
            .child(row("fr-move", "folder-input", tr("移动到", "Move to")).on_click(cx.listener(move |w, _, window, cx| w.open_move(vec![mv.clone()], window, cx))))
            .child(row("fr-delete", "bin", tr("删除", "Delete")).on_click(cx.listener(move |w, _, _, cx| w.ask_delete(&del, cx))))))
            .with_priority(2).into_any_element()]
    }
}
