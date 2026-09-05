//! `gridlook dump`: an `ncdump`-compatible header dump for every format
//! gridlook reads, from local paths or object-store URLs.
//!
//! Flags mirror ncdump's letters where the behavior exists (`-h`, `-s`,
//! `-k`, `-n`, `-g`). ncdump's data-section flags (`-c`, `-v`, `-x`, `-t`,
//! `-i`, `-w`, `-l`, `-p`, `-b`, `-f`) are recognized but rejected with a
//! clear "not implemented" message rather than clap's generic "unexpected
//! argument", since only the header is produced for now.

use std::io::{self, Write};
use std::process::ExitCode;

use clap::{ArgAction, Args, ValueEnum};
use gridlook_cdl::{CdlOptions, kind_string, render_cdl};
use gridlook_meta::{FormatHint, RemoteOptions, Source, SummarizeOptions, summarize_source};

/// Exit status for a source that could not be read or rendered.
const EXIT_FAILURE: u8 = 1;
/// Exit status for a usage problem (an unimplemented ncdump flag).
const EXIT_USAGE: u8 = 2;

#[derive(Args, Debug)]
#[command(
    disable_help_flag = true,
    after_help = "SOURCE may be a local file or directory, or a URL: file://, s3://, gs://, \
                  az://, http(s)://. Remote requests use the provider's default credential \
                  chain (environment, instance metadata, workload identity); pass \
                  --anonymous for public data. NetCDF/HDF5 objects are downloaded whole \
                  before reading; Zarr and Icechunk are read one metadata object at a time."
)]
pub struct DumpArgs {
    /// Show only the header (dimensions, variables, attributes, groups).
    /// This is currently the only mode; without -h a reminder is printed to
    /// stderr that the data section is not implemented yet.
    #[arg(short = 'h', long = "header")]
    pub header: bool,

    /// Also show special virtual attributes describing storage: _Storage,
    /// _ChunkSizes, _DeflateLevel, _Endianness, _Format, ... (Zarr: _Codecs,
    /// _ChunkKeyEncoding; Icechunk: _IcechunkBranch, _IcechunkSnapshot).
    #[arg(short = 's', long = "special")]
    pub specials: bool,

    /// Print only the format kind: classic, 64-bit offset, cdf5, netCDF-4,
    /// netCDF-4 classic model, Zarr v2, Zarr v3, or Icechunk.
    #[arg(short = 'k', long = "kind")]
    pub kind: bool,

    /// Name to print after `netcdf` on the first line (default: the source's
    /// file name without its extension).
    #[arg(short = 'n', long = "name", value_name = "NAME")]
    pub name: Option<String>,

    /// Show only these groups and their descendants (comma-separated leaf
    /// names or full paths such as /group_a/nested; "/" is the root).
    #[arg(
        short = 'g',
        long = "group",
        value_name = "GROUP,...",
        value_delimiter = ','
    )]
    pub groups: Option<Vec<String>>,

    /// Read with this format's reader instead of detecting the source
    /// format. (`--format` is reserved for choosing the output format.)
    #[arg(long = "source-format", value_enum, value_name = "FORMAT")]
    pub source_format: Option<FormatArg>,

    /// Send unsigned requests (public buckets) instead of using credentials.
    #[arg(long, alias = "no-sign-request")]
    pub anonymous: bool,

    /// S3 region (default: AWS_REGION / AWS_DEFAULT_REGION, else us-east-1).
    #[arg(long, value_name = "REGION")]
    pub region: Option<String>,

    /// S3-compatible endpoint URL (MinIO, Ceph, R2, ...).
    #[arg(long, value_name = "URL")]
    pub endpoint: Option<String>,

    /// Allow plain-http object store endpoints.
    #[arg(long)]
    pub allow_http: bool,

    /// Print help (-h is ncdump's "header only", so help is --help only).
    #[arg(long, action = ArgAction::HelpLong)]
    help: Option<bool>,

    // ncdump flags for the data section and output tweaks. Accepted by the
    // parser so users get a precise "not implemented" message; hidden from
    // --help so they are not advertised.
    #[arg(short = 'c', hide = true)]
    coords: bool,
    #[arg(short = 'v', hide = true, value_name = "VAR,...")]
    variables: Option<String>,
    #[arg(short = 'x', hide = true)]
    xml: bool,
    #[arg(short = 't', hide = true)]
    time_strings: bool,
    #[arg(short = 'i', hide = true)]
    iso_time: bool,
    #[arg(short = 'w', hide = true)]
    no_wrap: bool,
    #[arg(short = 'l', hide = true, value_name = "LENGTH")]
    line_length: Option<String>,
    #[arg(short = 'p', hide = true, value_name = "F[,D]")]
    precision: Option<String>,
    #[arg(short = 'b', hide = true, value_name = "[c|f]")]
    brief: Option<String>,
    #[arg(short = 'f', hide = true, value_name = "[c|f]")]
    full: Option<String>,

    /// File, directory, or URL to read.
    #[arg(value_name = "SOURCE")]
    pub source: String,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatArg {
    /// NetCDF-3/4 or HDF5 file
    Netcdf,
    /// Zarr v2 or v3 store
    Zarr,
    /// Icechunk repository
    Icechunk,
}

