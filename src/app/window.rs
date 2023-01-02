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

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::clone;
use gst::prelude::DeviceExt;
use gtk::{gdk, gio, glib, CompositeTemplate};
use postage::mpsc;
use postage::prelude::Stream;

use crate::adapters::models_repo::{ModelsRepo, RemoteModel};
use crate::app::transcriber::*;
use crate::ports::*;

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
        pub bottom_bar: TemplateChild<gtk::Box>,
        #[template_child]
        pub scrolled_win: TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub subtitle_mode_view: TemplateChild<gtk::Overlay>,
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
        #[template_child]
        pub rms: TemplateChild<gtk::Label>,
        pub transcriber: RefCell<Option<TranscriberActor>>,
        pub models_repo: RefCell<Option<ModelsRepo>>,
        pub active_model: RefCell<Option<RemoteModel>>,
        pub last_result_iter: RefCell<Option<gtk::TextMark>>,
        pub scroll_animation: RefCell<adw::TimedAnimation>,
        pub recording: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for TrascriWindow {
        const NAME: &'static str = "TrascriWindow";
        type Type = super::TrascriWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
            klass.bind_template_instance_callbacks();

            klass.install_action(
                "win.activate-subtitle-mode",
                None,
                |win, _aname, _atarget| {
                    win.set_subtitle_mode(true);
                },
            );
            klass.install_action(
                "win.disable-subtitle-mode",
                None,
                |win, _aname, _atarget| {
                    win.set_subtitle_mode(false);
                },
            );
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
        imp.models_repo
            .replace(Some(ModelsRepo::from_path(path.clone())));

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
                    obj.handle_selected_input();
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

        let save_mark = || {
            imp.last_result_iter
                .replace(Some(b.create_mark(None, &mut b.end_iter(), true)))
        };

        let animate_to_bottom = move || {
            let vadj = imp.scrolled_win.vadjustment();
            let dx = vadj.upper() - vadj.value();
            let value_to = vadj.upper() - vadj.page_size();
            let should_update = {
                let anim = imp.scroll_animation.borrow();
                dx != 0.0
                    && anim.value_to() != value_to
                    && anim.state() != adw::AnimationState::Playing
            };
            if should_update {
                let animation = adw::builders::TimedAnimationBuilder::new()
                    .value_from(vadj.value())
                    .value_to(value_to)
                    .duration(400)
                    .easing(adw::Easing::EaseOutCubic)
                    .widget(self)
                    .target(&adw::PropertyAnimationTarget::new(&vadj, "value"))
                    .build();
                animation.play();
                imp.scroll_animation.replace(animation);
            }
        };
        match dbg!(msg) {
            Msg::PartialResult(s) => {
                if let Some(ref mut mark) = *imp.last_result_iter.borrow_mut() {
                    b.delete(&mut b.iter_at_mark(mark), &mut b.end_iter());
                }

                let mut i = b.end_iter();
                b.insert(&mut i, &s);
                b.insert(&mut i, " ");

                animate_to_bottom();
            }
            Msg::Result(s) => {
                if let Some(ref mut mark) = *imp.last_result_iter.borrow_mut() {
                    b.delete(&mut b.iter_at_mark(mark), &mut b.end_iter());
                }
                let mut i = b.end_iter();
                b.insert(&mut i, &s);
                b.insert(&mut i, " ");

                animate_to_bottom();

                save_mark();
            }
            Msg::Started => {
                imp.recording.replace(true);
                imp.record_btn.remove_css_class("suggested-action");
                imp.record_btn.add_css_class("destructive-action");
                imp.record_btn.set_label("Stop");
                save_mark();
            }
            Msg::Stopped => {
                imp.recording.replace(false);
                imp.record_btn.remove_css_class("destructive-action");
                imp.record_btn.add_css_class("suggested-action");
                imp.record_btn.set_label("Start");
            }
        }
    }
    fn setup_transcriber(&self) {
        let imp = self.imp();

        let (Some(ref active_model), Some(ref models_repo)) = (
            &*imp.active_model.borrow(),
            &*imp.models_repo.borrow(),
        ) else {
            return;
        };

        let path = models_repo.model_path(active_model).clone();

        let (s, mut r) = mpsc::channel::<f64>(10);
        let obj = self.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Some(msg) = dbg!(r.recv().await) {
                obj.imp().rms.set_label(&format!("{:.4}", msg));
            }
        });
        imp.transcriber.replace(Some(TranscriberActor::new(
            move || {
                let recognizer = crate::adapters::recognizer::vosk::Vosk::new(
                    path.as_path(),
                    SAMPLE_RATE as f32,
                );
                Box::new(recognizer)
            },
            s,
        )));
        dbg!("tra");
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

        drop_down.set_model(Some(
            &crate::adapters::audio_src::pulse::Pulse::list_available(),
        ));

        let obj = self.clone();
        drop_down.connect_selected_item_notify(move |_| {
            obj.handle_selected_input();
        });
        // Somehow this first selected item doesn't trigger the item_notify signal, maybe
        // it's because the selected item is already 0 by default? But checking dropdown.selected_item()
        // it's None...
        drop_down.set_selected(0);
        // Manually handle first selection.
        self.handle_selected_input();
    }
    fn handle_selected_input(&self) {
        let imp = self.imp();
        dbg!("OK");
        let Some(device) = imp.device_drop_down.selected_item() else {
            return;
        };
        let device: gst::Device = device.downcast().unwrap();
        let audio_src = crate::adapters::audio_src::pulse::Pulse::from(device);
        if let Some(ref transcriber) = *imp.transcriber.borrow() {
            transcriber.set_element(audio_src.make_element());
        } else {
            println!("transcriber not ready, input element not changed");
        };
    }
    fn set_subtitle_mode(&self, active: bool) {
        let imp = self.imp();
        imp.stack.set_vhomogeneous(!active);

        if active {
            imp.stack.set_visible_child(&*imp.subtitle_mode_view);
            imp.flap.set_content(None::<&gtk::Widget>);
            imp.scrolled_win
                .set_vscrollbar_policy(gtk::PolicyType::External);
            imp.subtitle_mode_view.set_child(Some(&*imp.scrolled_win));
            self.add_css_class("osd");
            self.add_css_class("subtitle-mode");
        } else {
            imp.stack.set_visible_child(self.main_view());
            imp.subtitle_mode_view.set_child(None::<&gtk::Widget>);
            imp.scrolled_win
                .set_vscrollbar_policy(gtk::PolicyType::Automatic);
            imp.flap.set_content(Some(&*imp.scrolled_win));
            self.remove_css_class("osd");
            self.remove_css_class("subtitle-mode");
        }
    }
    fn main_view(&self) -> &impl IsA<gtk::Widget> {
        let imp = self.imp();
        if imp.transcriber.borrow().is_none() {
            &*imp.model_chooser_view
        } else {
            &*imp.transcriber_view
        }
    }

    #[template_callback]
    fn handle_record_btn_clicked(&self) {
        let imp = self.imp();
        let Some(ref transcriber) = &*imp.transcriber.borrow() else {
            return;
        };

        if imp.recording.get() {
            transcriber.stop();
        } else {
            let obj = self.clone();
            let (s, mut r) = mpsc::channel(2);
            glib::MainContext::default().spawn_local(async move {
                while let Some(msg) = dbg!(r.recv().await) {
                    obj.handle_transcriber_msg(msg);
                }
            });
            transcriber.start(s);
        }
    }
    fn setup_css(&self) {
        let provider = gtk::CssProvider::new();
        provider.load_from_data(
            r"
window.subtitle-mode {
    border-radius: 8px;
}
.subtitle-mode textview {
    background: none;
    color: white;
    font-size: 2rem;
    font-weight: bold;
}
.subtitle-mode headerbar {
    background: none;
    color: white;
    box-shadow: none;
}
.subtitle-mode button {
  opacity: 0.6;
}
.subtitle-mode textview {
  padding-right: 24px;
}
.subtitle-mode:hover button {
  opacity: 1.0;
}
        "
            .as_bytes(),
        );
        gtk::StyleContext::add_provider_for_display(
            &gdk::Display::default().unwrap(),
            &provider,
            800,
        );
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

        obj.setup_css();
        obj.set_subtitle_mode(false);
        obj
    }
}
