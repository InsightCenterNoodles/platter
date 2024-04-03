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

    /// New files may show up in subdirectories. Combine with `latest_only`.
    #[arg(short, long)]
    pub organize_by_dir: bool,
}

#[derive(Parser)]
#[command(name = "platter")]
#[command(version = clap::crate_version!())]
#[command(about = "Publish meshes to the NOODLES protocol", long_about = None)]
pub struct Arguments {
    #[command(subcommand)]
    pub source: Source,

    /// Host address to bind to
    #[arg(short, long)]
    pub address: Option<url::Url>,

    /// Port to listen on for clients
    #[arg(short, long)]
    pub port: Option<u16>,

    /// Size in bytes of a 'large' mesh. Large meshes will not be sent inline.
    #[arg(short, long, default_value_t = 4096)]
    pub size_large_limit: u64,

    ///Rescale content by this factor
    #[arg(short, long)]
    pub rescale: Option<f32>,

    ///Offset content by a vector as provided by a string
    #[arg(short, long)]
    pub offset: Option<String>,
}

pub fn get_arguments() -> Arguments {
    Arguments::parse()
}
