use crate::{arguments::Directory, playground_state::ServerCommand};
use colabrodo_core::server::tokio;
use notify::event::CreateKind;
use notify::EventKind;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use tokio::sync::mpsc;

pub async fn file_watcher(tx: mpsc::Sender<ServerCommand>, dir: Directory) {
    let (mut watcher, mut rx) = setup_watcher_thread().unwrap();

    watcher
        .watch(dir.dir.as_path(), RecursiveMode::Recursive)
        .unwrap();

    while let Some(msg) = rx.recv().await {
        if let Ok(event) = msg {
            log::debug!("Filesystem change: {event:?}");
            if let EventKind::Create(CreateKind::File) = event.kind {
                for p in event.paths {
                    log::info!("New file detected: {}", p.display());
                    tx.send(ServerCommand::LoadFile(p.clone())).await.unwrap();
                }
            }
        }
    }
}

// this should be a thread
fn setup_watcher_thread(
) -> notify::Result<(RecommendedWatcher, mpsc::Receiver<notify::Result<Event>>)> {
    let (send_from_watcher, recv_from_watcher) = mpsc::channel(16);

    let watcher = RecommendedWatcher::new(
        move |result| send_from_watcher.blocking_send(result).unwrap(),
        Config::default(),
    )?;

    Ok((watcher, recv_from_watcher))
}
