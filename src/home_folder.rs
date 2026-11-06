//! 主页's folders: a glass folder with its documents tucked inside as paper,
//! and dragging a document card into one.
//!
//! The folder is painted, not laid out: the papers tilt and fan, which GPUI
//! elements cannot, so back plate, papers and the frosted front are paths in
//! two canvases (papers below, glass above), with the labels as ordinary
//! elements between and over them. Every motion is a spring
//! (`sidebar_drag::{Fade, Spring}`): hover lifts the papers out one after
//! another while the front leans back; a dragged card turns into a paper,
//! the folder opens its mouth under it, and a drop slides it in behind the
//! glass while the folder dips.
use crate::app::{RowMenu, Workspace};
use crate::settings::display_title;
use crate::sidebar_drag::{Fade, Spring};
use crate::store::Entry;
use crate::theme::{self, Theme, craft, ca};
use crate::ui::{TipExt, icon, ui_text};
use gpui::{prelude::FluentBuilder, *};
use std::{cell::RefCell, collections::HashMap, rc::Rc, time::Instant};

/// The tab's share of a folder card's height: the folder's body is a document
/// card's size, the tab rises above it (`home.rs` lowers the cards beside it).
pub(crate) const TAB: f32 = 0.06;
/// Papers peeking out of a folder.
const PAPERS: usize = 3;
/// The carried paper: the size a paper has inside a folder, from the moment it
/// is picked up until it is let go (same proportions as a card).
const MINI_W: f32 = 110.;
const MINI_H: f32 = MINI_W * 264. / 236.;

#[derive(Default)]
pub(crate) struct HomeFolders {
    fx: HashMap<String, FolderFx>,
    hovered: Option<String>,
    pub(crate) drag: Option<HomeDrag>,
    /// Last-frame window bounds of the folder and document cards, by entry id.
    bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
}

struct FolderFx {
    /// Hover: the papers rise, the front leans back.
    open: Fade,
    /// A card held over it: the papers part and the front drops further.
    mouth: Fade,
    /// Each paper's rise; the back ones are softer, so they follow the front.
    papers: [Fade; PAPERS],
    /// Kicked on press and on a drop: the front dips and springs back.
    dip: Fade,
    /// A dropped document sliding in (card-local x, y, w, h).
    arrival: Option<(String, Spring<4>)>,
    /// The paper kept in front: the last one dropped in.
    front: Option<String>,
    last: Option<Instant>,
}

impl FolderFx {
    fn new() -> Self {
        Self {
            open: Fade::new(0., 260., 21.),
            mouth: Fade::new(0., 300., 23.),
            papers: [Fade::new(0., 320., 21.), Fade::new(0., 230., 18.), Fade::new(0., 170., 15.5)],
            dip: Fade::new(0., 520., 15.),
            arrival: None,
            front: None,
            last: None,
        }
    }
}

pub(crate) struct HomeDrag {
    id: String,
    start: Point<Pixels>,
    at: Point<Pixels>,
    started: bool,
    /// Where on the card it was picked up (0–1), kept on the carried paper.
    grab: (f32, f32),
    /// The card's bounds when picked up; a cancelled drag flies back there.
    home: Bounds<Pixels>,
    /// The carried paper in window coordinates (x, y, w, h).
    rect: Spring<4>,
    over: Option<String>,
    returning: bool,
    last: Option<Instant>,
}

/// What lies on a paper: drawn lines, a real title over drawn lines, or real content.
#[derive(Clone, Copy, PartialEq)]
enum Face { Sketch, Titled, Content }

/// One paper's pose in card coordinates: centre, size and tilt (radians).
#[derive(Clone, Copy)]
struct Paper { cx: f32, cy: f32, w: f32, h: f32, rot: f32, seed: u64 }

/// Everything both canvases need for one frame.
#[derive(Clone)]
struct Pose {
    w: f32,
    h: f32,
    lift: f32,
    open: f32,
    /// Front's top edge: left (higher) and right, and how far its top corners reach in
    /// (negative: out, the flap tipping toward the viewer as it opens).
    yl: f32,
    yr: f32,
    inset: f32,
    papers: Vec<Paper>,
}

