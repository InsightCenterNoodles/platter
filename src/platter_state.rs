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
    pub watcher_command_stream: tokio::sync::mpsc::UnboundedSender<Directory>,

    /// Where to store large assets
    pub asset_store: AssetStorePtr,

    /// What constitutes a 'large' buffer. Buffers smaller than this will be
    /// possibly sent inline
    pub size_large_limit: u64,

    /// User asks to rescale using this factor
    pub resize: f32,

    /// User asks to translate
    pub offset: nalgebra_glm::Vec3,
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
    source_map: HashMap<Tag, HashSet<u32>>,
}

pub type PlatterStatePtr = Arc<std::sync::Mutex<PlatterState>>;

#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy)]
pub struct Tag(uuid::Uuid);

impl Tag {
    pub fn new() -> Tag {
        Tag(uuid::Uuid::new_v4())
    }
}

/// An instruction to platter
#[derive(Debug)]
pub enum PlatterCommand {
    /// Load a file from disk, with an optional tag
    LoadFile(PathBuf, Option<Tag>),
    /// Start watching a directory
    WatchDirectory(arguments::Directory),
    /// Clear a tag
    ClearTag(Tag),
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
    fn import_filesystem_item(&mut self, p: &Path, source: Option<Tag>) {
        if p.is_dir() {
            self.import_dir(p, source);
        } else if p.is_file() {
            self.import_file(p, source);
        }
    }

    /// Import a specific file.
    fn import_file(&mut self, p: &Path, source: Option<Tag>) {
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
    fn import_dir(&mut self, p: &Path, source: Option<Tag>) {
        let paths = fs::read_dir(p).unwrap();

        for path in paths {
            self.import_file(path.unwrap().path().as_path(), source);
        }
    }

    /// Add an object scene to the state
    fn add_object(&mut self, o: Scene, source: Option<Tag>) -> u32 {
        let id = self.get_next_scene_id();

        let ent = o.root.parts.first().unwrap().clone();

        self.root_to_item.insert(ent.clone(), id);

        if false {
            let offset = self.init.offset;
            let offset = nalgebra_glm::translation(&offset);

            let rescale = self.init.resize;
            let rescale = nalgebra_glm::vec3(rescale, rescale, rescale);
            let rescale = nalgebra_glm::scale(&offset, &rescale);

            let rescale: [f32; 16] = rescale.as_slice().try_into().unwrap();

            log::debug!("Resetting scale tf: {rescale:?}");

            ServerEntityStateUpdatable {
                methods_list: Some(self.methods.clone()),
                transform: Some(rescale),
                ..Default::default()
            }
            .patch(&ent);
        }

        self.items.insert(id, o);

        if let Some(sid) = source {
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
    fn clear_source(&mut self, source: Tag) -> Option<()> {
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

            this.init.watcher_command_stream.send(dir).unwrap();
        }
        PlatterCommand::ClearTag(tag) => {
            this.clear_source(tag);
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
