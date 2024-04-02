//! Module to implement file and directory watching

use std::fs;
use std::path::PathBuf;

use crate::platter_state::Tag;
use crate::{arguments::Directory, platter_state::PlatterCommand};
use colabrodo_server::server::tokio;
use notify::event::AccessKind;
use notify::EventKind;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use tokio::sync::mpsc;

/// Create the file watcher loop
///
/// Takes a channel to send commands back to the platter system, an ID to mark
/// resources loaded from this watcher, and a directory to watch.
pub async fn launch_file_watcher(
    tx: mpsc::Sender<PlatterCommand>,
    dir: Directory,
    mut stopper: tokio::sync::broadcast::Receiver<bool>,
) {
    log::info!("Watching directory {}", dir.dir.display());

    let (mut watcher, mut rx) = setup_watcher().unwrap();

    let mut latest_dir = Option::<PathBuf>::default();
    let latest_tag = Tag::new();

    if dir.load_existing {
        load_existing(&dir, &tx, latest_tag).await;
    }

    watcher
        .watch(dir.dir.as_path(), RecursiveMode::Recursive)
        .unwrap();

    loop {
        tokio::select! {
                _ = stopper.recv() => {
                    let _ = watcher.unwatch(dir.dir.as_path());
                    return;
                }
                Some(msg) = rx.recv() => {
                    if let Ok(event) = msg {
                        log::debug!("Filesystem change: {event:?}");

                        match event.kind {
                            EventKind::Access(e) => match e {
                                AccessKind::Close(_) => {
                                    for p in event.paths {
                                        handle_file_closed(&tx, p, latest_tag, &dir, &latest_dir).await;
                                    }
                                }
                                _ => {}
                            },
                            EventKind::Create(e) => match e {
                                notify::event::CreateKind::File => {
                                    for p in event.paths {
                                        handle_file_created(&tx, p, latest_tag, &dir, &latest_dir).await;
                                    }
                                }
                                notify::event::CreateKind::Folder => {
                                    if dir.organize_by_dir && dir.latest_only {
                                        // clear all the old dirs
                                        tx.send(PlatterCommand::ClearTag(latest_tag)).await.unwrap();

                                        // use this new dir
                                        latest_dir = event.paths.into_iter().take(1).next();
                                    }
                                }
                                _ => {}
                            },
                            _ => {}
                        }
                    }
            }
        }
    }
}

async fn handle_file_closed(
    tx: &mpsc::Sender<PlatterCommand>,
    p: std::path::PathBuf,
    source_id: Tag,
    dir: &Directory,
    latest: &Option<PathBuf>,
) {
    handle_new_file(&tx, p, source_id, &dir, &latest).await;
}

async fn handle_file_created(
    tx: &mpsc::Sender<PlatterCommand>,
    p: std::path::PathBuf,
    source_id: Tag,
    dir: &Directory,
    latest: &Option<PathBuf>,
) {
    // For reasons on mac os x we do not see closes?
    #[cfg(target_os = "macos")]
    {
        handle_new_file(&tx, p, source_id, &dir, &latest).await;
    }
}

async fn handle_new_file(
    tx: &mpsc::Sender<PlatterCommand>,
    p: std::path::PathBuf,
    source_id: Tag,
    dir: &Directory,
    latest: &Option<PathBuf>,
) {
    log::info!("New file detected: {}", p.display());

    if dir.organize_by_dir {
        log::debug!("Organized by directory...");
        let Some(lp) = latest else {
            log::error!("New file, but no parent directory (organize by dir mode)");
            return;
        };

        // check if its in our latest dir
        let Ok(_) = p.strip_prefix(lp) else {
            log::info!("New file, but not in latest directory. Skipping");
            return;
        };

        // it is, so lets load this
        tx.send(PlatterCommand::LoadFile(p.clone(), Some(source_id)))
            .await
            .unwrap();
        return;
    }

    if dir.latest_only {
        log::debug!("Only latest is allowed, clearing");
        tx.send(PlatterCommand::ClearTag(source_id)).await.unwrap();
    }

    tx.send(PlatterCommand::LoadFile(p.clone(), Some(source_id)))
        .await
        .unwrap();
}