fn seed(id: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = rustc_hash::FxHasher::default();
    id.hash(&mut h);
    h.finish()
}

/// A rounded rectangle turned by `rot` about its centre (card-local `c`, painted at `o`).
fn rounded(o: Point<Pixels>, c: (f32, f32), w: f32, h: f32, r: f32, rot: f32) -> Option<Path<Pixels>> {
    let (s, k) = rot.sin_cos();
    let r = r.min(w / 2.).min(h / 2.);
    let mut pb = PathBuilder::fill();
    let mut first = true;
    for (ox, oy, start) in [(w / 2. - r, -h / 2. + r, -90f32), (w / 2. - r, h / 2. - r, 0.), (-w / 2. + r, h / 2. - r, 90.), (-w / 2. + r, -h / 2. + r, 180.)] {
        for i in 0..=6 {
            let a = (start + 15. * i as f32).to_radians();
            let (x, y) = (ox + r * a.cos(), oy + r * a.sin());
            let p = o + point(px(c.0 + x * k - y * s), px(c.1 + x * s + y * k));
            if first { pb.move_to(p); first = false; } else { pb.line_to(p); }
        }
    }
    pb.close();
    pb.build().ok()
}

impl Paper {
    /// A point on the paper, from its own top-left corner.
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        let (s, k) = self.rot.sin_cos();
        let (lx, ly) = (x - self.w / 2., y - self.h / 2.);
        (self.cx + lx * k - ly * s, self.cy + lx * s + ly * k)
    }

    /// The sheet, its hairline edge, a soft shadow and, unless real content
    /// lies on it, lines of "text" (below a real title when `titled`).
    fn paint(&self, o: Point<Pixels>, face: Face, window: &mut Window) {
        let b = Bounds::new(o + point(px(self.cx - self.w / 2.), px(self.cy - self.h / 2.)), size(px(self.w), px(self.h)));
        window.paint_shadows(b, Corners::all(px(10.)), &[BoxShadow { color: theme::rgbaf(0, 0, 0, if theme::dark() { 0.3 } else { 0.07 }), offset: point(px(0.), px(3.)), blur_radius: px(10.), spread_radius: px(0.) }]);
        if let Some(p) = rounded(o, (self.cx, self.cy), self.w + 1., self.h + 1., 10.5, self.rot) { window.paint_path(p, ca(craft::N1000, 0.08)); }
        if let Some(p) = rounded(o, (self.cx, self.cy), self.w, self.h, 10., self.rot) { window.paint_path(p, theme::raised(Theme::bg_body())); }
        let mut rng = self.seed;
        let mut next = || { rng ^= rng << 13; rng ^= rng >> 7; rng ^= rng << 17; (rng % 1000) as f32 / 1000. };
        if face == Face::Content { return; }
        let pad = 12.;
        let mut y = if face == Face::Titled { 36. } else {
            // An untitled paper still carries a heading.
            let len = (self.w - 2. * pad) * (0.42 + 0.3 * next());
            if let Some(p) = rounded(o, self.at(pad + len / 2., 16.), len, 4., 2., self.rot) { window.paint_path(p, ca(craft::N900, 0.55)); }
            30.
        };
        while y < self.h - 14. {
            let full = self.w - 2. * pad;
            let len = if next() < 0.2 { full * (0.3 + 0.3 * next()) } else { full * (0.72 + 0.28 * next()) };
            if let Some(p) = rounded(o, self.at(pad + len / 2., y), len, 3., 1.5, self.rot) { window.paint_path(p, ca(craft::N1000, 0.10)); }
            y += 8.5;
        }
    }
}

