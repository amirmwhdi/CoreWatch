//! `Graph`: a live history graph of values in 0.0..=1.0.
//!
//! Keeps the last N values and draws them as a filled line in the accent color.
//! Every page reuses this widget; no page draws by itself.

use adw::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

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

            let line = gsk::PathBuilder::new();
            let area = gsk::PathBuilder::new();
            area.move_to(x0, h);
            for (i, v) in values.iter().enumerate() {
                let x = x0 + step * i as f32;
                if i == 0 {
                    line.move_to(x, y(*v));
                } else {
                    line.line_to(x, y(*v));
                }
                area.line_to(x, y(*v));
            }
            area.line_to(w, h);
            area.close();

            let accent = adw::StyleManager::default().accent_color_rgba();
            let fill = gdk::RGBA::new(accent.red(), accent.green(), accent.blue(), 0.18);
            snapshot.append_fill(&area.to_path(), gsk::FillRule::Winding, &fill);
            snapshot.append_stroke(&line.to_path(), &gsk::Stroke::new(2.0), &accent);
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
