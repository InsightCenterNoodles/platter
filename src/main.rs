mod arguments;
mod dir_watcher;
mod import;
mod playground_state;

use colabrodo_server::server::{server_main_with_command_queue, tokio, ServerOptions};
use log::{self, info};
use std::env;

#[tokio::main]
async fn main() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let args = arguments::get_arguments();

    let opts = ServerOptions {
        host: format!("{}:{}", args.address, args.port),
    };

    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(16);

    let init = playground_state::PlaygroundInit {
        watcher_command_stream: watcher_tx,
    };

    let spawner_tx_clone = command_tx.clone();

    tokio::spawn(async move {
        while let Some(msg) = watcher_rx.recv().await {
            tokio::spawn(dir_watcher::launch_file_watcher(
                spawner_tx_clone.clone(),
                msg.1,
                msg.0,
            ));
        }
    });

    match args.source {
        arguments::Source::File { ref name } => {
            if !name.try_exists().unwrap() {
                log::error!("File {} is not readable.", name.display());
                panic!("Unable to continue");
            }

            command_tx
                .send(playground_state::PlatterCommand::LoadFile(
                    name.clone(),
                    None,
                ))
                .await
                .unwrap();
        }

        arguments::Source::Watch(ref dir) => {
            // early exit
            if !dir.dir.try_exists().unwrap() {
                log::error!("Directory {} is not readable.", dir.dir.display());
                panic!("Unable to continue");
            }

            command_tx
                .send(playground_state::PlatterCommand::WatchDirectory(
                    dir.clone(),
                ))
                .await
                .unwrap();
        }

        arguments::Source::Websocket { port: _ } => todo!(),
    }

    info!("Starting up.");

    server_main_with_command_queue::<playground_state::PlatterState>(opts, init, Some(command_rx))
        .await;
}
