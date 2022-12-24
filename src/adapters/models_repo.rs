use std::io::prelude::*;
use std::rc::Rc;
use std::sync::Arc;
use std::{fs, path};

use anyhow::Context;
use cap_std::fs as cap_fs;
use gtk::glib;
use serde::{Deserialize, Serialize};

const MODELS_DEF: &str = include_str!("../../data/models.json");

#[derive(Clone)]
pub struct ModelsRepo {
    dir: Arc<cap_fs::Dir>,
    path: path::PathBuf,
    thread_pool: Rc<glib::ThreadPool>,
}

#[derive(Serialize, Deserialize)]
struct Models {
    models: Vec<RemoteModel>,
}
#[derive(Serialize, Deserialize, Clone)]
pub struct RemoteModel {
    pub name: String,
    pub url: String,
}

impl ModelsRepo {
    pub fn from_path(path: path::PathBuf) -> Self {
        fs::DirBuilder::new().recursive(true).create(&path).unwrap();
        let dir = cap_fs::Dir::open_ambient_dir(&path, cap_std::ambient_authority()).unwrap();

        Self {
            path,
            dir: Arc::new(dir),
            thread_pool: Rc::new(glib::ThreadPool::shared(None).unwrap()),
        }
    }
    pub fn model_path(&self, m: &RemoteModel) -> path::PathBuf {
        self.path.join(&m.name)
    }
    pub fn models_iter() -> impl Iterator<Item = RemoteModel> {
        let models: Models = serde_json::from_str(MODELS_DEF).unwrap();
        models.models.into_iter()
    }
    pub fn is_downloaded(&self, m: &RemoteModel) -> bool {
        self.dir.exists(&m.name)
    }
    pub fn remove(&self, m: &RemoteModel) -> anyhow::Result<()> {
        self.dir.remove_dir_all(&m.name)?;
        Ok(())
    }

    // uses cap_std to ensure the extraction is safe
    fn extract(source: cap_std::fs::File, dir: cap_std::fs::Dir) -> anyhow::Result<()> {
        let mut zip = zip::ZipArchive::new(source)?;

        let prefix = {
            let prefix_folder = zip.by_index(0)?;
            prefix_folder.name().to_owned()
        };

        // The first file is skipped, because it's the containing folder,
        // but we already have a containing folder, it's the parameter `dir`.
        for i in 1..zip.len() {
            let mut zf = zip.by_index(i)?;
            let stripped_name = zf
                .name()
                .strip_prefix(&prefix)
                .context("stripping root folder path prefix")?;
            if zf.is_dir() {
                dir.create_dir(stripped_name).unwrap();
            } else {
                let mut f = dir.open_with(
                    stripped_name,
                    cap_std::fs::OpenOptions::new().write(true).create_new(true),
                )?;
                std::io::copy(&mut zf, &mut f)?;
            }
        }
        Ok(())
    }
    pub fn download(&self, m: &RemoteModel, cb: impl Fn() + 'static) {
        let (s, r) = glib::MainContext::channel(glib::PRIORITY_LOW);

        let url = m.url.to_owned();
        let tmpf_path = format!("{}.tmp", &m.name);
        let mut tmpf = self
            .dir
            .open_with(
                &tmpf_path,
                cap_std::fs::OpenOptions::new()
                    .write(true)
                    .read(true)
                    .truncate(true)
                    .create(true),
            )
            .unwrap();
        let dir = self.dir.clone();
        let name = m.name.to_owned();
        self.thread_pool
            .push(move || {
                {
                    let body = ureq::get(&url).call().unwrap();
                    let mut reader = body.into_reader();
                    std::io::copy(&mut reader, &mut tmpf).unwrap();
                }

                tmpf.seek(std::io::SeekFrom::Start(0)).unwrap();

                if !dir.exists(&name) {
                    dir.create_dir(&name).unwrap();
                }
                let dest_dir = dir.open_dir(&name).unwrap();
                Self::extract(tmpf, dest_dir)
                    .context("extracting downloaded model")
                    .unwrap();
                dir.remove_file(&tmpf_path).unwrap();

                s.send(()).unwrap();
            })
            .unwrap();

        r.attach(None, move |_| {
            cb();
            glib::Continue(false)
        });
    }
}