async fn load_existing(dir: &Directory, tx: &mpsc::Sender<PlatterCommand>, source_id: Tag) {
    let Ok(paths) = fs::read_dir(&dir.dir) else {
        log::warn!("Unable to read directory: {dir:?}");
        return;
    };

    for path in paths {
        let Ok(path) = path else {
            continue;
        };
        tx.send(PlatterCommand::LoadFile(path.path(), Some(source_id)))
            .await
            .unwrap();
    }
}

/// Construct a file watcher and channel for notifications
fn setup_watcher() -> notify::Result<(RecommendedWatcher, mpsc::Receiver<notify::Result<Event>>)> {
    let (send_from_watcher, recv_from_watcher) = mpsc::channel(16);

    let watcher = RecommendedWatcher::new(
        move |result| {
            if send_from_watcher.blocking_send(result).is_err() {
                log::warn!("Unable to send filesystem notification. Is this during a shutdown?");
            }
        },
        Config::default(),
    )?;

    Ok((watcher, recv_from_watcher))
}

#[cfg(test)]
mod test {
    use std::{
        collections::{HashSet, VecDeque},
        path::{Path, PathBuf},
    };

    use colabrodo_server::server::tokio;
    use serial_test::serial;
    use tempfile::TempDir;

    use crate::{
        arguments::Directory,
        platter_state::{PlatterCommand, Tag},
    };

    fn make_test_dir() -> TempDir {
        TempDir::new().expect("unable to create temp dir")
    }

    fn get_asset(asset_name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(asset_name)
    }

    fn copy_asset(dir: &Path, asset_name: &str) -> PathBuf {
        println!("Copy asset {asset_name} to {}", dir.display());
        let new_file_path = dir.join(asset_name);
        std::fs::copy(get_asset(asset_name), &new_file_path).unwrap();
        new_file_path
    }

