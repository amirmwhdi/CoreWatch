//! `Graph`: a live history graph of values in 0.0..=1.0.
//!
//! Keeps the last N values and draws them as a filled line in the accent color.
//! Every page reuses this widget; no page draws by itself.
//!
//! A graph can also scale itself to the largest value it holds (for speeds,
//! which have no fixed maximum) and draw a second series as a plain line in
//! the text color (upload next to download).

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

thread_local! {
    /// Points every graph keeps: the history length from Preferences divided
    /// by the update interval.
    static HISTORY: Cell<u32> = const { Cell::new(60) };
    /// Every live graph, so a new history length reaches them all.
    static GRAPHS: RefCell<Vec<glib::WeakRef<Graph>>> = const { RefCell::new(Vec::new()) };
}

/// Make every graph, existing and future, keep `points` values.
pub fn set_history(points: usize) {
    let points = u32::try_from(points).unwrap_or(u32::MAX).max(2);
    HISTORY.with(|h| h.set(points));
    GRAPHS.with(|graphs| {
        graphs.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(graph) => {
                graph.imp().set_capacity(points);
                true
            }
            None => false,
        });
    });
}

mod imp {
    use super::*;

    pub struct Graph {
        pub values: RefCell<VecDeque<f32>>,
        /// Optional second series, drawn as a line without fill.
        pub secondary: RefCell<VecDeque<f32>>,
        /// `Some(floor)`: values are raw and the top of the graph is the
        /// largest value held, but never less than `floor`.
        pub autoscale: Cell<Option<f32>>,
        /// Points kept; older ones scroll out on the left. The same for
        /// every graph (see [`super::set_history`]).
        pub capacity: Cell<u32>,
        /// Whole percent last written to the accessible description, so an
        /// unchanged value does not reach the accessibility bus every tick.
        pub announced: Cell<Option<u8>>,
        /// StyleManager is global: its handlers must go when the graph does,
        /// or every removed disk block would leave two closures behind.
        pub style_handlers: RefCell<Vec<glib::SignalHandlerId>>,
    }

    impl Default for Graph {
        fn default() -> Self {
            Self {
                values: RefCell::default(),
                secondary: RefCell::default(),
                autoscale: Cell::new(None),
                capacity: Cell::new(HISTORY.with(Cell::get)),
                announced: Cell::new(None),
                style_handlers: RefCell::default(),
            }
        }
    }

    impl Graph {
        pub fn set_capacity(&self, capacity: u32) {
            self.capacity.set(capacity.max(2));
            for series in [&self.values, &self.secondary] {
                let mut values = series.borrow_mut();
                while values.len() > self.capacity.get() as usize {
                    values.pop_front();
                }
            }
            self.obj().queue_draw();
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Graph {
        const NAME: &'static str = "CwGraph";
        type Type = super::Graph;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("cw-graph");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
        }
    }

    impl ObjectImpl for Graph {
        /// Setup shared by `Graph::new` and graphs built from Blueprint.
        fn constructed(&self) {
            self.parent_constructed();
            let graph = self.obj();
            GRAPHS.with(|graphs| graphs.borrow_mut().push(graph.downgrade()));
            graph.set_hexpand(true);
            graph.set_overflow(gtk::Overflow::Hidden);
            graph.add_css_class("card");

            // Follow the user's accent color and light/dark style without a restart.
            let style = adw::StyleManager::default();
            let accent = style.connect_accent_color_notify(glib::clone!(
                #[weak]
                graph,
                move |_| graph.queue_draw()
            ));
            let dark = style.connect_dark_notify(glib::clone!(
                #[weak]
                graph,
                move |_| graph.queue_draw()
            ));
            *self.style_handlers.borrow_mut() = vec![accent, dark];
        }

        fn dispose(&self) {
            let style = adw::StyleManager::default();
            for handler in self.style_handlers.take() {
                style.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for Graph {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let (w, h) = (widget.width() as f32, widget.height() as f32);
            if w <= 0.0 || h <= 0.0 {
                return;
            }

            // Quiet guide lines at 25%, 50% and 75%.
            let fg = widget.color();
            let rule = gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.08);
            for share in [0.25_f32, 0.5, 0.75] {
                let y = (h * share).round();
                snapshot.append_color(&rule, &graphene::Rect::new(0.0, y, w, 1.0));
            }

            let values = self.values.borrow();
            let secondary = self.secondary.borrow();
            if values.len() < 2 {
                return;
            }

            // Fixed graphs hold 0.0..=1.0; self-scaling ones divide by the peak.
            let top = match self.autoscale.get() {
                Some(floor) => values
                    .iter()
                    .chain(secondary.iter())
                    .copied()
                    .fold(floor, f32::max),
                None => 1.0,
            };
            // Long histories hold more points than the graph has pixels:
            // draw the peak of every few points, one point per ~3 px.
            let capacity = self.capacity.get().max(2) as usize;
            let bucket = capacity.div_ceil(((w / 3.0) as usize).max(2));
            let (values, secondary) = (downsample(&values, bucket), downsample(&secondary, bucket));
            if values.len() < 2 {
                return;
            }
            let step = w / (capacity.div_ceil(bucket).max(2) - 1) as f32;
            let y = |v: f32| h - (v / top).clamp(0.0, 1.0) * (h - 2.0) - 1.0;

            let accent = adw::StyleManager::default().accent_color_rgba();
            // Adapt visual scale to widget size (crisp rendering on 48px core boxes)
            let stroke = gsk::Stroke::new(if h < 60.0 { 1.6 } else { 2.4 });
            stroke.set_line_cap(gsk::LineCap::Round);

            let (line, area) = series_paths(&values, step, w, h, y);
            // Solid fill (append_fill instead of a gradient, because GTK4's
            // push_clip only accepts Rect, not arbitrary paths), then the line.
            let fill = gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), 0.25);
            snapshot.append_fill(&area, gsk::FillRule::Winding, &fill);
            snapshot.append_stroke(&line, &stroke, &accent);

