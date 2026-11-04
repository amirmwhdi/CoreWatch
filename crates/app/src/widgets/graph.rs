//! `Graph`: a live history graph of values in 0.0..=1.0.
//!
//! Keeps the last N values and draws them as a filled line in the accent color.
//! Every page reuses this widget; no page draws by itself.
//!
//! A graph can also scale itself to the largest value it holds (for speeds,
//! which have no fixed maximum) and draw a second series as a plain line in
//! the text color (upload next to download).
//!
//! The line scrolls smoothly, as in GNOME System Monitor: the newest value
//! enters just past the right edge and slides into view over one update
//! interval, and a self-scaling graph eases to its new top. While on screen
//! the graph redraws only when the line has moved by a pixel, so a slow
//! scroll costs few frames, and a hidden graph costs nothing. "Smooth Graphs"
//! in Preferences turns this off ([`set_smooth`]), and so does the system's
//! power saver mode while it is on ([`follow_power_saver`]).

use adw::prelude::*;
use gio::prelude::PowerProfileMonitorExt;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// Assumed time between values until two have arrived.
const DEFAULT_INTERVAL_US: i64 = 1_000_000;
/// Bounds for the measured interval: a stalled tick must not freeze the
/// scroll for long, a burst must not race it.
const MIN_INTERVAL_US: i64 = 250_000;
const MAX_INTERVAL_US: i64 = 4_000_000;

thread_local! {
    /// Points every graph keeps: the history length from Preferences divided
    /// by the update interval.
    static HISTORY: Cell<u32> = const { Cell::new(60) };
    /// Whether graphs scroll between samples (Preferences, "Smooth Graphs").
    static SMOOTH: Cell<bool> = const { Cell::new(true) };
    /// The system's power saver mode is on: no scrolling, whatever SMOOTH says.
    static POWER_SAVER: Cell<bool> = const { Cell::new(false) };
    static POWER_MONITOR: RefCell<Option<gio::PowerProfileMonitor>> = const { RefCell::new(None) };
    /// Every live graph, so a new history length reaches them all.
    static GRAPHS: RefCell<Vec<glib::WeakRef<Graph>>> = const { RefCell::new(Vec::new()) };
}

/// Turn the continuous scroll of every graph on or off. Off, a graph moves
/// once per value and has no per-frame work at all.
pub fn set_smooth(smooth: bool) {
    let before = scrolling();
    SMOOTH.with(|s| s.set(smooth));
    apply_scrolling(before);
}

/// Stop scrolling while the system's power saver mode is on (GNOME's
/// "Power Saver" power mode), and start again when it goes off.
pub fn follow_power_saver() {
    let monitor = gio::PowerProfileMonitor::get_default();
    let update = |monitor: &gio::PowerProfileMonitor| {
        let before = scrolling();
        POWER_SAVER.with(|p| p.set(monitor.is_power_saver_enabled()));
        apply_scrolling(before);
    };
    update(&monitor);
    monitor.connect_power_saver_enabled_notify(update);
    // Kept for the life of the app, so its notifications keep coming.
    POWER_MONITOR.with(|m| m.replace(Some(monitor)));
}

/// Graphs scroll: "Smooth Graphs" is on and power saver mode is off.
fn scrolling() -> bool {
    SMOOTH.with(Cell::get) && !POWER_SAVER.with(Cell::get)
}

/// Start or stop every graph's scrolling after a change of [`scrolling`].
fn apply_scrolling(before: bool) {
    let smooth = scrolling();
    if smooth == before {
        return;
    }
    for_each_graph(|graph| {
        let imp = graph.imp();
        if smooth && graph.is_mapped() {
            imp.start_ticking();
        } else {
            imp.stop_ticking();
        }
        graph.queue_draw();
    });
}

fn for_each_graph(f: impl Fn(&Graph)) {
    GRAPHS.with(|graphs| {
        graphs.borrow_mut().retain(|weak| match weak.upgrade() {
            Some(graph) => {
                f(&graph);
                true
            }
            None => false,
        });
    });
}