impl From<FormatArg> for FormatHint {
    fn from(arg: FormatArg) -> Self {
        match arg {
            FormatArg::Netcdf => FormatHint::NetCdf,
            FormatArg::Zarr => FormatHint::Zarr,
            FormatArg::Icechunk => FormatHint::Icechunk,
        }
    }
}

impl DumpArgs {
    /// The first ncdump flag given that this version does not implement, as
    /// `(flag, what it does in ncdump)`.
    fn unimplemented_flag(&self) -> Option<(&'static str, &'static str)> {
        let flags: [(&str, &str, bool); 10] = [
            ("-c", "coordinate variable data", self.coords),
            (
                "-v",
                "data for selected variables",
                self.variables.is_some(),
            ),
            ("-x", "NcML/XML output", self.xml),
            ("-t", "time values as date-time strings", self.time_strings),
            ("-i", "ISO 8601 time strings", self.iso_time),
            ("-w", "no line wrapping of data", self.no_wrap),
            ("-l", "data line length", self.line_length.is_some()),
            ("-p", "data precision", self.precision.is_some()),
            ("-b", "brief data annotations", self.brief.is_some()),
            ("-f", "full data annotations", self.full.is_some()),
        ];
        flags
            .into_iter()
            .find(|(_, _, given)| *given)
            .map(|(flag, what, _)| (flag, what))
    }
}

pub fn run(args: &DumpArgs) -> ExitCode {
    if let Some((flag, what)) = args.unimplemented_flag() {
        eprintln!(
            "gridlook dump: {flag} ({what}) is not implemented yet; \
             only the header (-h) is available"
        );
        return ExitCode::from(EXIT_USAGE);
    }

    match run_inner(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("gridlook dump: {err}");
            ExitCode::from(EXIT_FAILURE)
        }
    }
}

fn run_inner(args: &DumpArgs) -> Result<(), Box<dyn std::error::Error>> {
    let source = Source::parse(&args.source)?;
    let remote = RemoteOptions {
        anonymous: args.anonymous,
        region: args.region.clone(),
        endpoint: args.endpoint.clone(),
        allow_http: args.allow_http,
    };
    let opts = SummarizeOptions {
        // `-k` needs the precise file kind, which only the detailed read
        // records; `-s` needs everything.
        storage_details: args.specials || args.kind,
    };
    let hint = args.source_format.map(FormatHint::from);
    let summary = summarize_source(&source, hint, &opts, &remote)?;

    if args.kind {
        return write_stdout(&format!("{}\n", kind_string(&summary)));
    }

    let cdl = CdlOptions {
        name: args.name.clone().unwrap_or_else(|| source.display_name()),
        specials: args.specials,
        groups: args.groups.clone(),
    };
    let text = render_cdl(&summary, &cdl)?;
    write_stdout(&text)?;

    if !args.header {
        eprintln!(
            "note: gridlook dump prints the header only; the data section is not \
             implemented yet (pass -h to silence this note)"
        );
    }
    Ok(())
}

/// Writes to stdout, treating a closed pipe (`| head`) as success.
fn write_stdout(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::stdout().lock();
    match stdout
        .write_all(text.as_bytes())
        .and_then(|()| stdout.flush())
    {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        Err(err) => Err(err.into()),
    }
}
