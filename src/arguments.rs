use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Clone, Subcommand)]
pub enum Source {
    /// Publish a single file or directory
    File { name: PathBuf },

    /// Watch a directory; new files will be loaded as soon as they appear.
    Watch(Directory),

    /// Listen on a websocket for geometry (NYI)
    Websocket { port: String },
}

#[derive(Debug, Clone, Args)]
pub struct Directory {
    /// Directory to watch for changes
    pub dir: PathBuf,

    /// Load existing files in the directory first
    #[arg(long)]
    pub load_existing: bool,

    /// When a new file shows up, discard previous objects before loading
    #[arg(short, long)]
    pub latest_only: bool,
}

#[derive(Parser)]
#[command(name = "platter")]
#[command(version = clap::crate_version!())]
#[command(about = "Publish meshes to the NOODLES protocol", long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub source: Source,

    /// Host address to bind to
    #[arg(short, long, default_value_t = String::from("localhost"))]
    pub address: String,

    /// Port to listen on for clients
    #[arg(short, long, default_value_t = 50000)]
    pub port: u16,

    /// Size in bytes of a 'large' mesh. Large meshes will not be sent inline.
    #[arg(short, long, default_value_t = 4096)]
    pub size_large_limit: u64,
}

pub fn get_arguments() -> Arguments {
    Arguments::parse()
}
