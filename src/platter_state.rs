use crate::arguments;
use crate::arguments::Directory;
use crate::import;
use crate::methods::setup_methods;
use crate::scene::Scene;

use anyhow::Result;

#[cfg(use_assimp)]
use crate::assimp_import;

use colabrodo_server::server::*;
use colabrodo_server::server_http::*;
use colabrodo_server::server_messages::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::{collections::HashMap, path::Path};

/// Initization info for our platter server
pub struct PlatterInit {
    /// Stream for commands
    pub command_stream: tokio::sync::mpsc::Sender<PlatterCommand>,

    /// Stream for commands from the directory watcher
    pub watcher_command_stream: tokio::sync::mpsc::Sender<(Directory, uuid::Uuid)>,

    /// Where to store large assets
    pub asset_store: AssetStorePtr,

    /// What constitutes a 'large' buffer. Buffers smaller than this will be
    /// possibly sent inline
    pub size_large_limit: u64,
}

/// Our server state
pub struct PlatterState {
    /// Initial options
    init: PlatterInit,

    /// NOODLES server
    state: ServerStatePtr,

    /// Application specific methods
    methods: Vec<MethodReference>,

    /// Each file roughly maps to a scene. Each Scene gets an ID.
    items: HashMap<u32, Scene>,

    /// We attach some methods to entities; this maps entities to scenes
    root_to_item: HashMap<EntityReference, u32>,

    /// The next Scene ID to use. Just a monotonic counter
    next_item_id: u32,

    /// Tag UUID to Scene to identify scenes derived from a single source
    source_map: HashMap<uuid::Uuid, HashSet<u32>>,

    /// A map that says if scenes from a tag are to be exclusive; that only one scene with a tag can exist
    source_exclusive: HashSet<uuid::Uuid>,
}

pub type PlatterStatePtr = Arc<std::sync::Mutex<PlatterState>>;

/// An instruction to platter
#[derive(Debug)]
pub enum PlatterCommand {
    /// Load a file from disk, with an optional tag
    LoadFile(PathBuf, Option<uuid::Uuid>),
    /// Start watching a directory
    WatchDirectory(arguments::Directory),
}

impl PlatterState {
    /// Create new platter state
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

    /// Obtain the next scene ID
    fn get_next_scene_id(&mut self) -> u32 {
        let ret = self.next_item_id;
        self.next_item_id += 1;
        ret
    }

    /// An order to import a filesystem item. This could be a directory or a file
    fn import_filesystem_item(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        if p.is_dir() {
            self.import_dir(p, source);
        } else if p.is_file() {
            self.import_file(p, source);
        }
    }

    /// Import a specific file.
    fn import_file(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        log::info!("Loading file: {}", p.display());
        let res = match handle_import(p, self.state.clone(), self.init.asset_store.clone()) {
            Ok(x) => x,
            Err(x) => {
                log::error!("Error loading file: {x:?}");
                return;
            }
        };

        self.add_object(res, source);
    }

    /// Import a directory.
    ///
    /// Searches through the directory and tries to load every file encountered.
    fn import_dir(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        let paths = fs::read_dir(p).unwrap();

        for path in paths {
            self.import_file(path.unwrap().path().as_path(), source);
        }
    }

    /// Add an object scene to the state
    fn add_object(&mut self, o: Scene, source: Option<uuid::Uuid>) -> u32 {
        let id = self.get_next_scene_id();

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

    /// Remove an object scene from the state
    fn remove_object(&mut self, id: u32) {
        let ent = self.items.get(&id).unwrap().root.parts.first().unwrap();

        self.root_to_item.remove(ent);

        self.items.remove(&id);
    }

    /// Clear all objects with the same source tag
    fn clear_source(&mut self, source: uuid::Uuid) -> Option<()> {
        let list = self.source_map.remove(&source)?;

        for item in list.iter() {
            self.remove_object(*item);
        }

        Some(())
    }

    /// Given an entity reference, get the object scene it belongs to
    pub fn find_id(&self, ent: &EntityReference) -> Option<u32> {
        self.root_to_item.get(ent).copied()
    }

    /// Given an object scene id, get the scene object to mutuate
    pub fn get_object_mut(&mut self, id: u32) -> Option<&mut Scene> {
        self.items.get_mut(&id)
    }
}

/// Handle a command and mutate the platter state
pub fn handle_command(platter_state: PlatterStatePtr, c: PlatterCommand) {
    let mut this = platter_state.lock().unwrap();

    match c {
        PlatterCommand::LoadFile(f, s_id) => {
            this.import_filesystem_item(f.as_path(), s_id);
        }
        PlatterCommand::WatchDirectory(dir) => {
            if !dir.dir.try_exists().unwrap() {
                log::error!("Directory {} is not readable.", dir.dir.display());
                return;
            }

            let s_id = get_next_source_id();

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

/// Dispatch a request to import. Depending on options this will either use builtin import tools or use assimp.
fn handle_import(path: &Path, state: ServerStatePtr, asset_store: AssetStorePtr) -> Result<Scene> {
    #[cfg(use_assimp)]
    return assimp_import::import_file(p);

    #[cfg(not(use_assimp))]
    return import::import_file(path, state, asset_store);
}

/// Get a new tag
fn get_next_source_id() -> uuid::Uuid {
    uuid::Uuid::new_v4()
}
