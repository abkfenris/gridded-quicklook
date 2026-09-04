//! `gridlook`: command-line inspection of gridded scientific data (NetCDF,
//! HDF5, Zarr v2/v3, Icechunk), local or in object storage.
//!
//! Subcommands:
//! - `dump` — print a dataset's header as CDL, like `ncdump -h`.

mod dump;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "gridlook",
    version,
    about = "Inspect gridded scientific data: NetCDF, HDF5, Zarr, and Icechunk",
    propagate_version = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Print a dataset's header as CDL, like `ncdump -h`
    Dump(dump::DumpArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Dump(args) => dump::run(&args),
    }
}
