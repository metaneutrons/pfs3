use anyhow::{Result, bail};
use clap::{Args, Parser, Subcommand};
use libpfs3::volume::{Volume, detect_pfs3_partitions};
use std::path::PathBuf;

mod cmd;

#[derive(Parser)]
#[command(name = "pfs3", version, about = "PFS3 filesystem tools")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// Common arguments for selecting a partition within a disk image.
#[derive(Args, Clone)]
struct ImageArgs {
    /// Path to the disk image or device
    image: PathBuf,
    /// Byte offset to the partition start
    #[arg(long, default_value = "0")]
    offset: u64,
    /// Partition name or index (e.g. DH0, DH1, 0, 1)
    #[arg(long, short)]
    partition: Option<String>,
}

impl ImageArgs {
    /// Open a read-only volume, erroring if multiple partitions and none selected.
    fn open_vol(&self) -> Result<Volume> {
        self.require_unambiguous()?;
        Ok(Volume::open_auto(
            &self.image,
            self.offset,
            self.partition.as_deref(),
            false,
        )?)
    }

    /// Error if the image has multiple PFS3 partitions and the user didn't select one.
    fn require_unambiguous(&self) -> Result<()> {
        if self.partition.is_some() || self.offset != 0 {
            return Ok(());
        }
        let parts = detect_pfs3_partitions(&self.image)?;
        if parts.len() > 1 {
            let names: Vec<_> = parts.iter().map(|p| p.name.as_str()).collect();
            bail!(
                "Multiple PFS3 partitions found ({}).\nUse --partition <name> to select one.",
                names.join(", ")
            );
        }
        Ok(())
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Show filesystem information
    Info {
        #[command(flatten)]
        img: ImageArgs,
    },
    /// List directory contents
    Ls {
        #[command(flatten)]
        img: ImageArgs,
        /// Path inside the filesystem
        #[arg(default_value = "/")]
        path: String,
    },
    /// Extract files from the image
    Extract {
        #[command(flatten)]
        img: ImageArgs,
        /// Path inside the filesystem to extract
        #[arg(default_value = "/")]
        path: String,
        /// Local output directory
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
    },
    /// Print file contents to stdout
    Cat {
        #[command(flatten)]
        img: ImageArgs,
        /// Path to the file inside the filesystem
        path: String,
    },
    /// Check filesystem consistency
    Check {
        #[command(flatten)]
        img: ImageArgs,
        /// Attempt to repair detected issues
        #[arg(long)]
        repair: bool,
    },
    /// Format a partition/image as PFS3
    Mkfs {
        #[command(flatten)]
        img: ImageArgs,
        /// Volume name
        #[arg(short, long, default_value = "Untitled")]
        name: String,
        /// Create a new image with this size (in MB)
        #[arg(long)]
        size_mb: Option<u32>,
    },
    /// Write a local file into the PFS3 image
    Write {
        #[command(flatten)]
        img: ImageArgs,
        /// Local source file
        src: PathBuf,
        /// Destination path inside the filesystem
        dest: String,
    },
    /// Create a directory inside the PFS3 image
    Mkdir {
        #[command(flatten)]
        img: ImageArgs,
        /// Directory path to create
        path: String,
    },
    /// Remove a file or empty directory
    Rm {
        #[command(flatten)]
        img: ImageArgs,
        /// Path to remove inside the filesystem
        path: String,
    },
    /// Set Amiga protection bits (e.g. "rwed", "+p", "-wd")
    Protect {
        #[command(flatten)]
        img: ImageArgs,
        /// Path to the file or directory
        path: String,
        /// Protection spec: "rwed", "+rw", "-ed", "hsparwed"
        bits: String,
    },
    /// Change volume properties
    Tune {
        #[command(flatten)]
        img: ImageArgs,
        /// New volume name
        #[arg(long)]
        name: Option<String>,
    },
    /// List partitions in an RDB disk image
    Partitions {
        /// Path to the disk image
        image: PathBuf,
    },
    /// List deleted files in the deldir (trash)
    Deldir {
        #[command(flatten)]
        img: ImageArgs,
    },
    /// Undelete a file from the deldir
    Undelete {
        #[command(flatten)]
        img: ImageArgs,
        /// Filename or index from `pfs3 deldir`
        name: String,
        /// Destination path inside the filesystem (default: root)
        #[arg(short, long)]
        dest: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Info { img } => {
            if img.partition.is_none() && img.offset == 0 {
                let parts = detect_pfs3_partitions(&img.image)?;
                if parts.len() > 1 {
                    return cmd::info::run_overview(&img.image, &parts);
                }
            }
            let vol = img.open_vol()?;
            cmd::info::run_vol(vol)
        }
        Commands::Ls { img, path } => {
            let mut vol = img.open_vol()?;
            cmd::ls::run_vol(&mut vol, &path)
        }
        Commands::Extract { img, path, output } => {
            let mut vol = img.open_vol()?;
            cmd::extract::run_vol(&mut vol, &path, &output)
        }
        Commands::Cat { img, path } => {
            let mut vol = img.open_vol()?;
            cmd::cat::run_vol(&mut vol, &path)
        }
        Commands::Check { img, repair } => {
            img.require_unambiguous()?;
            cmd::check::run(&img.image, img.offset, img.partition.as_deref(), repair)
        }
        Commands::Mkfs { img, name, size_mb } => {
            img.require_unambiguous()?;
            cmd::mkfs::run(
                &img.image,
                &name,
                size_mb,
                img.offset,
                img.partition.as_deref(),
            )
        }
        Commands::Write { img, src, dest } => {
            img.require_unambiguous()?;
            cmd::write::run(
                &img.image,
                &src,
                &dest,
                img.offset,
                img.partition.as_deref(),
            )
        }
        Commands::Mkdir { img, path } => {
            img.require_unambiguous()?;
            cmd::write::mkdir(&img.image, &path, img.offset, img.partition.as_deref())
        }
        Commands::Rm { img, path } => {
            img.require_unambiguous()?;
            cmd::write::rm(&img.image, &path, img.offset, img.partition.as_deref())
        }
        Commands::Protect { img, path, bits } => {
            img.require_unambiguous()?;
            cmd::protect::run(
                &img.image,
                &path,
                &bits,
                img.offset,
                img.partition.as_deref(),
            )
        }
        Commands::Tune { img, name } => {
            img.require_unambiguous()?;
            cmd::tune::run(
                &img.image,
                name.as_deref(),
                img.offset,
                img.partition.as_deref(),
            )
        }
        Commands::Partitions { image } => {
            let parts = detect_pfs3_partitions(&image)?;
            if parts.is_empty() {
                println!("No PFS3 partitions found.");
            } else {
                println!("{:<6} {:<12} {:<12} Blocks", "Index", "Name", "Offset");
                for (i, p) in parts.iter().enumerate() {
                    println!("{:<6} {:<12} {:<12} {}", i, p.name, p.offset, p.blocks);
                }
            }
            Ok(())
        }
        Commands::Deldir { img } => {
            let mut vol = img.open_vol()?;
            let entries = vol.list_deldir()?;
            if entries.is_empty() {
                println!("Deldir is empty (or not enabled).");
            } else {
                println!("{:<4} {:<16} {:>10} Date", "Idx", "Filename", "Size");
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
        Commands::Undelete { img, name, dest } => {
            img.require_unambiguous()?;
            cmd::write::undelete(
                &img.image,
                &name,
                dest.as_deref(),
                img.offset,
                img.partition.as_deref(),
            )
        }
    }
}