            if secondary.len() >= 2 {
                let (line, _) = series_paths(&secondary, step, w, h, y);
                let color = gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.55);
                snapshot.append_stroke(&line, &stroke, &color);
            }
        }
    }
}

/// The peak of every `bucket` values, counted from the newest so the right
/// edge stays steady while the graph scrolls. `bucket` 1 borrows as is.
fn downsample(values: &VecDeque<f32>, bucket: usize) -> std::borrow::Cow<'_, VecDeque<f32>> {
    if bucket <= 1 {
        return std::borrow::Cow::Borrowed(values);
    }
    let mut reduced: VecDeque<f32> = VecDeque::with_capacity(values.len() / bucket + 1);
    let mut iter = values.iter().rev().peekable();
    while iter.peek().is_some() {
        let peak = iter.by_ref().take(bucket).copied().fold(f32::MIN, f32::max);
        reduced.push_front(peak);
    }
    std::borrow::Cow::Owned(reduced)
}

/// The line through `values` (newest at the right edge, older ones to the
/// left) as smooth cubic curves, and the same line closed down to the bottom
/// for the fill.
fn series_paths(
    values: &VecDeque<f32>,
    step: f32,
    w: f32,
    h: f32,
    y: impl Fn(f32) -> f32 + Copy,
) -> (gsk::Path, gsk::Path) {
    let x0 = w - step * (values.len() - 1) as f32;
    let mut points = values
        .iter()
        .enumerate()
        .map(|(i, v)| (x0 + step * i as f32, y(*v)));
    let line = gsk::PathBuilder::new();
    let area = gsk::PathBuilder::new();
    if let Some(first) = points.next() {
        line.move_to(first.0, first.1);
        area.move_to(first.0, h);
        area.line_to(first.0, first.1);
        let mut prev = first;
        for curr in points {
            let (c1, c2) = ((prev.0 + step / 2.0, prev.1), (curr.0 - step / 2.0, curr.1));
            line.cubic_to(c1.0, c1.1, c2.0, c2.1, curr.0, curr.1);
            area.cubic_to(c1.0, c1.1, c2.0, c2.1, curr.0, curr.1);
            prev = curr;
        }
        area.line_to(w, h);
        area.close();
    }
    (line.to_path(), area.to_path())
}

glib::wrapper! {
    pub struct Graph(ObjectSubclass<imp::Graph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Graph {
    /// A graph that asks for `height` pixels and keeps the current history.
    pub fn new(height: i32) -> Self {
        glib::Object::builder()
            .property("height-request", height)
            .build()
    }

    /// A graph for values without a fixed maximum (speeds): the top of the
    /// graph follows the largest value shown, but stays at least `floor`.
    pub fn new_autoscale(height: i32, floor: f32) -> Self {
        let graph = Self::new(height);
        graph.imp().autoscale.set(Some(floor));
        graph
    }

    /// Append a value in 0.0..=1.0 and redraw.
    pub fn push(&self, value: f32) {
        let imp = self.imp();
        push_capped(&imp.values, value, imp.capacity.get());
        if imp.autoscale.get().is_some() {
            self.queue_draw();
            return;
        }
        let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u8;
        if imp.announced.replace(Some(percent)) != Some(percent) {
            self.update_property(&[gtk::accessible::Property::Description(&format!(
                "{percent}%"
            ))]);
        }
        self.queue_draw();
    }

    /// Append one value to each series and redraw. The accessible text is
    /// left to the page, which knows what the numbers mean.
    pub fn push_pair(&self, primary: f32, secondary: f32) {
        let imp = self.imp();
        push_capped(&imp.values, primary, imp.capacity.get());
        push_capped(&imp.secondary, secondary, imp.capacity.get());
        self.queue_draw();
    }
}

fn push_capped(series: &RefCell<VecDeque<f32>>, value: f32, capacity: u32) {
    let mut values = series.borrow_mut();
    values.push_back(value);
    while values.len() > capacity as usize {
        values.pop_front();
    }
}
