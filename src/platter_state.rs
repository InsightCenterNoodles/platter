use crate::arguments;
use crate::arguments::Directory;
use crate::import::{ImportedScene, Object};

use colabrodo_common::server_communication::*;
use colabrodo_server::server::ciborium;
use colabrodo_server::server::tokio;
use colabrodo_server::server::*;
use colabrodo_server::server_messages::*;
use colabrodo_server::server_state::*;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use std::{collections::HashMap, path::Path};

pub struct PlaygroundInit {
    pub watcher_command_stream: tokio::sync::mpsc::Sender<(Directory, uuid::Uuid)>,
}

pub struct PlatterState {
    init: PlaygroundInit,
    state: ServerState,

    items: HashMap<u32, Object>,
    next_item_id: u32,

    source_map: HashMap<uuid::Uuid, HashSet<u32>>,
    source_exclusive: HashSet<uuid::Uuid>,
}

#[derive(Debug)]
pub enum PlatterCommand {
    LoadFile(PathBuf, Option<uuid::Uuid>),
    WatchDirectory(arguments::Directory),
}

impl AsyncServer for PlatterState {
    type CommandType = PlatterCommand;
    type InitType = PlaygroundInit;

    fn new(tx: colabrodo_server::server_state::CallbackPtr, init: PlaygroundInit) -> Self {
        Self {
            init,
            state: ServerState::new(tx),
            items: Default::default(),
            next_item_id: 0,
            source_map: HashMap::new(),
            source_exclusive: HashSet::new(),
        }
    }

    fn initialize_state(&mut self) {
        // match self.init.args.source.clone() {
        //     arguments::Source::File { name } => self.add_filesystem_item(name.as_path()),
        //     arguments::Source::Directory(d) if d.load_existing => self.add_dir(d.dir.as_path()),
        //     arguments::Source::Websocket { port: _ } => todo!(),
        //     _ => (),
        // }
    }

    fn handle_command(&mut self, c: Self::CommandType) {
        match c {
            PlatterCommand::LoadFile(f, s_id) => {
                self.import_filesystem_item(f.as_path(), s_id);
            }
            PlatterCommand::WatchDirectory(dir) => {
                if !dir.dir.try_exists().unwrap() {
                    log::error!("Directory {} is not readable.", dir.dir.display());
                    panic!("Unable to continue");
                }

                let s_id = self.get_next_source_id();

                self.source_map.insert(s_id, HashSet::new());

                if dir.latest_only {
                    self.source_exclusive.insert(s_id);
                }

                self.init
                    .watcher_command_stream
                    .blocking_send((dir, s_id))
                    .unwrap();
            }
        }
    }
}

impl UserServerState for PlatterState {
    fn mut_state(&mut self) -> &ServerState {
        &self.state
    }

    fn state(&self) -> &ServerState {
        &self.state
    }

    fn invoke(
        &mut self,
        _method: ComponentReference<MethodState>,
        _context: InvokeObj,
        _args: Vec<ciborium::value::Value>,
    ) -> MethodResult {
        Err(MethodException::method_not_found(Some(
            "No methods implemented yet".to_string(),
        )))
    }
}

impl PlatterState {
    fn get_next_id(&mut self) -> u32 {
        let ret = self.next_item_id;
        self.next_item_id += 1;
        ret
    }

    fn get_next_source_id(&self) -> uuid::Uuid {
        uuid::Uuid::new_v4()
    }

    fn import_object(&mut self, o: Object, source: Option<uuid::Uuid>) -> u32 {
        let id = self.get_next_id();

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

    fn import_filesystem_item(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        if p.is_dir() {
            self.import_dir(p, source);
        } else if p.is_file() {
            self.import_file(p, source);
        }
    }

    fn import_file(&mut self, p: &Path, source: Option<uuid::Uuid>) {
        log::info!("Loading file: {}", p.display());
        let res = ImportedScene::import_file(p);

        match res {
            Ok(x) => {
                let o = x.build_objects(&mut self.state);
                self.import_object(o, source);
            }
            Err(e) => {
                log::error!("Error loading file: {e:?}");
            }
        }
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

    fn clear_source(&mut self, source: uuid::Uuid) {
        if let Some(list) = self.source_map.get_mut(&source) {
            for item in list.iter() {
                self.items.remove(item);
            }

            list.clear();
        }
    }
}