    #[tokio::test]
    #[serial]
    async fn test_dir_watch() {
        let test_dir = make_test_dir();

        let setup = Directory {
            dir: test_dir.path().into(),
            load_existing: false,
            latest_only: false,
            organize_by_dir: false,
        };

        let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(16);
        let (stop_tx, stop_rx) = tokio::sync::broadcast::channel(1);

        println!("Starting watcher on {}", test_dir.path().display());

        tokio::spawn(super::launch_file_watcher(watcher_tx, setup, stop_rx));

        println!("Watcher up...waiting");

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // copy in a file
        let new_file_path = copy_asset(test_dir.path(), "cube.obj");

        let mut sequence = VecDeque::from([PlatterCommand::LoadFile(new_file_path, None)]);

        println!("Awaiting commands");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        while let Some(command) = watcher_rx.recv().await {
            //println!("Next: {command:?}");
            let should_be = sequence.pop_front().expect("expected command underflow");
            match (command, should_be) {
                (PlatterCommand::LoadFile(x, _), PlatterCommand::LoadFile(y, _)) => {
                    assert_eq!(x, y);
                }
                (PlatterCommand::ClearTag(x), PlatterCommand::ClearTag(y)) => {
                    assert_eq!(x, y);
                }
                (a, b) => {
                    panic!("Mismatched commands {a:?} != {b:?}");
                }
            }
            if sequence.is_empty() {
                stop_tx.send(true).unwrap();
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_exclusive_watch() {
        // std::env::set_var("RUST_LOG", "debug");
        // env_logger::init();
        let test_dir = make_test_dir();

        let setup = Directory {
            dir: test_dir.path().into(),
            load_existing: false,
            latest_only: true,
            organize_by_dir: false,
        };

        let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(16);
        let (stop_tx, stop_rx) = tokio::sync::broadcast::channel(1);

        println!("Starting watcher on {}", test_dir.path().display());

        tokio::spawn(super::launch_file_watcher(watcher_tx, setup, stop_rx));

        println!("Watcher up...waiting");

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        // copy in a file
        let new_file_path1 = copy_asset(test_dir.path(), "cube.obj");

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        // copy in a second file
        let new_file_path2 = copy_asset(test_dir.path(), "monkey.obj");

        let mut sequence = VecDeque::from([
            PlatterCommand::ClearTag(Tag::new()),
            PlatterCommand::LoadFile(new_file_path1, None),
            PlatterCommand::ClearTag(Tag::new()),
            PlatterCommand::LoadFile(new_file_path2, None),
        ]);

        println!("Awaiting commands");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let mut last_tag = Tag::new();
        let mut first_clear = true; // we dont know the first tag yet.

        while let Some(command) = watcher_rx.recv().await {
            //println!("Next: {command:?}");
            let should_be = sequence.pop_front().expect("expected command underflow");
            match (command, should_be) {
                (PlatterCommand::LoadFile(x, u), PlatterCommand::LoadFile(y, _)) => {
                    last_tag = u.unwrap();
                    assert_eq!(x, y);
                }
                (PlatterCommand::ClearTag(x), PlatterCommand::ClearTag(_)) => {
                    if first_clear {
                        first_clear = false;
                    } else {
                        // now we know the tag and can test
                        assert_eq!(x, last_tag);
                    }
                }
                (a, b) => {
                    panic!("Mismatched commands {a:?} != {b:?}");
                }
            }
            if sequence.is_empty() {
                stop_tx.send(true).unwrap();
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn test_exclusive_dir_watch() {
        //std::env::set_var("RUST_LOG", "debug");
        //env_logger::init();
        let test_dir = make_test_dir();

        let setup = Directory {
            dir: test_dir.path().into(),
            load_existing: false,
            latest_only: true,
            organize_by_dir: true,
        };

        let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(16);
        let (stop_tx, stop_rx) = tokio::sync::broadcast::channel(1);

        println!("Starting watcher on {}", test_dir.path().display());

        tokio::spawn(super::launch_file_watcher(watcher_tx, setup, stop_rx));

        println!("Watcher up...waiting");

        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let d1 = test_dir.path().join("d1");
        let d2 = test_dir.path().join("d2");

        std::fs::create_dir(&d1).unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let mut sequence = VecDeque::new();

        sequence.push_back(PlatterCommand::ClearTag(Tag::new()));

        // copy in a file
        let new_file_path1 = copy_asset(d1.as_path(), "cube.obj");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        // copy in a second file
        let new_file_path2 = copy_asset(d1.as_path(), "monkey.obj");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        sequence.push_back(PlatterCommand::LoadFile(new_file_path1, None));
        sequence.push_back(PlatterCommand::LoadFile(new_file_path2, None));

        std::fs::create_dir(&d2).unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        sequence.push_back(PlatterCommand::ClearTag(Tag::new()));

        // copy in a file
        let new_file_path1 = copy_asset(d2.as_path(), "cube.obj");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        // copy in a second file
        let new_file_path2 = copy_asset(d2.as_path(), "monkey.obj");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        sequence.push_back(PlatterCommand::LoadFile(new_file_path1, None));
        sequence.push_back(PlatterCommand::LoadFile(new_file_path2, None));

        println!("Awaiting commands");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let mut known_tags = HashSet::new();

        while let Some(command) = watcher_rx.recv().await {
            println!("Next: {command:?}");
            let should_be = sequence.pop_front().expect("expected command underflow");
            match (command, should_be) {
                (PlatterCommand::LoadFile(x, u), PlatterCommand::LoadFile(y, _)) => {
                    known_tags.insert(u.unwrap().clone());
                    assert_eq!(x, y);
                }
                (PlatterCommand::ClearTag(x), PlatterCommand::ClearTag(_)) => {
                    if !known_tags.is_empty() {
                        assert!(known_tags.contains(&x));
                        known_tags.remove(&x);
                    }
                }
                (a, b) => {
                    panic!("Mismatched commands {a:?} != {b:?}");
                }
            }
            if sequence.is_empty() {
                stop_tx.send(true).unwrap();
            }
        }
    }
}
