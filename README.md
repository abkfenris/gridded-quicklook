# GridLook

A macOS Quick Look app extension that previews gridded scientific data (NetCDF/HDF5 files, Zarr stores, and Icechunk repositories) as an
xarray-style dataset repr, plus a `gridlook` command-line tool that dumps the
same metadata as `ncdump`-style CDL.

![Quick Look previewing an Icechunk repository: dimensions, coordinates, data variables, and attributes in xarray's repr style, with the repo's branch, snapshot, and commit ancestry below](docs/quicklook-icechunk.png)

Previews are currently metadata only: dimensions, variables, dtypes, chunking,
attributes, and group hierarchy. Icechunk repositories additionally show the
commit history of their `main` branch.

## Installing

`mise run install-dev` from the repo should try to install things, or yell at you.

## Supported formats

| Kind             | Format               | Recognized by                                   | Notes                                          |
| ---------------- | -------------------- | ----------------------------------------------- | ---------------------------------------------- |
| File             | NetCDF-3 / NetCDF-4  | `.nc`, `.nc4`, `.cdf`                           | Groups (DataTree) supported                    |
| File             | HDF5                 | `.h5`, `.hdf5`, `.he5`                          | Same netCDF-4 reader; badge says HDF5 when the file lacks netCDF-4's markers |
| Directory store  | Zarr v2              | `.zgroup` / `.zarray` / `.zmetadata` at the root | Consolidated metadata used when present        |
| Directory store  | Zarr v3              | `zarr.json` at the root                          | Directory tree walked node by node             |
| Directory store  | Icechunk (spec v1/v2) | `snapshots/` plus `refs/` or `repo` at the root | Tip of `main`, plus its snapshot ancestry      |

Directory stores are dispatched on their **contents**, not their name, so a
store called anything at all previews correctly once Quick Look hands it
over. For the Finder to offer a preview in the first place, though, the
directory needs a recognized extension: the app declares `dev.gridlook.zarr`
(`.zarr`) and `dev.gridlook.icechunk` (`.icechunk`) as exported UTIs
conforming to `com.apple.package`.

## Layout

| Path                   | What it is                                                       |
| ---------------------- | ---------------------------------------------------------------- |
| `crates/gridlook-meta`  | Format readers → a format-agnostic `DatasetSummary`               |
| `crates/gridlook-html`  | Renders a `DatasetSummary` as a self-contained HTML document      |
| `crates/gridlook-cdl`   | Renders a `DatasetSummary` as `ncdump`-style CDL text             |
| `crates/gridlook-cli`   | The `gridlook` command-line tool (`gridlook dump`)                |
| `crates/gridlook-ffi`   | C ABI (`staticlib`) linked into the app extension                 |
| `apple/`               | XcodeGen spec, the host app, and the Quick Look preview extension |
| `fixtures/`            | Fixture generator (`generate.py`); its output is not committed    |

The Icechunk reader lives behind `gridlook-meta`'s non-default `icechunk`
cargo feature (it pulls in a sizable dependency tree); `gridlook-ffi` enables
it, so the extension always has it. Remote sources (object stores and HTTP)
live behind the `remote` feature, which only the CLI enables, so the Quick
Look extension never builds the cloud client stack.

## Command line

`gridlook dump` prints a dataset's header as CDL, the text format `ncdump`
uses, for every supported format. `mise run install-cli` builds it and
installs `gridlook` into `~/.cargo/bin`:

```sh
mise run install-cli
gridlook dump -h data/simple.nc            # header, like ncdump -h
gridlook dump -hs data/store.zarr          # plus storage details (ncdump -s)
gridlook dump -k data/repo.icechunk        # just the format kind
mise run dump -- -hs data/simple.nc        # run from the source tree without installing
```

| Flag                       | Meaning                                                                          |
| -------------------------- | -------------------------------------------------------------------------------- |
| `-h`, `--header`           | Header only. This is currently the only mode, so it is also the default.         |
| `-s`, `--special`          | Add special virtual attributes: `_Storage`, `_ChunkSizes`, `_DeflateLevel`, `_Endianness`, `_Format`, ... For Zarr also `_Codecs`, `_ChunkKeyEncoding`, `_Order`; for Icechunk `_IcechunkBranch`, `_IcechunkSnapshot`, ... |
| `-k`, `--kind`             | Print only the format kind: `classic`, `netCDF-4`, `Zarr v3`, `Icechunk`, ...   |
| `-n NAME`, `-g GROUP,...`  | Rename the dataset / print only the named groups, as in `ncdump`.                |
| `--source-format netcdf\|zarr\|icechunk` | Force a reader instead of detecting the source format. (`--format` is reserved for choosing the *output* format once there is more than CDL.) |
| `--anonymous`, `--region`, `--endpoint`, `--allow-http` | Object-store access options (see below).                    |