/// The folder's back: a plate with a tab on its top-left.
fn back_plate(o: Point<Pixels>, p: &Pose, window: &mut Window) {
    let (w, h, r) = (p.w, p.h, 18.);
    let top = TAB * h + p.lift;
    let tab_top = p.lift;
    let tab = 0.40 * w;
    let pt = |x: f32, y: f32| o + point(px(x), px(y));
    let mut pb = PathBuilder::fill();
    pb.move_to(pt(0., h - r + p.lift));
    pb.line_to(pt(0., tab_top + r));
    pb.curve_to(pt(r, tab_top), pt(0., tab_top));
    pb.line_to(pt(tab - 22., tab_top));
    pb.cubic_bezier_to(pt(tab + 14., top), pt(tab - 6., tab_top), pt(tab - 2., top));
    pb.line_to(pt(w - r, top));
    pb.curve_to(pt(w, top + r), pt(w, top));
    pb.line_to(pt(w, h - r + p.lift));
    pb.curve_to(pt(w - r, h + p.lift), pt(w, h + p.lift));
    pb.line_to(pt(r, h + p.lift));
    pb.curve_to(pt(0., h - r + p.lift), pt(0., h + p.lift));
    pb.close();
    if let Ok(path) = pb.build() {
        window.paint_path(path, linear_gradient(180., linear_color_stop(ca(craft::N1000, 0.11), 0.), linear_color_stop(ca(craft::N1000, 0.07), 1.)));
    }
}

/// The frosted front: a sheet whose top steps down from a raised left part,
/// milky at the top and nearly opaque where the name sits, with a lit edge.
fn glass(o: Point<Pixels>, p: &Pose, window: &mut Window) {
    let (w, h, r) = (p.w, p.h + p.lift, 18.);
    let (yl, yr, i) = (p.yl + p.lift, p.yr + p.lift, p.inset);
    let step = 0.44 * w;
    let pt = |x: f32, y: f32| o + point(px(x), px(y));
    let edge = |pb: &mut PathBuilder| {
        pb.move_to(pt(i, yl + r));
        pb.curve_to(pt(i + r, yl), pt(i, yl));
        pb.line_to(pt(step - 16., yl));
        pb.cubic_bezier_to(pt(step + 26., yr), pt(step + 6., yl), pt(step + 6., yr));
        pb.line_to(pt(w - i - r, yr));
        pb.curve_to(pt(w - i, yr + r), pt(w - i, yr));
    };
    // The whole folder's shadow sits under the front.
    let lift = -p.lift / 4.;
    window.paint_shadows(Bounds::new(pt(6., yr + 10.), size(px(w - 12.), px(h - yr - 10.))), Corners::all(px(r)),
        &[BoxShadow { color: theme::rgbaf(0, 0, 0, if theme::dark() { 0.35 } else { 0.06 + 0.04 * lift }), offset: point(px(0.), px(8. + 6. * lift)), blur_radius: px(22. + 10. * lift), spread_radius: px(-2.) }]);
    let mut pb = PathBuilder::fill();
    edge(&mut pb);
    pb.line_to(pt(w, h - r));
    pb.curve_to(pt(w - r, h), pt(w, h));
    pb.line_to(pt(r, h));
    pb.curve_to(pt(0., h - r), pt(0., h));
    pb.close();
    let face = theme::raised(Theme::bg_body());
    if let Ok(path) = pb.build() {
        // Milky where the papers show through, all but opaque by the name; a faint
        // cool tint keeps the glass apart from the white papers behind it.
        window.paint_path(path.clone(), linear_gradient(180., linear_color_stop(face.opacity(0.78), 0.), linear_color_stop(face.opacity(0.97), 0.42)));
        window.paint_path(path, linear_gradient(180., linear_color_stop(ca(craft::N1000, 0.012), 0.), linear_color_stop(ca(craft::N1000, 0.035), 1.)));
    }
    let mut hair = PathBuilder::stroke(px(1.));
    edge(&mut hair);
    hair.line_to(pt(w, h - r));
    hair.curve_to(pt(w - r, h), pt(w, h));
    hair.line_to(pt(r, h));
    hair.curve_to(pt(0., h - r), pt(0., h));
    hair.close();
    if let Ok(path) = hair.build() { window.paint_path(path, ca(craft::N1000, 0.07)); }
    // The lit top edge: brighter as the front leans into the light.
    let mut lit = PathBuilder::stroke(px(1.5));
    edge(&mut lit);
    if let Ok(path) = lit.build() { window.paint_path(path, theme::rgbaf(255, 255, 255, if theme::dark() { 0.18 } else { 0.85 + 0.15 * p.open.clamp(0., 1.) })); }
}

