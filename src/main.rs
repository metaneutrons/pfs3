use anyhow::Result;
use clap::{Parser, Subcommand};
use libpfs3::volume::Volume;
use std::path::{Path, PathBuf};

mod cmd;

#[derive(Parser)]
#[command(name = "pfs3", version, about = "PFS3 filesystem tools")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show filesystem information
    Info {
        image: PathBuf,
        #[arg(long, default_value = "0")]
        offset: u64,
        /// Partition name or index (e.g. DH0, DH1, 0, 1)
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// List directory contents
    Ls {
        image: PathBuf,
        #[arg(default_value = "/")]
        path: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Extract files from the image
    Extract {
        image: PathBuf,
        #[arg(default_value = "/")]
        path: String,
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short = 'P')]
        partition: Option<String>,
    },
    /// Print file contents to stdout
    Cat {
        image: PathBuf,
        path: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Check filesystem consistency
    Check {
        image: PathBuf,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
        /// Attempt to repair detected issues
        #[arg(long)]
        repair: bool,
    },
    /// Format a partition/image as PFS3
    Mkfs {
        image: PathBuf,
        #[arg(short, long, default_value = "Untitled")]
        name: String,
        #[arg(long)]
        size_mb: Option<u32>,
        #[arg(long, default_value = "0")]
        offset: u64,
    },
    /// Write a local file into the PFS3 image
    Write {
        image: PathBuf,
        src: PathBuf,
        dest: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Create a directory inside the PFS3 image
    Mkdir {
        image: PathBuf,
        path: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Remove a file or empty directory
    Rm {
        image: PathBuf,
        path: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Set Amiga protection bits (e.g. "rwed", "+p", "-wd")
    Protect {
        image: PathBuf,
        path: String,
        /// Protection spec: "rwed", "+rw", "-ed", "hsparwed"
        bits: String,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Change volume properties
    Tune {
        image: PathBuf,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value = "0")]
        offset: u64,
    },
    /// List partitions in an RDB disk image
    Partitions { image: PathBuf },
    /// List deleted files in the deldir (trash)
    Deldir {
        image: PathBuf,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
    /// Undelete a file from the deldir
    Undelete {
        image: PathBuf,
        /// Filename (or index from `pfs3 deldir`)
        name: String,
        /// Destination path inside PFS3 (default: root)
        #[arg(short, long)]
        dest: Option<String>,
        #[arg(long, default_value = "0")]
        offset: u64,
        #[arg(long, short)]
        partition: Option<String>,
    },
}

/// Open a volume using --partition or --offset.
fn open_vol(image: &Path, offset: u64, partition: Option<&str>) -> Result<Volume> {
    Ok(Volume::open_auto(image, offset, partition, false)?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Info {
            image,
            offset,
            partition,
        } => {
            let vol = open_vol(&image, offset, partition.as_deref())?;
            cmd::info::run_vol(vol)
        }
        Commands::Ls {
            image,
            path,
            offset,
            partition,
        } => {
            let mut vol = open_vol(&image, offset, partition.as_deref())?;
            cmd::ls::run_vol(&mut vol, &path)
        }
        Commands::Extract {
            image,
            path,
            output,
            offset,
            partition,
        } => {
            let mut vol = open_vol(&image, offset, partition.as_deref())?;
            cmd::extract::run_vol(&mut vol, &path, &output)
        }
        Commands::Cat {
            image,
            path,
            offset,
            partition,
        } => {
            let mut vol = open_vol(&image, offset, partition.as_deref())?;
            cmd::cat::run_vol(&mut vol, &path)
        }
        Commands::Check {
            image,
            offset,
            partition,
            repair,
        } => cmd::check::run(&image, offset, partition.as_deref(), repair),
        Commands::Mkfs {
            image,
            name,
            size_mb,
            offset,
        } => cmd::mkfs::run(&image, &name, size_mb, offset),
        Commands::Write {
            image,
            src,
            dest,
            offset,
            partition,
        } => cmd::write::run(&image, &src, &dest, offset, partition.as_deref()),
        Commands::Mkdir {
            image,
            path,
            offset,
            partition,
        } => cmd::write::mkdir(&image, &path, offset, partition.as_deref()),
        Commands::Rm {
            image,
            path,
            offset,
            partition,
        } => cmd::write::rm(&image, &path, offset, partition.as_deref()),
        Commands::Protect {
            image,
            path,
            bits,
            offset,
            partition,
        } => cmd::protect::run(&image, &path, &bits, offset, partition.as_deref()),
        Commands::Tune {
            image,
            name,
            offset,
        } => cmd::tune::run(&image, name.as_deref(), offset),
        Commands::Partitions { image } => {
            let parts = libpfs3::volume::detect_pfs3_partitions(&image)?;
            if parts.is_empty() {
                println!("No PFS3 partitions found.");
            } else {
                println!(
                    "{:<6} {:<12} {:<12} {}",
                    "Index", "Name", "Offset", "Blocks"
                );
                for (i, p) in parts.iter().enumerate() {
                    println!("{:<6} {:<12} {:<12} {}", i, p.name, p.offset, p.blocks);
                }
            }
            Ok(())
        }
        Commands::Deldir {
            image,
            offset,
            partition,
        } => {
            let mut vol = open_vol(&image, offset, partition.as_deref())?;
            let entries = vol.list_deldir()?;
            if entries.is_empty() {
                println!("Deldir is empty (or not enabled).");
            } else {
                println!("{:<4} {:<16} {:>10} {}", "Idx", "Filename", "Size", "Date");
                println!("{}", "-".repeat(50));
                for (i, e) in entries.iter().enumerate() {
                    let date = libpfs3::util::amiga_date_string(
                        e.creation_day,
                        e.creation_minute,
                        e.creation_tick,
                    );
                    println!("{:<4} {:<16} {:>10} {}", i, e.filename, e.file_size(), date);
                }
                println!("\n{} deleted files", entries.len());
            }
            Ok(())
        }
        Commands::Undelete {
            image,
            name,
            dest,
            offset,
            partition,
        } => cmd::write::undelete(&image, &name, dest.as_deref(), offset, partition.as_deref()),
    }
}