`ncdump`'s data-section flags (`-c`, `-v`, `-x`, `-t`, ...) are recognized but
rejected with a "not implemented" message: only the header is produced for
now.

`SOURCE` may be a local file or directory, or a URL:

| URL                                                 | Backend                                         |
| --------------------------------------------------- | ----------------------------------------------- |
| `s3://bucket/prefix`, `https://….amazonaws.com/…`   | Amazon S3 (or S3-compatible with `--endpoint`)  |
| `gs://bucket/prefix`                                | Google Cloud Storage                            |
| `az://container/prefix`, `https://{account}.blob.core.windows.net/{container}/prefix` | Azure Blob Storage |
| `http(s)://host/path`                               | Plain HTTP(S)                                   |
| `file:///path`                                      | Local filesystem                                |

Requests use each provider's default credential chain: environment variables
(`AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`, `GOOGLE_APPLICATION_CREDENTIALS`,
`AZURE_STORAGE_ACCOUNT_NAME`/`AZURE_STORAGE_ACCOUNT_KEY`, ...), instance or
container metadata, and workload identity. `~/.aws/credentials` profiles are
**not** read. Pass `--anonymous` (alias `--no-sign-request`) for public data.

Zarr and Icechunk are read one metadata object at a time. Over plain HTTP,
where directory listing is impossible, a Zarr store needs consolidated
metadata (`.zmetadata`, or zarr-python 3's inline `consolidated_metadata`);
Icechunk repositories always work. NetCDF/HDF5 objects are downloaded whole
to a temporary file before reading, because the statically linked libnetcdf
is built without byte-range access.

## Development

Toolchains (Rust, cmake, uv, xcodegen, prek) are pinned in `mise.toml`:

```sh
mise install
```

Building the Xcode project additionally needs a **full Xcode install**, not
just the Command Line Tools (`xcode-select -p` must point at `Xcode.app`).
`xcodegen` alone works with only the CLT, so the Rust side and project
generation are usable without Xcode.

### Tasks

| Task                          | What it does                                            |
| ----------------------------- | ------------------------------------------------------- |
| `mise run test`               | Regenerate fixtures, then `cargo test --workspace`      |
| `mise run lint`               | `cargo fmt --check` + clippy                            |
| `mise run hooks`              | `prek run --all-files`                                  |
| `mise run fixtures`           | Generate `fixtures/data` + `fixtures/reference` (untracked) |
| `mise run dump -- ARGS`       | Run `gridlook dump ARGS` from the source tree (see "Command line") |
| `mise run install-cli`        | `cargo install` the `gridlook` CLI into `~/.cargo/bin`  |
| `mise run sync-xarray-assets` | Re-copy xarray's repr CSS/SVG into `gridlook-html`       |
| `mise run xcodeproj`          | Generate `apple/GridLook.xcodeproj`             |
| `mise run build-appex`        | `xcodebuild` the extension (needs full Xcode)           |
| `mise run install-dev`        | `scripts/install-dev.sh`                                |
| `mise run preview`            | Reset/reload the Quick Look daemon                      |

To use the extension locally, `mise run install-dev`
builds it, ad-hoc signs it, and installs it into `~/Applications` (no Apple
Developer account or provisioning profile required).

Snapshot tests use [insta](https://insta.rs); run `cargo insta review` after
an intentional change. The Icechunk snapshots redact snapshot ids and
timestamps, so regenerating fixtures does not churn them.

If `ncdump` is on your PATH when fixtures are generated (`brew install
netcdf`), reference `.cdl` headers are written to `fixtures/reference/` so
`gridlook dump` output can be diffed against the real thing, e.g.
`diff fixtures/reference/simple.s.cdl <(mise run dump -- -hs fixtures/data/simple.nc)`.

A `.devcontainer/` is provided for Linux work on the Rust crates. A Mac with
Xcode is needed to build or run the macOS app extension.

## Attribution

`crates/gridlook-html/assets/` contains xarray's HTML-repr stylesheet and
inline SVG icons, copied from
[pydata/xarray](https://github.com/pydata/xarray) (Apache License 2.0,
copyright the xarray contributors) so that previews are styled identically
to xarray's own `_repr_html_`. Refresh them with
`mise run sync-xarray-assets`.

## License

BSD-3-Clause. See [LICENSE](LICENSE).