/// Make every graph, existing and future, keep `points` values.
pub fn set_history(points: usize) {
    let points = u32::try_from(points).unwrap_or(u32::MAX).max(2);
    HISTORY.with(|h| h.set(points));
    for_each_graph(|graph| {
        if !graph.imp().fixed.get() {
            graph.imp().set_capacity(points);
        }
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
        /// `glib::monotonic_time` of the last value, in µs.
        pub pushed_at: Cell<i64>,
        /// Time between values, measured and smoothed, in µs.
        pub interval: Cell<i64>,
        /// Scroll offset and top of the last frame drawn, to skip frames
        /// that would look the same.
        pub drawn_shift: Cell<f32>,
        pub drawn_top: Cell<f32>,
        /// See [`Graph::target_top`].
        pub peak: Cell<f32>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        /// Values pushed since the graph was made, so a long history can be
        /// averaged in buckets that stay put while the line scrolls.
        pub pushed: Cell<u64>,
        /// A fixed series ([`super::Graph::set_values`]): no scrolling, and
        /// the history setting does not apply.
        pub fixed: Cell<bool>,
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
                pushed_at: Cell::new(0),
                interval: Cell::new(DEFAULT_INTERVAL_US),
                drawn_shift: Cell::new(0.0),
                drawn_top: Cell::new(1.0),
                peak: Cell::new(1.0),
                tick: RefCell::default(),
                pushed: Cell::new(0),
                fixed: Cell::new(false),
                style_handlers: RefCell::default(),
            }
        }
    }

    impl Graph {
        /// How far into the current interval we are, 0.0..=1.0: the share of
        /// one step the line has scrolled left since the last value.
        pub fn phase(&self) -> f32 {
            if self.pushed_at.get() == 0 || !scrolling() || self.fixed.get() {
                return 1.0;
            }
            let elapsed = glib::monotonic_time() - self.pushed_at.get();
            (elapsed as f32 / self.interval.get().max(1) as f32).clamp(0.0, 1.0)
        }

        /// The top the graph scales to: 1.0, or the largest value held.
        /// Kept up to date when values change, not on every frame.
        pub fn target_top(&self) -> f32 {
            self.peak.get()
        }

        /// Find the largest value held again, after values changed.
        pub fn update_peak(&self) {
            let peak = match self.autoscale.get() {
                Some(floor) => self
                    .values
                    .borrow()
                    .iter()
                    .chain(self.secondary.borrow().iter())
                    .copied()
                    .fold(floor, f32::max),
                None => 1.0,
            };
            self.peak.set(peak);
        }

        /// Distance between two points in pixels. One point lies past each
        /// edge, so the line always reaches both sides while it scrolls.
        pub fn step(&self, width: f32) -> f32 {
            width / (self.capacity.get().max(3) - 2) as f32
        }

        /// Redraw on frames where the line has moved a pixel or the scale
        /// is still easing.
        pub fn start_ticking(&self) {
            if self.tick.borrow().is_some() || self.fixed.get() {
                return;
            }
            let id = self.obj().add_tick_callback(|graph, _| {
                let imp = graph.imp();
                let shift = imp.phase() * imp.step(graph.width() as f32);
                let top = imp.target_top();
                let moved = (shift - imp.drawn_shift.get()).abs() >= 1.0;
                let rescaling = (top - imp.drawn_top.get()).abs() > top * 0.002;
                if moved || rescaling {
                    graph.queue_draw();
                }
                glib::ControlFlow::Continue
            });
            self.tick.replace(Some(id));
        }

        pub fn stop_ticking(&self) {
            if let Some(id) = self.tick.take() {
                id.remove();
            }
        }

        /// Record a new value's time and learn the interval from it.
        pub fn mark_push(&self) {
            let now = glib::monotonic_time();
            let last = self.pushed_at.replace(now);
            if last != 0 {
                let measured = (now - last).clamp(MIN_INTERVAL_US, MAX_INTERVAL_US);
                // Smooth out timer jitter; follow a new update speed in a few values.
                let smoothed = (self.interval.get() * 2 + measured) / 3;
                self.interval.set(smoothed);
            }
            self.obj().queue_draw();
        }

        pub fn set_capacity(&self, capacity: u32) {
            self.capacity.set(capacity.max(2));
            for series in [&self.values, &self.secondary] {
                let mut values = series.borrow_mut();
                while values.len() > self.capacity.get() as usize {
                    values.pop_front();
                }
            }
            self.update_peak();
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
            GRAPHS.with(|graphs| {
                let mut graphs = graphs.borrow_mut();
                // Drop the graphs that are gone (a removed disk or network
                // interface), so the list does not grow for ever.
                graphs.retain(|weak| weak.upgrade().is_some());
                graphs.push(graph.downgrade());
            });
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

            // Scroll only while on screen.
            graph.connect_map(|graph| {
                if scrolling() {
                    graph.imp().start_ticking();
                }
            });
            graph.connect_unmap(|graph| graph.imp().stop_ticking());
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

            // Fixed graphs hold 0.0..=1.0; self-scaling ones divide by the
            // peak, easing toward a new peak instead of jumping to it.
            let target = self.target_top();
            let top = if self.autoscale.get().is_some() && scrolling() {
                let shown = self.drawn_top.get();
                if shown <= 0.0 || (target - shown).abs() <= target * 0.002 {
                    target
                } else {
                    shown + (target - shown) * 0.15
                }
            } else {
                1.0
            };
            self.drawn_top.set(top);

            let step = self.step(w);
            let shift = self.phase() * step;
            self.drawn_shift.set(shift);
            // The newest point starts one step past the right edge and
            // reaches it as the next value is due.
            let newest_x = w + step - shift;
            let y = |v: f32| h - (v / top).clamp(0.0, 1.0) * (h - 2.0) - 1.0;

            let accent = adw::StyleManager::default().accent_color_rgba();
            // Adapt visual scale to widget size (crisp rendering on 48px core boxes)
            let stroke = gsk::Stroke::new(if h < 60.0 { 1.6 } else { 2.4 });
            stroke.set_line_cap(gsk::LineCap::Round);

            // More points than half the width in pixels: average them in
            // buckets, so an hour of samples is not drawn one by one.
            let max_points = ((w / 2.0) as usize).max(8);
            let pushed = self.pushed.get();
            let (line, area) = series_paths(&values, pushed, max_points, newest_x, step, h, y);
            // Solid fill (append_fill instead of a gradient, because GTK4's
            // push_clip only accepts Rect, not arbitrary paths), then the line.
            let fill = gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), 0.25);
            snapshot.append_fill(&area, gsk::FillRule::Winding, &fill);
            snapshot.append_stroke(&line, &stroke, &accent);

            if secondary.len() >= 2 {
                let (line, _) = series_paths(&secondary, pushed, max_points, newest_x, step, h, y);
                let color = gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), 0.55);
                snapshot.append_stroke(&line, &stroke, &color);
            }
        }
    }
}

