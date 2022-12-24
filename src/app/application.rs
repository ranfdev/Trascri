/* application.rs
 *
 * Copyright 2022 Lorenzo Miglietta
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 */

use adw::subclass::prelude::*;
use glib::clone;
use gtk::prelude::*;
use gtk::{gio, glib};

use crate::config::VERSION;
use crate::TrascriWindow;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct TrascriApplication {}

    #[glib::object_subclass]
    impl ObjectSubclass for TrascriApplication {
        const NAME: &'static str = "TrascriApplication";
        type Type = super::TrascriApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for TrascriApplication {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = self.obj();

            obj.setup_gactions();
            obj.set_accels_for_action("app.quit", &["<primary>q"]);
        }
    }

    impl ApplicationImpl for TrascriApplication {
        // We connect to the activate callback to create a window when the application
        // has been launched. Additionally, this callback notifies us when the user
        // tries to launch a "second instance" of the application. When they try
        // to do that, we'll just present any existing window.
        fn activate(&self) {
            let obj = self.obj();
            // Get the current window or create one if necessary
            let window = if let Some(window) = obj.active_window() {
                window
            } else {
                let window = TrascriWindow::new(&*obj);
                window.upcast()
            };

            // Ask the window manager/compositor to present the window
            window.present();
        }
    }

    impl GtkApplicationImpl for TrascriApplication {}
    impl AdwApplicationImpl for TrascriApplication {}
}

glib::wrapper! {
    pub struct TrascriApplication(ObjectSubclass<imp::TrascriApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

impl TrascriApplication {
    pub fn new(application_id: &str, flags: &gio::ApplicationFlags) -> Self {
        glib::Object::builder()
            .property("application-id", &application_id)
            .property("flags", flags)
            .build()
    }

    fn setup_gactions(&self) {
        let quit_action = gio::SimpleAction::new("quit", None);
        quit_action.connect_activate(clone!(@weak self as app => move |_, _| {
            app.quit();
        }));
        self.add_action(&quit_action);

        let about_action = gio::SimpleAction::new("about", None);
        about_action.connect_activate(clone!(@weak self as app => move |_, _| {
            app.show_about();
        }));
        self.add_action(&about_action);
    }

    fn show_about(&self) {
        let window = self.active_window().unwrap();
        let about = adw::AboutWindow::builder()
            .transient_for(&window)
            .application_name("trascri")
            .application_icon("com.ranfdev.Trascri")
            .developer_name("ranfdev")
            .version(VERSION)
            .developers(vec!["ranfdev".into()])
            .copyright("© 2022 Lorenzo Miglietta (ranfdev)")
            .build();

        about.present();
    }
}
