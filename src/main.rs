mod arguments;
mod dir_watcher;
mod import;
mod playground_state;

use colabrodo_core::server::{tokio, ServerOptions};
use log::{self, info};
use std::env;

#[tokio::main]
async fn main() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    // we have a silly thing here with double parsing args

    let args = arguments::get_arguments();

    let mut opts = ServerOptions::default();

    opts.host = format!("{}:{}", args.address, args.port);

    match args.source {
        arguments::Source::File { name } => {
            if !name.try_exists().unwrap() {
                log::error!("File {} is not readable.", name.display());
                panic!("Unable to continue");
            }

            // the server will parse again to get the args.
            // TODO: Figure out a better way to send startup info to the server
            colabrodo_core::server::server_main::<playground_state::PlaygroundState>(opts).await;
        }

        arguments::Source::Directory(dir) => {
            // early exit
            if !dir.dir.try_exists().unwrap() {
                log::error!("Directory {} is not readable.", dir.dir.display());
                panic!("Unable to continue");
            }

            log::info!("Watching directory {}", dir.dir.display());

            let (tx, rx) = tokio::sync::mpsc::channel(16);

            tokio::spawn(dir_watcher::file_watcher(tx, dir));

            colabrodo_core::server::server_main_with_command_queue::<
                playground_state::PlaygroundState,
            >(opts, Some(rx))
            .await;
        }

        arguments::Source::Websocket { port: _ } => todo!(),
    }

    info!("Starting up.");
}
