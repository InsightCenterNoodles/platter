use crate::arguments;
use crate::arguments::Directory;
use crate::intermediate_to_noodles::*;
use crate::methods::setup_methods;
use crate::object::ObjectRoot;
use crate::scene_import;

use colabrodo_server::server::*;
use colabrodo_server::server_http::*;
use colabrodo_server::server_messages::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, path::Path};

pub struct PlatterInit {
    pub command_stream: tokio::sync::mpsc::Sender<PlatterCommand>,
    pub watcher_command_stream: tokio::sync::mpsc::Sender<(Directory, uuid::Uuid)>,
    pub link: AssetStorePtr,
    pub size_large_limit: u64,
}

pub struct PlatterState {
    init: PlatterInit,
    state: ServerStatePtr,

    methods: Vec<MethodReference>,

    items: HashMap<u32, ObjectRoot>,
    root_to_item: HashMap<EntityReference, u32>,
    next_item_id: u32,

    source_map: HashMap<uuid::Uuid, HashSet<u32>>,
    source_exclusive: HashSet<uuid::Uuid>,
}

pub type PlatterStatePtr = Arc<std::sync::Mutex<PlatterState>>;

#[derive(Debug)]
pub enum PlatterCommand {
    LoadFile(PathBuf, Option<uuid::Uuid>),
    WatchDirectory(arguments::Directory),
}

impl PlatterState {
    pub fn new(state: ServerStatePtr, init: PlatterInit) -> PlatterStatePtr {
        // awkwardness with the methods...

        let ret = Arc::new(std::sync::Mutex::new(Self {
            init,
            state: state.clone(),
            methods: Vec::new(),
            items: Default::default(),
            root_to_item: HashMap::new(),
            next_item_id: 0,
            source_map: HashMap::new(),
            source_exclusive: HashSet::new(),
        }));

        ret.lock().unwrap().methods = setup_methods(state, ret.clone());

        ret
    }

    fn get_next_id(&mut self) -> u32 {
        let ret = self.next_item_id;
        self.next_item_id += 1;
        ret
    }

    fn get_next_source_id(&self) -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn import_filesystem_item(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        if p.is_dir() {
            self.import_dir(p, source);
        } else if p.is_file() {
            self.import_file(p, source);
        }
    }

    fn import_file(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        log::info!("Loading file: {}", p.display());
        let res = match scene_import::import_file(p) {
            Ok(x) => x,
            Err(x) => {
                log::error!("Error loading file: {x:?}");
                return;
            }
        };

        let root = convert_intermediate(res, self.state.clone(), self.init.link.clone());

        self.import_object(root, source);
    }

    fn import_dir(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        let paths = fs::read_dir(p).unwrap();

        for path in paths {
            self.import_file(path.unwrap().path().as_path(), source);
        }
    }

    fn _clear(&mut self) {
        self.items.clear();
    }

    fn import_object(&mut self, o: ObjectRoot, source: Option<uuid::Uuid>) -> u32 {
        let id = self.get_next_id();

        let ent = o.root.parts.first().unwrap().clone();

        self.root_to_item.insert(ent.clone(), id);

        {
            ServerEntityStateUpdatable {
                methods_list: Some(self.methods.clone()),
                ..Default::default()
            }
            .patch(&ent);
        }

        self.items.insert(id, o);

        if let Some(sid) = source {
            // check if we have some exclusion
            if self.source_exclusive.contains(&sid) {
                self.clear_source(sid);
            }

            if let Some(list) = self.source_map.get_mut(&sid) {
                list.insert(id);
            }
        }

        id
    }

    fn remove_object(&mut self, id: u32) {
        let ent = self.items.get(&id).unwrap().root.parts.first().unwrap();

        self.root_to_item.remove(ent);

        self.items.remove(&id);
    }

    fn clear_source(&mut self, source: uuid::Uuid) -> Option<()> {
        let list = self.source_map.remove(&source)?;

        for item in list.iter() {
            self.remove_object(*item);
        }

        Some(())
    }

    pub fn find_id(&self, ent: &EntityReference) -> Option<u32> {
        self.root_to_item.get(ent).copied()
    }

    pub fn _get_object(&self, id: u32) -> Option<&ObjectRoot> {
        self.items.get(&id)
    }

    pub fn get_object_mut(&mut self, id: u32) -> Option<&mut ObjectRoot> {
        self.items.get_mut(&id)
    }
}

pub fn handle_command(platter_state: PlatterStatePtr, c: PlatterCommand) {
    let mut this = platter_state.lock().unwrap();

    match c {
        PlatterCommand::LoadFile(f, s_id) => {
            this.import_filesystem_item(f.as_path(), s_id);
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
