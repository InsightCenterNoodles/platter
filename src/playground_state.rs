use crate::arguments;
use crate::import::{ImportedScene, Object};

use colabrodo_core::server::AsyncServer;
use colabrodo_core::{
    server_messages::MethodException,
    server_state::{ServerState, UserServerState},
};
use std::fs;
use std::path::PathBuf;
use std::{collections::HashMap, path::Path};

pub struct PlaygroundState {
    args: arguments::Arguments,
    state: ServerState,

    items: HashMap<u32, Object>,
    next_item_id: u32,
}

#[derive(Debug)]
pub enum ServerCommand {
    LoadFile(PathBuf),
}

impl AsyncServer for PlaygroundState {
    type CommandType = ServerCommand;

    fn new(tx: colabrodo_core::server_state::CallbackPtr) -> Self {
        Self {
            args: arguments::get_arguments(),
            state: ServerState::new(tx),
            items: Default::default(),
            next_item_id: 0,
        }
    }

    fn initialize_state(&mut self) {
        match self.args.source.clone() {
            arguments::Source::File { name } => self.add_filesystem_item(name.as_path()),
            arguments::Source::Directory(d) if d.load_existing => self.add_dir(d.dir.as_path()),
            arguments::Source::Websocket { port: _ } => todo!(),
            _ => (),
        }
    }

    fn handle_command(&mut self, c: Self::CommandType) {
        match c {
            ServerCommand::LoadFile(f) => {
                // do we need to clear anything first?
                match &self.args.source {
                    arguments::Source::Directory(d) if d.latest_only => self.clear(),
                    _ => (),
                }

                self.add_file(f.as_path())
            }
        }
    }
}

impl UserServerState for PlaygroundState {
    fn mut_state(&mut self) -> &ServerState {
        &self.state
    }

    fn state(&self) -> &ServerState {
        &self.state
    }

    fn invoke(
        &mut self,
        _method: colabrodo_core::server_messages::ComponentReference<
            colabrodo_core::server_messages::MethodState,
        >,
        _context: colabrodo_core::server_state::InvokeObj,
        _args: Vec<colabrodo_core::client::ciborium::value::Value>,
    ) -> colabrodo_core::server_state::MethodResult {
        Err(MethodException::method_not_found(Some(
            "No methods implemented yet".to_string(),
        )))
    }
}

impl PlaygroundState {
    fn get_next_id(&mut self) -> u32 {
        let ret = self.next_item_id;
        self.next_item_id += 1;
        ret
    }

    fn add_object(&mut self, o: Object) -> u32 {
        let id = self.get_next_id();

        self.items.insert(id, o);

        id
    }

    fn add_filesystem_item(&mut self, p: &Path) {
        if p.is_dir() {
            self.add_dir(p);
        } else if p.is_file() {
            self.add_file(p);
        }
    }

    fn add_file(&mut self, p: &Path) {
        log::info!("Loading file: {}", p.display());
        let res = ImportedScene::import_file(p);

        match res {
            Ok(x) => {
                let o = x.build_objects(&mut self.state);
                self.add_object(o);
            }
            Err(e) => {
                log::error!("Error loading file: {e:?}");
            }
        }
    }

    fn add_dir(&mut self, p: &Path) {
        let paths = fs::read_dir(p).unwrap();

        for path in paths {
            self.add_file(path.unwrap().path().as_path());
        }
    }

    fn clear(&mut self) {
        self.items.clear();
    }
}