/// `values` (the newest was the `pushed`th value) as at most about
/// `max_points` points: each bucket of `k` values is averaged. Buckets are
/// counted from the first value ever pushed, so they do not shift as values
/// arrive. Returns (index into `values`, value) pairs; the index of a bucket
/// is its middle.
pub fn bucket(values: &VecDeque<f32>, pushed: u64, max_points: usize) -> Vec<(f32, f32)> {
    let len = values.len();
    let k = len.div_ceil(max_points.max(1)).max(1);
    if k == 1 {
        return values
            .iter()
            .enumerate()
            .map(|(i, v)| (i as f32, *v))
            .collect();
    }
    // Absolute index of values[0].
    let first = pushed.saturating_sub(len as u64);
    let mut points = Vec::with_capacity(len / k + 2);
    let (mut bucket, mut sum, mut count, mut index_sum) = (None, 0.0, 0usize, 0.0);
    for (i, v) in values.iter().enumerate() {
        let id = (first + i as u64) / k as u64;
        if bucket != Some(id) && count > 0 {
            points.push((index_sum / count as f32, sum / count as f32));
            (sum, count, index_sum) = (0.0, 0, 0.0);
        }
        bucket = Some(id);
        sum += v;
        count += 1;
        index_sum += i as f32;
    }
    if count > 0 {
        points.push((index_sum / count as f32, sum / count as f32));
    }
    points
}

