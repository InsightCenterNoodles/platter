mod arguments;
mod dir_watcher;
pub mod import;
pub mod import_gltf;
pub mod import_obj;
mod methods;
mod platter_state;
mod scene;

use colabrodo_common::network::default_server_address;
use colabrodo_server::server::{server_main, tokio, ServerOptions};
use colabrodo_server::server_http::*;
use colabrodo_server::server_state::ServerState;
use platter_state::PlatterState;
use platter_state::PlatterStatePtr;
use platter_state::{handle_command, PlatterCommand};
use std::env;

async fn command_handler(
    ps: PlatterStatePtr,
    mut command_stream: tokio::sync::mpsc::Receiver<PlatterCommand>,
) {
    while let Some(msg) = command_stream.recv().await {
        handle_command(ps.clone(), msg);
    }
}

fn mdns_publish(port: u16) -> mdns_sd::ServiceDaemon {
    let mdns = mdns_sd::ServiceDaemon::new().expect("unable to create mdns daemon");

    const SERVICE_TYPE: &'static str = "_noodles._tcp.local.";
    const INSTANCE_NAME: &'static str = "platter";

    if let Ok(nif) = local_ip_address::list_afinet_netifas() {
        for (_, ip) in nif.iter().filter(|f| f.1.is_ipv4()) {
            let ip_str = ip.to_string();
            let host = format!("{}.local.", ip);

            if ip.to_string().contains("10.15.88") {
                continue;
            }

            let srv_info =
                mdns_sd::ServiceInfo::new(SERVICE_TYPE, INSTANCE_NAME, &host, ip_str, port, None)
                    .expect("unable to  build MDNS service information");

            log::info!("registering MDNS SD on {}", ip);

            if mdns.register(srv_info).is_err() {
                log::warn!("unable to register MDNS SD for {}", ip);
            }
        }
    }

    mdns
}

#[tokio::main]
async fn main() {
    if env::var("RUST_LOG").is_err() {
        env::set_var("RUST_LOG", "info")
    }
    env_logger::init();

    let args = arguments::get_arguments();

    // Set up options for the noodles server

    let mut host = args.address.unwrap_or_else(default_server_address);

    if let Some(port) = args.port {
        host.set_port(Some(port)).unwrap();
    }

    let opts = ServerOptions { host };

    // Prep asset server
    let asset_server = make_asset_server(AssetServerOptions::new(&opts));

    // Prep command streams
    let (command_tx, command_rx) = tokio::sync::mpsc::channel(16);

    let (stop_tx, _) = tokio::sync::broadcast::channel(1);

    // Prep streams for the watcher controller
    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::unbounded_channel();

    let offset = args.offset.map(|f| {
        let mut iter = f.split(",").map(|g| g.trim().parse().unwrap());
        nalgebra_glm::Vec3::new(
            iter.next().unwrap_or_default(),
            iter.next().unwrap_or_default(),
            iter.next().unwrap_or_default(),
        )
    });

    let init = platter_state::PlatterInit {
        command_stream: command_tx.clone(),
        watcher_command_stream: watcher_tx,
        asset_store: asset_server.clone(),
        size_large_limit: args.size_large_limit,
        resize: args.rescale.unwrap_or(1.0),
        offset: offset.unwrap_or_default(),
    };

    // take a copy of the command sender to move into the watcher command task
    let spawner_tx_clone = command_tx.clone();

    // start up a command task for the watcher: this will spawn new dir watchers upon request.
    tokio::spawn(async move {
        while let Some(msg) = watcher_rx.recv().await {
            tokio::spawn(dir_watcher::launch_file_watcher(
                spawner_tx_clone.clone(),
                msg,
                stop_tx.subscribe(),
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

    let server_state = ServerState::new();

    let platter_state = PlatterState::new(server_state.clone(), init);

    tokio::spawn(command_handler(platter_state, command_rx));

    log::info!("Starting up.");

    let mdns = mdns_publish(opts.host.port().unwrap());

    // Launch the main noodles task and wait for it to complete
    server_main(opts, server_state).await;

    mdns.shutdown().unwrap();
}
