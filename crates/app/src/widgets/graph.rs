//! `Graph`: a live history graph of values in 0.0..=1.0.
//!
//! Keeps the last N values and draws them as a filled line in the accent color.
//! Every page reuses this widget; no page draws by itself.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

/// Graphs declared in Blueprint keep this many points unless they set
/// `capacity`; it matches the history every page shows.
const _: () = assert!(
    crate::config::HISTORY_LEN == 60,
    "update the `default = 60` of Graph's capacity property too"
);

mod imp {
    use super::*;

    #[derive(glib::Properties)]
    #[properties(wrapper_type = super::Graph)]
    pub struct Graph {
        pub values: RefCell<VecDeque<f32>>,
        /// Points kept; older ones scroll out on the left. Settable from
        /// Blueprint as `capacity: 60;`.
        #[property(get, set = Self::set_capacity, minimum = 2, default = 60)]
        pub capacity: Cell<u32>,
    }

    impl Default for Graph {
        fn default() -> Self {
            Self {
                values: RefCell::default(),
                capacity: Cell::new(60),
            }
        }
    }

    impl Graph {
        fn set_capacity(&self, capacity: u32) {
            self.capacity.set(capacity.max(2));
            let mut values = self.values.borrow_mut();
            while values.len() > self.capacity.get() as usize {
                values.pop_front();
            }
            drop(values);
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

    #[glib::derived_properties]
    impl ObjectImpl for Graph {
        /// Setup shared by `Graph::new` and graphs built from Blueprint.
        fn constructed(&self) {
            self.parent_constructed();
            let graph = self.obj();
            graph.set_hexpand(true);
            graph.set_overflow(gtk::Overflow::Hidden);
            graph.add_css_class("card");

            // Follow the user's accent color and light/dark style without a restart.
            let style = adw::StyleManager::default();
            style.connect_accent_color_notify(glib::clone!(
                #[weak]
                graph,
                move |_| graph.queue_draw()
            ));
            style.connect_dark_notify(glib::clone!(
                #[weak]
                graph,
                move |_| graph.queue_draw()
            ));
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
            if values.len() < 2 {
                return;
            }

            // Newest value at the right edge; older values move left.
            let step = w / (self.capacity.get().max(2) - 1) as f32;
            let x0 = w - step * (values.len() - 1) as f32;
            let y = |v: f32| h - v.clamp(0.0, 1.0) * (h - 2.0) - 1.0;

            // Collect points for easier calculation
            let points: Vec<(f32, f32)> = values
                .iter()
                .enumerate()
                .map(|(i, v)| (x0 + step * i as f32, y(*v)))
                .collect();

            let line = gsk::PathBuilder::new();
            let area = gsk::PathBuilder::new();

            // Setup initial points for both line and area
            let first = points[0];
            line.move_to(first.0, first.1);
            area.move_to(first.0, h); // Area starts from bottom
            area.line_to(first.0, first.1);

            // Draw smooth curves using cubic beziers
            for i in 1..points.len() {
                let (x_prev, y_prev) = points[i - 1];
                let (x_curr, y_curr) = points[i];

                // Control points for smooth horizontal easing
                let cp1_x = x_prev + step / 2.0;
                let cp1_y = y_prev;
                let cp2_x = x_curr - step / 2.0;
                let cp2_y = y_curr;

                line.cubic_to(cp1_x, cp1_y, cp2_x, cp2_y, x_curr, y_curr);
                area.cubic_to(cp1_x, cp1_y, cp2_x, cp2_y, x_curr, y_curr);
            }

            // Close the area path at the bottom right
            let last = points.last().unwrap();
            area.line_to(last.0, h);
            area.close();

            let accent = adw::StyleManager::default().accent_color_rgba();

            // 1. Draw Solid Fill (using append_fill instead of gradient
            //    because GTK4's push_clip only accepts Rect, not arbitrary Paths)
            let fill = gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), 0.25);
            snapshot.append_fill(&area.to_path(), gsk::FillRule::Winding, &fill);

            // 2. Draw the Line Stroke with rounded caps
            let stroke = gsk::Stroke::new(2.5);
            stroke.set_line_cap(gsk::LineCap::Round);
            snapshot.append_stroke(&line.to_path(), &stroke, &accent);

            // 3. Draw a dot at the latest data point (Live indicator)
            let dot_builder = gsk::PathBuilder::new();
            dot_builder.add_circle(&graphene::Point::new(last.0, last.1), 3.5);

            // Draw a subtle glow behind the dot
            let glow_color = gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), 0.3);
            let glow_builder = gsk::PathBuilder::new();
            glow_builder.add_circle(&graphene::Point::new(last.0, last.1), 6.0);
            snapshot.append_fill(&glow_builder.to_path(), gsk::FillRule::Winding, &glow_color);

            // Draw the solid dot
            snapshot.append_fill(&dot_builder.to_path(), gsk::FillRule::Winding, &accent);
        }
    }
}

glib::wrapper! {
    pub struct Graph(ObjectSubclass<imp::Graph>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Graph {
    /// A graph that keeps `capacity` points and asks for `height` pixels.
    pub fn new(capacity: usize, height: i32) -> Self {
        glib::Object::builder()
            .property("capacity", u32::try_from(capacity).unwrap_or(u32::MAX))
            .property("height-request", height)
            .build()
    }

    /// Append a value in 0.0..=1.0 and redraw.
    pub fn push(&self, value: f32) {
        let imp = self.imp();
        {
            let mut values = imp.values.borrow_mut();
            values.push_back(value);
            while values.len() > imp.capacity.get() as usize {
                values.pop_front();
            }
        }
        self.update_property(&[gtk::accessible::Property::Description(&format!(
            "{:.0}%",
            value * 100.0
        ))]);
        self.queue_draw();
    }
}
