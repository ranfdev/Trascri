/* window.rs
 *
 * Copyright 2022 Unknown
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

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gst::prelude::DeviceExt;
use gtk::{gio, glib, CompositeTemplate};

use crate::models_repo::{ModelsRepo, RemoteModel};
use crate::ports::*;
use crate::transcriber::*;
use crate::transcriber::Msg;

const SAMPLE_RATE: i32 = 16000;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/com/ranfdev/Trascri/window.ui")]
    pub struct TrascriWindow {
        // Template widgets
        #[template_child]
        pub header_bar: TemplateChild<gtk::HeaderBar>,
        #[template_child]
        pub text_view: TemplateChild<gtk::TextView>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub flap: TemplateChild<adw::Flap>,
        #[template_child]
        pub language_chooser: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub record_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub settings_btn: TemplateChild<gtk::Button>,
        #[template_child]
        pub device_drop_down: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub transcriber_view: TemplateChild<gtk::Box>,
        #[template_child]
        pub model_chooser_view: TemplateChild<gtk::Box>,
        pub transcriber: RefCell<Option<TranscriberActor>>,
        pub models_repo: RefCell<Option<ModelsRepo>>,
        pub active_model: RefCell<Option<RemoteModel>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrascriWindow {
        const NAME: &'static str = "TrascriWindow";
        type Type = super::TrascriWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for TrascriWindow {}
    impl WidgetImpl for TrascriWindow {}
    impl WindowImpl for TrascriWindow {}
    impl ApplicationWindowImpl for TrascriWindow {}
    impl AdwApplicationWindowImpl for TrascriWindow {}
}

glib::wrapper! {
    pub struct TrascriWindow(ObjectSubclass<imp::TrascriWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,        @implements gio::ActionGroup, gio::ActionMap;
}

#[gtk::template_callbacks]
impl TrascriWindow {
    fn setup_language_chooser(&self, path: PathBuf) {
        let imp = self.imp();
        imp.models_repo.replace(Some(ModelsRepo::from_path(path.clone())));

        let Some(ref models_repo) = *imp.models_repo.borrow() else {
            unreachable!("it's set above");
        };

        let b = gtk::Box::new(gtk::Orientation::Vertical, 16);
        let ls = gtk::ListBox::new();
        ls.add_css_class("boxed-list");
        ls.set_selection_mode(gtk::SelectionMode::None);

        for lang in ModelsRepo::models_iter() {
            let row = adw::ActionRow::builder()
                .title(&lang.name)
                .activatable(true)
                .build();

            let btn_download = gtk::Button::builder()
                .icon_name("folder-download-symbolic")
                .valign(gtk::Align::Center)
                .build();

            let btn_remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .valign(gtk::Align::Center)
                .build();

            let btn_use = gtk::Button::builder()
                .css_classes(vec!["suggested-action".to_owned()])
                .label("Use")
                .valign(gtk::Align::Center)
                .build();

            row.add_suffix(&btn_remove);
            row.add_suffix(&btn_use);
            row.add_suffix(&btn_download);

            let show_as_exists = Rc::new(
                clone!(@weak btn_remove, @weak btn_use, @weak btn_download => move |exists| {
                    btn_remove.set_visible(exists);
                    btn_use.set_visible(exists);
                    btn_download.set_visible(!exists);
                }),
            );

            show_as_exists(models_repo.is_downloaded(&lang));

            btn_download.connect_clicked({
                let fc = show_as_exists.clone();
                let lang = lang.clone();
                let models_repo = models_repo.clone();
                move |_| {
                    let fc = fc.clone();
                    let models_repo = models_repo.clone();
                    models_repo.download(&lang, move || {
                        fc(true);
                    });
                }
            });
            btn_remove.connect_clicked({
                let models_repo = models_repo.clone();
                let lang = lang.clone();
                move |_| {
                    models_repo.remove(&lang).unwrap();
                    show_as_exists(false);
                }
            });
            btn_use.connect_clicked({
                let obj = self.clone();

                move |_| {
                    let imp = obj.imp();
                    imp.active_model.replace(Some(lang.clone()));
                    imp.stack.set_visible_child(&*imp.transcriber_view);
                    obj.setup_transcriber();
                }
            });

            ls.append(&row);
        }

        b.append(&ls);

        let show_folder_btn = gtk::Button::builder()
            .label("Show models folder")
            .css_classes(vec!["suggested-action".into(), "pill".into()])
            .halign(gtk::Align::Center)
            .build();

        show_folder_btn.connect_clicked(move |_| {
            gtk::show_uri(
                None::<&gtk::Window>,
                &format!("file://{}", path.to_str().unwrap()),
                0,
            );
        });
        b.append(&show_folder_btn);
        imp.language_chooser.set_child(Some(&b));
    }
    #[template_callback]
    fn open_model_chooser(&self) {
        let imp = self.imp();
        imp.stack.set_visible_child(&*imp.model_chooser_view);
    }
    fn handle_transcriber_msg(&self, msg: Msg) {
        let imp = self.imp();

        let text_view = self.imp().text_view.clone();
        let b = text_view.buffer();
        match msg {
            Msg::PartialResult(s) => {
                let mut i = b.end_iter();
                b.insert(&mut i, &s);
                b.insert(&mut i, " ");
            },
            Msg::Result(s) => {
                let mut i = b.end_iter();
                b.insert(&mut i, &s);
                b.insert(&mut i, " ");
            },
            Msg::Started => {
                imp.record_btn.remove_css_class("suggested-action");
                imp.record_btn.add_css_class("destructive-action");
                imp.record_btn.set_label("Stop");
            },
            Msg::Stopped => {
                imp.record_btn.remove_css_class("destructive-action");
                imp.record_btn.add_css_class("suggested-action");
                imp.record_btn.set_label("Start");
            }
        }
    }
    fn setup_transcriber(&self) {
        let imp = self.imp();
        if let Some(ref transcriber) = *imp.transcriber.borrow() {
            transcriber.stop();
        }


        let (Some(ref active_model), Some(ref models_repo)) = (
            &*imp.active_model.borrow(),
            &*imp.models_repo.borrow(),
        ) else {
            return;
        };
        let recognizer = crate::adapters::recognizer::vosk::Vosk::new(
            models_repo.model_path(active_model).as_path(),
            SAMPLE_RATE as f32,
        );
        let obj = self.clone();
        imp.transcriber.replace(Some(TranscriberActor::new(
            Box::new(recognizer),
            move |msg| {
                obj.handle_transcriber_msg(msg);
            },
        )));
    }
    fn setup_drop_down(&self) {
        let imp = self.imp();
        let drop_down = imp.device_drop_down.clone();
        let item_factory = gtk::SignalListItemFactory::new();
        item_factory.connect_setup(|_, list_item| {
            let list_item: &gtk::ListItem = list_item.downcast_ref().unwrap();
            list_item.set_child(Some(
                &gtk::Label::builder()
                    .ellipsize(gtk::pango::EllipsizeMode::Middle)
                    .build(),
            ));
        });
        item_factory.connect_bind(|_, list_item| {
            let Some(list_item) = list_item.downcast_ref::<gtk::ListItem>() else {
                panic!("can't get list_item");
            };
            let (Some(Ok(label)), Some(Ok(item))) = (
                list_item.child().map(|x| x.downcast::<gtk::Label>()),
                list_item.item().map(|x| x.downcast::<gst::Device>())
            ) else {
                panic!("can't get item inside list_item");
            };
            label.set_label(&item.display_name());
        });
        drop_down.set_factory(Some(&item_factory));

        let obj = self.clone();
        drop_down.connect_selected_notify(move |dd| {
            let imp = obj.imp();
            let Some(device) = dd.selected_item() else {
                return;
            };
            let device: gst::Device = device.downcast().unwrap();
            let audio_src = crate::adapters::audio_src::pulse::Pulse::from(device);
            if let Some(ref transcriber) = *imp.transcriber.borrow() {
                transcriber.set_element(audio_src.make_element());
            } else {
                println!("transcriber not ready, input element not changed");
            }
        });

        drop_down.set_model(Some(
            &crate::adapters::audio_src::pulse::Pulse::list_available(),
        ));
    }
    #[template_callback]
    fn handle_record_btn_clicked(&self) {
        let imp = self.imp();
        let Some(ref transcriber) = &*imp.transcriber.borrow() else {
            return;
        };
        if transcriber.state() == gst::State::Playing {
            transcriber.stop();
        } else {
            transcriber.start();
        }
    }
    #[template_callback]
    fn handle_settings_btn_clicked(&self) {
        let imp = self.imp();
        imp.flap.set_reveal_flap(!imp.flap.reveals_flap());
    }
    pub fn new<P: glib::IsA<gtk::Application>>(application: &P) -> Self {
        let obj = glib::Object::builder::<Self>()
            .property("application", application)
            .build();

        obj.setup_language_chooser(glib::user_data_dir().join("models"));
        obj.setup_drop_down();
        obj.setup_transcriber();
        obj
    }
}
