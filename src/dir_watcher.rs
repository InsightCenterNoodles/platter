use std::fs;
use std::path::PathBuf;

use crate::{arguments::Directory, playground_state::PlatterCommand};
use colabrodo_server::server::tokio;
use notify::event::CreateKind;
use notify::EventKind;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use tokio::sync::mpsc;

struct FilePathRing {
    buffs: std::collections::VecDeque<PathBuf>,
    source_id: uuid::Uuid,
    max_size: usize,
}

impl FilePathRing {
    async fn send(&self, p: PathBuf, tx: &mpsc::Sender<PlatterCommand>) {
        tx.send(PlatterCommand::LoadFile(p.clone(), Some(self.source_id)))
            .await
            .unwrap();
    }

    async fn add(&mut self, p: PathBuf, tx: &mpsc::Sender<PlatterCommand>) {
        if self.max_size == 0 {
            self.send(p, tx).await;
            return;
        }

        self.buffs.push_back(p);

        while self.buffs.len() >= self.max_size {
            let p = self.buffs.pop_front().unwrap();

            self.send(p, tx).await;
        }
    }
}

pub async fn launch_file_watcher(
    tx: mpsc::Sender<PlatterCommand>,
    source_id: uuid::Uuid,
    dir: Directory,
) {
    log::info!("Watching directory {}", dir.dir.display());

    let mut path_ring = FilePathRing {
        buffs: Default::default(),
        source_id,
        max_size: if dir.latest_only { 1 } else { 0 },
    };

    let (mut watcher, mut rx) = setup_watcher().unwrap();

    if dir.load_existing {
        let paths = fs::read_dir(&dir.dir).unwrap();

        for path in paths {
            path_ring.add(path.unwrap().path(), &tx).await;
        }
    }

    watcher
        .watch(dir.dir.as_path(), RecursiveMode::Recursive)
        .unwrap();

    while let Some(msg) = rx.recv().await {
        if let Ok(event) = msg {
            log::debug!("Filesystem change: {event:?}");
            if let EventKind::Create(CreateKind::File) = event.kind {
                for p in event.paths {
                    log::info!("New file detected: {}", p.display());
                    path_ring.add(p, &tx).await;
                }
            }
        }
    }
}

fn setup_watcher() -> notify::Result<(RecommendedWatcher, mpsc::Receiver<notify::Result<Event>>)> {
    let (send_from_watcher, recv_from_watcher) = mpsc::channel(16);

    let watcher = RecommendedWatcher::new(
        move |result| send_from_watcher.blocking_send(result).unwrap(),
        Config::default(),
    )?;

    Ok((watcher, recv_from_watcher))
}