/// The line through `values` (newest at `newest_x`, older ones `step` apart
/// to the left) as smooth cubic curves, and the same line closed down to the
/// bottom for the fill. Long series are averaged first, see [`bucket`].
fn series_paths(
    values: &VecDeque<f32>,
    pushed: u64,
    max_points: usize,
    newest_x: f32,
    step: f32,
    h: f32,
    y: impl Fn(f32) -> f32 + Copy,
) -> (gsk::Path, gsk::Path) {
    let x0 = newest_x - step * (values.len() - 1) as f32;
    let mut points = bucket(values, pushed, max_points)
        .into_iter()
        .map(|(i, v)| (x0 + step * i, y(v)));
    let line = gsk::PathBuilder::new();
    let area = gsk::PathBuilder::new();
    if let Some(first) = points.next() {
        line.move_to(first.0, first.1);
        area.move_to(first.0, h);
        area.line_to(first.0, first.1);
        let mut prev = first;
        for curr in points {
            let half = (curr.0 - prev.0) / 2.0;
            let (c1, c2) = ((prev.0 + half, prev.1), (curr.0 - half, curr.1));
            line.cubic_to(c1.0, c1.1, c2.0, c2.1, curr.0, curr.1);
            area.cubic_to(c1.0, c1.1, c2.0, c2.1, curr.0, curr.1);
            prev = curr;
        }
        area.line_to(prev.0, h);
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
        graph.imp().update_peak();
        graph
    }

    /// Show these values, oldest first, as they are: no scrolling, and the
    /// graph history setting does not apply (the battery's last day).
    pub fn set_values(&self, values: &[f32]) {
        let imp = self.imp();
        imp.fixed.set(true);
        imp.stop_ticking();
        imp.capacity
            .set(u32::try_from(values.len()).unwrap_or(u32::MAX).max(2));
        *imp.values.borrow_mut() = values.iter().copied().collect();
        imp.pushed.set(values.len() as u64);
        imp.update_peak();
        self.queue_draw();
    }

    /// Append a value in 0.0..=1.0 and redraw.
    pub fn push(&self, value: f32) {
        let imp = self.imp();
        imp.pushed.set(imp.pushed.get() + 1);
        push_capped(&imp.values, value, imp.capacity.get());
        imp.update_peak();
        imp.mark_push();
        if imp.autoscale.get().is_some() {
            return;
        }
        let percent = (value.clamp(0.0, 1.0) * 100.0).round() as u8;
        if imp.announced.replace(Some(percent)) != Some(percent) {
            self.update_property(&[gtk::accessible::Property::Description(&format!(
                "{percent}%"
            ))]);
        }
    }

    /// Append one value to each series and redraw. The accessible text is
    /// left to the page, which knows what the numbers mean.
    pub fn push_pair(&self, primary: f32, secondary: f32) {
        let imp = self.imp();
        imp.pushed.set(imp.pushed.get() + 1);
        push_capped(&imp.values, primary, imp.capacity.get());
        push_capped(&imp.secondary, secondary, imp.capacity.get());
        imp.update_peak();
        imp.mark_push();
    }
}

fn push_capped(series: &RefCell<VecDeque<f32>>, value: f32, capacity: u32) {
    let mut values = series.borrow_mut();
    values.push_back(value);
    while values.len() > capacity as usize {
        values.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::bucket;
    use std::collections::VecDeque;

    #[test]
    fn short_series_are_drawn_as_they_are() {
        let values: VecDeque<f32> = [0.1, 0.2, 0.3].into();
        assert_eq!(
            bucket(&values, 3, 100),
            [(0.0, 0.1), (1.0, 0.2), (2.0, 0.3)]
        );
    }

    #[test]
    fn long_series_are_averaged_in_buckets_that_stay_put() {
        let values: VecDeque<f32> = (0..100).map(|i| i as f32).collect();
        let points = bucket(&values, 100, 10);
        assert_eq!(points.len(), 10);
        assert_eq!(points[0], (4.5, 4.5));
        // One more value: the old buckets keep their values, the series
        // only moved by one index.
        let mut later = values.clone();
        later.pop_front();
        later.push_back(100.0);
        let moved = bucket(&later, 101, 10);
        assert_eq!(moved[1], (points[1].0 - 1.0, points[1].1));
    }
}
