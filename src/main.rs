mod arguments;
mod dir_watcher;
mod import;
mod object;
mod platter_state;

use colabrodo_server::server::{server_main_with_command_queue, tokio, ServerOptions};
use colabrodo_server::server_http::*;
use log::{self, info};
use std::env;

#[tokio::main]
async fn main() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let args = arguments::get_arguments();

    // Set up options for the noodles server
    let opts = ServerOptions {
        host: format!("{}:{}", args.address, args.port),
    };

    // Prep asset server
    let (asset_server, mut link) = make_asset_server(AssetServerOptions::default());

    // Launch it
    tokio::spawn(asset_server);

    // Wait for it to start
    link.wait_for_start().await;

    // Prep command streams
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

    // Prep streams for the watcher controller
    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(16);

    let init = platter_state::PlaygroundInit {
        watcher_command_stream: watcher_tx,
        link,
        size_large_limit: args.size_large_limit,
    };

    // take a copy of the command sender to move into the watcher command task
    let spawner_tx_clone = command_tx.clone();

    // start up a command task for the watcher: this will spawn new dir watchers upon request.
    tokio::spawn(async move {
        while let Some(msg) = watcher_rx.recv().await {
            tokio::spawn(dir_watcher::launch_file_watcher(
                spawner_tx_clone.clone(),
                msg.1,
                msg.0,
            ));
        }
    });

    // Based on args, insert an initial command into the command stream
    match args.source {
        arguments::Source::File { ref name } => {
            if !name.try_exists().unwrap() {
                log::error!("File {} is not readable.", name.display());
                panic!("Unable to continue");
            }

            command_tx
                .send(platter_state::PlatterCommand::LoadFile(name.clone(), None))
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
                .send(platter_state::PlatterCommand::WatchDirectory(dir.clone()))
                .await
                .unwrap();
        }

        arguments::Source::Websocket { port: _ } => todo!(),
    }

    info!("Starting up.");

    // Launch the main noodles task and wait for it to complete
    server_main_with_command_queue::<platter_state::PlatterState>(opts, init, Some(command_rx))
        .await;
}
