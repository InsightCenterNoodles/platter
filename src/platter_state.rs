use crate::arguments;
use crate::arguments::Directory;
use crate::intermediate_to_noodles::*;
use crate::object::ObjectRoot;
use crate::scene_import;

use colabrodo_server::server::*;
use colabrodo_server::server_http::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, path::Path};

pub struct PlaygroundInit {
    pub command_stream: tokio::sync::mpsc::Sender<PlatterCommand>,
    pub watcher_command_stream: tokio::sync::mpsc::Sender<(Directory, uuid::Uuid)>,
    pub link: Arc<tokio::sync::Mutex<AssetServerLink>>,
    pub size_large_limit: u64,
}

pub struct PlatterState {
    init: PlaygroundInit,
    state: ServerStatePtr,

    items: HashMap<u32, ObjectRoot>,
    next_item_id: u32,

    source_map: HashMap<uuid::Uuid, HashSet<u32>>,
    source_exclusive: HashSet<uuid::Uuid>,
}

pub type PlatterStatePtr = Arc<tokio::sync::Mutex<PlatterState>>;

#[derive(Debug)]
pub enum PlatterCommand {
    LoadFile(PathBuf, Option<uuid::Uuid>),
    WatchDirectory(arguments::Directory),
}

impl PlatterState {
    pub fn new(state: ServerStatePtr, init: PlaygroundInit) -> Arc<tokio::sync::Mutex<Self>> {
        Arc::new(tokio::sync::Mutex::new(Self {
            init,
            state,
            items: Default::default(),
            next_item_id: 0,
            source_map: HashMap::new(),
            source_exclusive: HashSet::new(),
        }))
    }

    fn get_next_id(&mut self) -> u32 {
        let ret = self.next_item_id;
        self.next_item_id += 1;
        ret
    }

    fn get_next_source_id(&self) -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    async fn import_filesystem_item(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        if p.is_dir() {
            self.import_dir(p, source).await;
        } else if p.is_file() {
            self.import_file(p, source).await;
        }
    }

    async fn import_file(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        log::info!("Loading file: {}", p.display());
        let res = match scene_import::import_file(p) {
            Ok(x) => x,
            Err(x) => {
                log::error!("Error loading file: {x:?}");
                return;
            }
        };

        let meshes = convert_meshes(&res.meshes, self.init.link.clone()).await;
        let images = convert_images(&res.images, self.init.link.clone()).await;

        let root = convert_intermediate(images, meshes, res, self.state.clone());

        self.import_object(root, source).await;

        // move into binding
        // let limit = self.init.size_large_limit;
        // let link = self.init.link.clone();
        // let st = self.state.clone();
        // {
        //     let o = x.build_objects(limit, link, st).await;
        //     self.import_object(o, source).await;
        // }
    }

    async fn import_dir(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        let paths = fs::read_dir(p).unwrap();

        for path in paths {
            self.import_file(path.unwrap().path().as_path(), source)
                .await;
        }
    }

    fn _clear(&mut self) {
        self.items.clear();
    }

    async fn import_object(&mut self, o: ObjectRoot, source: Option<uuid::Uuid>) -> u32 {
        let id = self.get_next_id();

        self.items.insert(id, o);

        if let Some(sid) = source {
            // check if we have some exclusion
            if self.source_exclusive.contains(&sid) {
                self.clear_source(sid).await;
            }

            if let Some(list) = self.source_map.get_mut(&sid) {
                list.insert(id);
            }
        }

        id
    }

    async fn clear_source(&mut self, source: uuid::Uuid) -> Option<()> {
        let list = self.source_map.remove(&source)?;

        for item in list.iter() {
            if let Some(obj) = self.items.get(item) {
                obj.prepare_remove(self.init.link.clone()).await;
            }
            self.items.remove(item);
        }

        Some(())
    }
}

pub async fn handle_command(platter_state: PlatterStatePtr, c: PlatterCommand) {
    let mut this = platter_state.lock().await;

    match c {
        PlatterCommand::LoadFile(f, s_id) => {
            this.import_filesystem_item(f.as_path(), s_id).await;
        }
        PlatterCommand::WatchDirectory(dir) => {
            if !dir.dir.try_exists().unwrap() {
                log::error!("Directory {} is not readable.", dir.dir.display());
                panic!("Unable to continue");
            }

            let s_id = this.get_next_source_id();

            this.source_map.insert(s_id, HashSet::new());

            if dir.latest_only {
                this.source_exclusive.insert(s_id);
            }

            this.init
                .watcher_command_stream
                .blocking_send((dir, s_id))
                .unwrap();
        }
    }
}