impl Workspace {
    fn folder_papers(&self, id: &str, front: Option<&String>) -> (Vec<Entry>, usize) {
        let mut kids: Vec<Entry> = self.entries.iter().filter(|e| e.meta.parent.as_deref() == Some(id)).cloned().collect();
        let n = kids.len();
        kids.sort_by(|a, b| {
            let lead = |e: &Entry| front.is_some_and(|f| *f == e.id);
            lead(b).cmp(&lead(a)).then(b.meta.opened.max(b.meta.modified).cmp(&a.meta.opened.max(a.meta.modified)))
        });
        kids.truncate(PAPERS);
        (kids, n)
    }

    pub(crate) fn render_folder_card(&mut self, e: &Entry, card_h: f32, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let id = e.id.clone();
        let w = self.home.folders.bounds.borrow().get(&id).map_or(236., |b| f32::from(b.size.width));
        let h = card_h;
        let hot = self.home.folders.hovered.as_ref() == Some(&id) && self.home.folders.drag.is_none();
        let over = self.home.folders.drag.as_ref().is_some_and(|d| d.started && !d.returning && d.over.as_ref() == Some(&id));
        let front = self.home.folders.fx.get(&id).and_then(|f| f.front.clone());
        let (kids, count) = self.folder_papers(&id, front.as_ref());

        // ---- Springs.
        let fx = self.home.folders.fx.entry(id.clone()).or_insert_with(FolderFx::new);
        let now = Instant::now();
        let dt = fx.last.replace(now).map_or(1. / 120., |t| now.duration_since(t).as_secs_f32().min(0.032));
        fx.open.to = if hot || over { 1. } else { 0. };
        fx.mouth.to = if over { 1. } else { 0. };
        for p in &mut fx.papers { p.to = fx.open.to + 0.35 * fx.mouth.to; }
        let mut moving = fx.open.step(dt) | fx.mouth.step(dt) | fx.dip.step(dt);
        for p in &mut fx.papers { moving |= p.step(dt); }
        let (o, m, dip) = (fx.open.p, fx.mouth.p, fx.dip.p);
        let lift = -5. * o - 2. * m + 3. * dip;
        let pose_of = |i: usize, r: f32| -> Paper {
            // (dx, top, tilt°) at rest; how far hover moves them; how far a held card parts them.
            let (rest, rise, part) = [
                // Kept inside the card: tilted corners stay clear of the neighbours.
                ((0.0, 0.16, 0.0), (0.0, -0.07, 0.0), (0.0, -0.03, 0.0)),
                ((0.03, 0.135, 2.5), (0.03, -0.055, 2.0), (0.01, -0.02, 1.5)),
                ((-0.035, 0.115, -3.0), (-0.03, -0.05, -2.5), (-0.01, -0.02, -1.5)),
            ][i];
            let (pw, ph) = (0.80 * w, 0.64 * h);
            let dx = (rest.0 + rise.0 * r + part.0 * m) * w;
            let top = (rest.1 + rise.1 * r + part.1 * m) * h + lift;
            Paper { cx: w / 2. + dx, cy: top + ph / 2., w: pw, h: ph, rot: (rest.2 + rise.2 * r + part.2 * m).to_radians(), seed: 0 }
        };
        let mut papers: Vec<Paper> = kids.iter().enumerate().map(|(i, k)| Paper { seed: seed(&k.id), ..pose_of(i, fx.papers[i].p) }).collect();
        if let Some((aid, s)) = &mut fx.arrival {
            let t = pose_of(0, fx.papers[0].p);
            s.target = [t.cx - t.w / 2., t.cy - t.h / 2., t.w, t.h];
            let still = !s.step(dt);
            moving |= !still;
            if let Some(first) = papers.first_mut().filter(|_| kids.first().is_some_and(|k| &k.id == aid)) {
                let [x, y, pw, ph] = s.p;
                *first = Paper { cx: x + pw / 2., cy: y + ph / 2., w: pw, h: ph, rot: 0., seed: first.seed };
            }
            if still { fx.arrival = None; }
        }
        if moving { window.request_animation_frame(); }
        let lean = 0.03 * h * o + 0.045 * h * m + 0.05 * h * dip;
        let pose = Pose { w, h, lift, open: o, yl: 0.49 * h + lean, yr: 0.54 * h + lean, inset: -(3. * o + 7. * m), papers };

        // ---- Elements: back canvas, the front paper's title, the glass, the labels.
        let record = self.home.folders.bounds.clone();
        let rid = id.clone();
        // The front paper is never tilted, so it carries the document's real first page.
        let front_face = if kids.first().is_some_and(|k| k.meta.folder.is_none()) { Face::Content } else { Face::Titled };
        let back = { let pose = pose.clone(); canvas(move |b, _, _| { record.borrow_mut().insert(rid.clone(), b); }, move |b, _, window, _| {
            back_plate(b.origin, &pose, window);
            for (i, p) in pose.papers.iter().enumerate().rev() { p.paint(b.origin, if i == 0 { front_face } else { Face::Sketch }, window); }
        }).absolute().inset_0() };
        let title = pose.papers.first().zip(kids.first()).map(|(p, k)| if front_face == Face::Content {
            div().absolute().left(px(p.cx - p.w / 2.)).top(px(p.cy - p.h / 2.)).w(px(p.w)).h(px(p.h)).rounded(px(10.)).overflow_hidden()
                .child(self.thumb_page(k, 12.))
        } else {
            div().absolute().left(px(p.cx - p.w / 2. + 12.)).top(px(p.cy - p.h / 2. + 10.)).w(px(p.w - 24.))
                .flex().items_center().gap(px(4.)).overflow_hidden()
                .text_size(px(11.)).line_height(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text())
                .when_some(k.icon.clone(), |d, em| d.child(div().flex_none().child(em)))
                .when(k.meta.folder.is_some(), |d| d.child(icon("folder", 12., Theme::icon())))
                .child(div().min_w_0().overflow_hidden().whitespace_nowrap().text_ellipsis().child(ui_text(display_title(&k.title))))
        });
        let front_glass = { let pose = pose.clone(); canvas(|_, _, _| {}, move |b, _, window, _| glass(b.origin, &pose, window)).absolute().inset_0() };
        let menu_on = self.row_menu.as_ref().is_some_and(|r| r.id == id);
        // The buttons come with hover only, not under a carried card.
        let reveal = if menu_on { 1. } else { (o - m).clamp(0., 1.) };
        let (open_id, menu_id, new_id) = (id.clone(), id.clone(), id.clone());
        let hover_id = id.clone();
        let label = div().absolute().left(px(16.)).right(px(14.)).bottom(px(14. - lift)).flex().items_end().gap(px(8.))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_size(px(15.)).line_height(px(22.)).font_weight(FontWeight::SEMIBOLD).text_color(Theme::text())
                    .overflow_hidden().whitespace_nowrap().text_ellipsis().child(ui_text(display_title(&e.title))))
                .child(div().text_size(px(12.)).line_height(px(18.)).text_color(Theme::text_caption())
                    .child(if count == 1 { crate::settings::tr("1 项", "1 item").into() } else { crate::localized_format!("{} 项", "{} items", count) })))
            .when(self.batch.is_none(), |d| d
                .child(div().id(SharedString::from(format!("home-fnew-{id}"))).flex_none().size(px(28.)).rounded_full().flex().items_center().justify_center()
                    .bg(theme::raised(Theme::bg_body())).shadow(theme::shadow_button()).opacity(reveal).cursor_pointer()
                    .hover(|s| s.bg(Theme::fill_hover_strong()))
                    .tip(crate::settings::tr("新建文档", "New document"), None)
                    .child(icon("plus", 15., Theme::icon()).with_transformation(Transformation::scale(size(0.7 + 0.3 * reveal, 0.7 + 0.3 * reveal))))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_click(cx.listener(move |w, _, window, cx| { cx.stop_propagation(); w.new_child(&new_id, window, cx); })))
                .child(self.home_more(e, format!("home-fmore-{id}"), cx).size(px(28.)).rounded_full().bg(theme::raised(Theme::bg_body())).shadow(theme::shadow_button()).opacity(reveal)));
        div().id(SharedString::from(format!("home-folder-{id}"))).relative().w_full().min_w_0().h(px(h)).cursor_pointer()
            .when(self.batch.is_some(), |d| d.opacity(0.45))
            .on_hover(cx.listener(move |w, on: &bool, _, cx| {
                if *on { w.home.folders.hovered = Some(hover_id.clone()); }
                else if w.home.folders.hovered.as_ref() == Some(&hover_id) { w.home.folders.hovered = None; }
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Left, cx.listener({ let id = id.clone(); move |w, _, _, cx| {
                // A press sinks the front a little; the release springs it back.
                if let Some(f) = w.home.folders.fx.get_mut(&id) { f.dip.v += 2.2; }
                cx.notify();
            }}))
            .when(self.batch.is_none(), |d| d
                .on_click(cx.listener(move |w, _, window, cx| { w.try_open(&open_id, window, cx); }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |w, ev: &MouseDownEvent, _, cx| {
                    w.row_menu = Some(RowMenu { id: menu_id.clone(), pos: ev.position, corner: Corner::TopLeft, sub: false, batch: false });
                    cx.notify();
                })))
            .child(back).children(title).child(front_glass).child(label)
            .into_any_element()
    }

    /// The card's bounds are kept for picking it up (and for the flight back).
    pub(crate) fn record_home_card(&self, id: &str) -> impl IntoElement {
        let record = self.home.folders.bounds.clone();
        let id = id.to_string();
        canvas(move |b, _, _| { record.borrow_mut().insert(id.clone(), b); }, |_, _, _, _| {}).absolute().inset_0()
    }

    pub(crate) fn arm_home_drag(&mut self, id: &str, at: Point<Pixels>) {
        let Some(home) = self.home.folders.bounds.borrow().get(id).copied() else { return };
        let grab = (((at.x - home.left()) / home.size.width).clamp(0., 1.), ((at.y - home.top()) / home.size.height).clamp(0., 1.));
        let r = [f32::from(home.left()), f32::from(home.top()), f32::from(home.size.width), f32::from(home.size.height)];
        self.home.folders.drag = Some(HomeDrag { id: id.to_string(), start: at, at, started: false, grab, home, rect: Spring::new(r), over: None, returning: false, last: None });
    }

    /// Before the drag starts the pointer is tracked by the home body.
    pub(crate) fn track_home_press(&mut self, ev: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(d) = self.home.folders.drag.as_mut().filter(|d| !d.started) else { return };
        if ev.pressed_button != Some(MouseButton::Left) { self.home.folders.drag = None; return; }
        d.at = ev.position;
        if f32::from(d.at.x - d.start.x).hypot(f32::from(d.at.y - d.start.y)) >= 5. {
            d.started = true;
            self.row_menu = None;
            cx.notify();
        }
    }

    pub(crate) fn home_dragging(&self, id: &str) -> bool {
        self.home.folders.drag.as_ref().is_some_and(|d| d.started && d.id == id)
    }

    fn move_home_drag(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.home.folders.drag.as_ref().is_some_and(|d| d.started && !d.returning) { return; }
        // Only folders on the page now: bounds linger for cards of pages left behind.
        let shown = if self.view == crate::app::View::Folder { self.level_entries(self.folder_current.as_deref()) } else { self.home_entries() };
        let Some(d) = self.home.folders.drag.as_mut() else { return };
        d.at = at;
        let over = shown.iter().filter(|e| e.meta.folder.is_some())
            .find(|e| self.home.folders.bounds.borrow().get(&e.id).is_some_and(|b| b.contains(&at)))
            .map(|e| e.id.clone());
        if over != d.over {
            // The folder notices: a small dip as its mouth opens.
            if let Some(f) = over.as_ref().and_then(|k| self.home.folders.fx.get_mut(k)) { f.dip.v -= 1.6; }
            d.over = over;
        }
        cx.notify();
    }

    fn end_home_drag(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.home.folders.drag.as_mut().filter(|d| d.started && !d.returning) else { self.home.folders.drag = None; return };
        let target = d.over.clone().and_then(|f| self.home.folders.bounds.borrow().get(&f).copied().map(|b| (f, b)));
        let Some((folder, b)) = target else {
            // Not over a folder: the paper flies back and becomes the card again.
            d.returning = true;
            d.rect.target = [f32::from(d.home.left()), f32::from(d.home.top()), f32::from(d.home.size.width), f32::from(d.home.size.height)];
            cx.notify();
            return;
        };
        let id = d.id.clone();
        // Hand the paper to the folder where it is; it slides in behind the glass.
        let [x, y, w, h] = d.rect.p;
        let local = [x - f32::from(b.left()), y - f32::from(b.top()), w, h];
        self.home.folders.drag = None;
        let fx = self.home.folders.fx.entry(folder.clone()).or_insert_with(FolderFx::new);
        fx.arrival = Some((id.clone(), Spring::new(local)));
        fx.front = Some(id.clone());
        fx.dip.v += 5.5;
        self.move_into_folder(&id, &folder, cx);
    }

    pub(crate) fn render_home_drag_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !matches!(self.view, crate::app::View::Home | crate::app::View::Folder) { self.home.folders.drag = None; return None; }
        let folder_bounds = self.home.folders.drag.as_ref().and_then(|d| d.over.clone()).and_then(|f| self.home.folders.bounds.borrow().get(&f).copied());
        let d = self.home.folders.drag.as_mut().filter(|d| d.started)?;
        let now = Instant::now();
        let dt = d.last.replace(now).map_or(1. / 120., |t| now.duration_since(t).as_secs_f32().min(0.032));
        if !d.returning {
            let (mut x, mut y) = (f32::from(d.at.x) - d.grab.0 * MINI_W, f32::from(d.at.y) - d.grab.1 * MINI_H);
            if let Some(b) = folder_bounds {
                // Drawn toward the folder's mouth, held just above the glass.
                let (fx, fy) = (f32::from(b.center().x) - MINI_W / 2., f32::from(b.top()) + 0.56 * f32::from(b.size.height) - MINI_H);
                x += (fx - x) * 0.45; y += (fy - y) * 0.45;
            }
            d.rect.target = [x, y, MINI_W, MINI_H];
        }
        let moving = d.rect.step(dt);
        if d.returning && !moving { self.home.folders.drag = None; cx.notify(); return None; }
        window.request_animation_frame();
        let [x, y, w, h] = d.rect.p;
        let e = self.entries.iter().find(|e| e.id == d.id)?.clone();
        let page = self.thumb_page(&e, 12.);
        let seed = seed(&e.id);
        let paper = Paper { cx: w / 2., cy: h / 2., w, h, rot: 0., seed };
        let lines = canvas(|_, _, _| {}, move |b, _, window, _| paper.paint(b.origin, Face::Content, window)).absolute().inset_0();
        let layer = div().id("home-drag-overlay").absolute().inset_0().occlude().cursor(CursorStyle::ClosedHand)
            .on_mouse_move(cx.listener(|w, ev: &MouseMoveEvent, _, cx| {
                if ev.pressed_button != Some(MouseButton::Left) { w.end_home_drag(cx); return; }
                w.move_home_drag(ev.position, cx);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|w, _, _, cx| w.end_home_drag(cx)))
            .child(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h))
                .shadow(theme::shadow_panel()).rounded(px(10.))
                .child(lines)
                .child(div().absolute().inset_0().rounded(px(10.)).overflow_hidden().child(page)));
        Some(deferred(layer).with_priority(5).into_any_element())
    }
}
