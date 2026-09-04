//! Sources addressed by URL: `http(s)://`, `s3://`, `gs://`, `az://`, and
//! `file://`.
//!
//! Everything remote goes through [`object_store`]. Zarr stores are read
//! directly through a [`ZarrStore`] adapter (one small GET per metadata
//! document); Icechunk repositories are opened with icechunk's own
//! object-store backends; NetCDF/HDF5 files are downloaded whole to a
//! temporary file and opened locally, because the statically linked
//! libnetcdf is built without byte-range/DAP support.
//!
//! Credentials come from each provider's default chain (environment
//! variables, instance/container metadata, workload identity) unless
//! [`RemoteOptions::anonymous`] forces unsigned requests. Note that
//! `object_store`'s AWS chain does not read `~/.aws/credentials` profiles;
//! export `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` (or `AWS_PROFILE` via a
//! credential process) instead.

use std::io::Write;
use std::path::{Path, PathBuf};

use futures_util::StreamExt;
use object_store::aws::AmazonS3Builder;
use object_store::azure::MicrosoftAzureBuilder;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::http::HttpBuilder;
use object_store::local::LocalFileSystem;
use object_store::memory::InMemory;
use object_store::path::Path as ObjPath;
use object_store::{ClientOptions, ObjectStore, ObjectStoreExt, ObjectStoreScheme};
use tokio::runtime::Runtime;
use url::Url;

use crate::dispatch::{FormatHint, ZARR_ROOT_MARKERS, has_netcdf_like_extension, summarize_path};
use crate::error::MetaError;
use crate::model::{DatasetSummary, SummarizeOptions};
use crate::netcdf::summarize_netcdf_with;
use crate::zarr::store::{Listing, ZarrStore};
use crate::zarr::summarize_zarr_store;

/// Region assumed for S3 when neither `--region` nor `AWS_REGION` /
/// `AWS_DEFAULT_REGION` says otherwise. `object_store` does not follow
/// region redirects, so a wrong guess surfaces as an error that names the
/// bucket's real region.
const DEFAULT_S3_REGION: &str = "us-east-1";

/// Where a dataset lives: a local path, or a URL to a remote object store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Remote(Url),
}

impl Source {
    /// Parses a command-line source spec. Anything with a `scheme://` prefix
    /// is a URL (`file://` URLs are turned back into local paths); everything
    /// else is a filesystem path.
    pub fn parse(spec: &str) -> Result<Self, MetaError> {
        let looks_like_url = spec.split_once("://").is_some_and(|(scheme, _)| {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        });
        if !looks_like_url {
            return Ok(Source::Local(PathBuf::from(spec)));
        }
        let url = Url::parse(spec).map_err(|err| MetaError::Unsupported {
            location: spec.to_owned(),
            message: format!("not a valid URL: {err}"),
        })?;
        if url.scheme() == "file" {
            let path = url.to_file_path().map_err(|()| MetaError::Unsupported {
                location: spec.to_owned(),
                message: "a file:// URL must be absolute and have no host".to_owned(),
            })?;
            return Ok(Source::Local(path));
        }
        Ok(Source::Remote(url))
    }

    /// The source as the user would recognize it (path or URL).
    pub fn location(&self) -> String {
        match self {
            Source::Local(path) => path.display().to_string(),
            Source::Remote(url) => url.as_str().to_owned(),
        }
    }

    /// The dataset name `ncdump` would print after `netcdf`: the last path
    /// segment with its extension removed (`simple.nc` → `simple`,
    /// `s3://b/store.zarr/` → `store`), falling back to the URL host and
    /// finally to `dataset`.
    pub fn display_name(&self) -> String {
        let last = match self {
            Source::Local(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Source::Remote(url) => url
                .path_segments()
                .and_then(|mut segments| segments.rfind(|s| !s.is_empty()).map(str::to_owned))
                .or_else(|| url.host_str().map(str::to_owned))
                .unwrap_or_default(),
        };
        Path::new(&last)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .filter(|stem| !stem.is_empty() && stem != "." && stem != "..")
            .unwrap_or_else(|| "dataset".to_owned())
    }
}

/// How to reach remote object stores.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RemoteOptions {
    /// Send unsigned requests (public buckets) instead of using the
    /// provider's default credential chain.
    pub anonymous: bool,
    /// S3 region override.
    pub region: Option<String>,
    /// S3-compatible endpoint override (MinIO, Ceph, R2, ...).
    pub endpoint: Option<String>,
    /// Allow plain-HTTP endpoints for S3/Azure (`--endpoint http://...`).
    pub allow_http: bool,
}

/// Summarizes a local path or remote URL, detecting the format unless
/// `hint` says otherwise.
pub fn summarize_source(
    source: &Source,
    hint: Option<FormatHint>,
    opts: &SummarizeOptions,
    remote: &RemoteOptions,
) -> Result<DatasetSummary, MetaError> {
    match source {
        Source::Local(path) => summarize_path(path, hint, opts),
        Source::Remote(url) => summarize_url(url, hint, opts, remote),
    }
}

/// Summarizes whatever `url` points at through an object store built from
/// the URL's scheme.
fn summarize_url(
    url: &Url,
    hint: Option<FormatHint>,
    opts: &SummarizeOptions,
    remote: &RemoteOptions,
) -> Result<DatasetSummary, MetaError> {
    let (scheme, prefix) = ObjectStoreScheme::parse(url).map_err(|err| MetaError::Unsupported {
        location: url.as_str().to_owned(),
        message: format!("unrecognized URL scheme: {err}"),
    })?;
    let store = build_store(&scheme, url, remote)?;
    let runtime = build_runtime(url)?;
    let source = RemoteSource {
        runtime: &runtime,
        store: store.as_ref(),
        prefix,
        url,
        scheme,
    };
    source.summarize(hint, opts, remote)
}

fn build_runtime(url: &Url) -> Result<Runtime, MetaError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| MetaError::Io {
            location: url.as_str().to_owned(),
            source,
        })
}

/// Builds the object store for `url`'s scheme, reading provider credentials
/// and settings from the environment (the `from_env` builders), then
/// applying `remote` overrides. `object_store::parse_url` is deliberately
/// not used: it constructs clients with `Builder::new()`, which skips the
/// environment entirely.
fn build_store(
    scheme: &ObjectStoreScheme,
    url: &Url,
    remote: &RemoteOptions,
) -> Result<Box<dyn ObjectStore>, MetaError> {
    let location = url.as_str();
    let store: Box<dyn ObjectStore> = match scheme {
        ObjectStoreScheme::Local => Box::new(LocalFileSystem::new()),
        ObjectStoreScheme::Memory => Box::new(InMemory::new()),
        ObjectStoreScheme::AmazonS3 => {
            let mut builder = AmazonS3Builder::from_env()
                .with_url(location)
                .with_skip_signature(remote.anonymous)
                .with_allow_http(remote.allow_http);
            if let Some(region) = remote.region.as_deref().or_else(|| {
                // `from_env` already picked up AWS_REGION / AWS_DEFAULT_REGION
                // when set; only fall back when neither is.
                let unset = std::env::var_os("AWS_REGION").is_none()
                    && std::env::var_os("AWS_DEFAULT_REGION").is_none();
                unset.then_some(DEFAULT_S3_REGION)
            }) {
                builder = builder.with_region(region);
            }
            if let Some(endpoint) = &remote.endpoint {
                builder = builder.with_endpoint(endpoint);
            }
            Box::new(
                builder
                    .build()
                    .map_err(|source| remote_error(location, source))?,
            )
        }
        ObjectStoreScheme::GoogleCloudStorage => Box::new(
            GoogleCloudStorageBuilder::from_env()
                .with_url(location)
                .with_skip_signature(remote.anonymous)
                .build()
                .map_err(|source| remote_error(location, source))?,
        ),
        ObjectStoreScheme::MicrosoftAzure => Box::new(
            MicrosoftAzureBuilder::from_env()
                .with_url(location)
                .with_skip_signature(remote.anonymous)
                .with_allow_http(remote.allow_http)
                .build()
                .map_err(|source| remote_error(location, source))?,
        ),
        ObjectStoreScheme::Http => {
            let base = &url[..url::Position::BeforePath];
            Box::new(
                HttpBuilder::new()
                    .with_url(base)
                    .with_client_options(ClientOptions::new().with_allow_http(true))
                    .build()
                    .map_err(|source| remote_error(location, source))?,
            )
        }
        other => {
            return Err(MetaError::Unsupported {
                location: location.to_owned(),
                message: format!("unsupported object store scheme {other:?}"),
            });
        }
    };
    Ok(store)
}

fn remote_error(location: &str, source: object_store::Error) -> MetaError {
    MetaError::Remote {
        location: location.to_owned(),
        source,
    }
}

/// An object-store location plus the runtime that drives its async calls.
struct RemoteSource<'a> {
    runtime: &'a Runtime,
    store: &'a dyn ObjectStore,
    /// Store-relative path of the dataset root (or of the NetCDF object).
    prefix: ObjPath,
    url: &'a Url,
    scheme: ObjectStoreScheme,
}

impl RemoteSource<'_> {
    fn location(&self) -> &str {
        self.url.as_str()
    }

    fn key(&self, key: &str) -> ObjPath {
        if key.is_empty() {
            self.prefix.clone()
        } else if self.prefix.as_ref().is_empty() {
            ObjPath::from(key)
        } else {
            ObjPath::from(format!("{}/{key}", self.prefix.as_ref()))
        }
    }

    /// Does an object exist at `key` below the prefix? Missing objects are
    /// `false`; any other failure (authentication, network) propagates,
    /// since silently treating a 403 as "absent" would hide the real issue.
    fn exists(&self, key: &str) -> Result<bool, MetaError> {
        let path = self.key(key);
        match self.runtime.block_on(self.store.head(&path)) {
            Ok(_) => Ok(true),
            Err(object_store::Error::NotFound { .. }) => Ok(false),
            Err(source) => Err(remote_error(&self.describe(key), source)),
        }
    }

    fn describe(&self, key: &str) -> String {
        if key.is_empty() {
            self.location().to_owned()
        } else {
            format!("{}/{key}", self.location().trim_end_matches('/'))
        }
    }

    fn list_dir(&self, prefix: &str) -> Result<Listing, MetaError> {
        let path = self.key(prefix);
        let listed = self
            .runtime
            .block_on(self.store.list_with_delimiter(Some(&path)));
        let result = match listed {
            Ok(result) => result,
            Err(source) => {
                // Plain HTTP servers have no listing API at all (object_store
                // tries WebDAV PROPFIND); report that as such rather than as
                // a generic access failure. Real object stores can list, so
                // for them any error is a real error.
                let unsupported = self.scheme == ObjectStoreScheme::Http
                    || matches!(
                        source,
                        object_store::Error::NotImplemented { .. }
                            | object_store::Error::NotSupported { .. }
                    );
                if unsupported {
                    return Err(MetaError::ListingUnsupported {
                        location: self.describe(prefix),
                        message: source.to_string(),
                    });
                }
                return Err(remote_error(&self.describe(prefix), source));
            }
        };
        let mut listing = Listing {
            dirs: result
                .common_prefixes
                .iter()
                .filter_map(|p| p.filename().map(str::to_owned))
                .collect(),
            files: result
                .objects
                .iter()
                .filter_map(|o| o.location.filename().map(str::to_owned))
                .collect(),
        };
        listing.dirs.sort();
        listing.files.sort();
        Ok(listing)
    }

    /// Sniffs the source kind: a NetCDF-like extension on the last URL
    /// segment, else a Zarr root document, else an Icechunk repository
    /// layout (its `repo` object, or `snapshots/` plus `refs/` or
    /// `transactions/` when the store can list).
    fn detect_kind(&self) -> Result<FormatHint, MetaError> {
        if self
            .prefix
            .filename()
            .is_some_and(has_netcdf_like_extension)
        {
            return Ok(FormatHint::NetCdf);
        }
        for marker in ZARR_ROOT_MARKERS {
            if self.exists(marker)? {
                return Ok(FormatHint::Zarr);
            }
        }
        if self.exists("repo")? {
            return Ok(FormatHint::Icechunk);
        }
        match self.list_dir("") {
            Ok(listing) => {
                let has = |name: &str| listing.dirs.iter().any(|d| d == name);
                if has("snapshots") && (has("refs") || has("transactions")) {
                    return Ok(FormatHint::Icechunk);
                }
            }
            Err(MetaError::ListingUnsupported { .. }) => {}
            Err(err) => return Err(err),
        }
        Err(MetaError::Unsupported {
            location: self.location().to_owned(),
            message: "no Zarr root document (zarr.json, .zgroup, .zarray, .zmetadata) or \
                      Icechunk repository found here; pass --source-format to force a reader"
                .to_owned(),
        })
    }

    fn summarize(
        &self,
        hint: Option<FormatHint>,
        opts: &SummarizeOptions,
        remote: &RemoteOptions,
    ) -> Result<DatasetSummary, MetaError> {
        let kind = match hint {
            Some(kind) => kind,
            None => self.detect_kind()?,
        };
        match kind {
            FormatHint::NetCdf => {
                let temp = self.download_to_temp()?;
                summarize_netcdf_with(temp.path(), opts)
            }
            FormatHint::Zarr => summarize_zarr_store(self, opts),
            FormatHint::Icechunk => self.summarize_icechunk(opts, remote),
        }
    }

    /// Streams the object at the prefix into a temporary file that keeps the
    /// source's extension (libnetcdf sniffs content, not names, but the
    /// suffix keeps error messages recognizable). The file is deleted when
    /// the returned handle drops.
    fn download_to_temp(&self) -> Result<tempfile::NamedTempFile, MetaError> {
        let location = self.location().to_owned();
        let io_error = |source: std::io::Error| MetaError::Io {
            location: location.clone(),
            source,
        };
        let extension = self
            .prefix
            .filename()
            .and_then(|name| Path::new(name).extension())
            .map(|ext| format!(".{}", ext.to_string_lossy()))
            .unwrap_or_default();
        let mut temp = tempfile::Builder::new()
            .prefix("gridlook-")
            .suffix(&extension)
            .tempfile()
            .map_err(io_error)?;

        self.runtime.block_on(async {
            let result = self
                .store
                .get(&self.prefix)
                .await
                .map_err(|source| remote_error(&location, source))?;
            let mut stream = result.into_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk.map_err(|source| remote_error(&location, source))?;
                temp.write_all(&chunk).map_err(io_error)?;
            }
            Ok::<(), MetaError>(())
        })?;
        temp.flush().map_err(io_error)?;
        Ok(temp)
    }

    #[cfg(feature = "icechunk")]
    fn summarize_icechunk(
        &self,
        opts: &SummarizeOptions,
        remote: &RemoteOptions,
    ) -> Result<DatasetSummary, MetaError> {
        self.runtime.block_on(async {
            let storage = icechunk_storage(self.url, &self.scheme, &self.prefix, remote).await?;
            crate::icechunk::summarize_icechunk_storage_async(storage, self.location(), opts).await
        })
    }

    #[cfg(not(feature = "icechunk"))]
    fn summarize_icechunk(
        &self,
        _opts: &SummarizeOptions,
        _remote: &RemoteOptions,
    ) -> Result<DatasetSummary, MetaError> {
        Err(MetaError::Unsupported {
            location: self.location().to_owned(),
            message:
                "Icechunk support was not compiled in (enable gridlook-meta's `icechunk` feature)"
                    .to_owned(),
        })
    }
}

/// The Zarr reader's view of a remote source: metadata documents fetched
/// one GET at a time below the prefix.
impl ZarrStore for RemoteSource<'_> {
    fn location(&self) -> &str {
        RemoteSource::location(self)
    }

    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, MetaError> {
        let path = self.key(key);
        let fetched = self.runtime.block_on(async {
            match self.store.get(&path).await {
                Ok(result) => result.bytes().await.map(Some),
                Err(object_store::Error::NotFound { .. }) => Ok(None),
                Err(err) => Err(err),
            }
        });
        match fetched {
            Ok(bytes) => Ok(bytes.map(|b| b.to_vec())),
            Err(source) => Err(remote_error(&self.describe(key), source)),
        }
    }

    fn list_dir(&self, prefix: &str) -> Result<Listing, MetaError> {
        RemoteSource::list_dir(self, prefix)
    }
}

/// Builds icechunk's own storage backend for the repository at `url`.
///
/// Icechunk needs bucket/prefix (or account/container/prefix) split out, so
/// only the native URL forms are accepted: `s3://bucket/prefix`,
/// `gs://bucket/prefix`, `az://container/prefix` (account from
/// `AZURE_STORAGE_ACCOUNT_NAME`), `https://{account}.blob.core.windows.net/
/// {container}/prefix`, any `http(s)://` base URL, and `file://` paths.
#[cfg(feature = "icechunk")]
async fn icechunk_storage(
    url: &Url,
    scheme: &ObjectStoreScheme,
    prefix: &ObjPath,
    remote: &RemoteOptions,
) -> Result<std::sync::Arc<dyn icechunk::storage::Storage + Send + Sync>, MetaError> {
    use icechunk::config::{S3Credentials, S3Options};
    use icechunk::storage::{
        AzureCredentials, GcsCredentials, new_azure_blob_storage, new_gcs_storage,
        new_http_storage, new_local_filesystem_storage, new_s3_object_store_storage,
    };

    let location = url.as_str();
    let unsupported = |message: String| MetaError::Unsupported {
        location: location.to_owned(),
        message,
    };
    let storage_error = |err: icechunk::storage::StorageError| MetaError::Icechunk {
        location: location.to_owned(),
        message: format!("cannot open Icechunk storage: {err}"),
    };
    let prefix_opt = Some(prefix.as_ref())
        .filter(|p| !p.is_empty())
        .map(str::to_owned);
    let bucket = || {
        url.host_str()
            .map(str::to_owned)
            .ok_or_else(|| unsupported("URL has no bucket/host component".to_owned()))
    };

    match scheme {
        ObjectStoreScheme::Local => {
            let path = url
                .to_file_path()
                .map_err(|()| unsupported("file:// URL must be absolute".to_owned()))?;
            new_local_filesystem_storage(&path)
                .await
                .map_err(storage_error)
        }
        ObjectStoreScheme::AmazonS3 => {
            if !matches!(url.scheme(), "s3" | "s3a") {
                return Err(unsupported(
                    "use the s3://bucket/prefix form for Icechunk repositories on S3".to_owned(),
                ));
            }
            let region = remote
                .region
                .clone()
                .or_else(|| std::env::var("AWS_REGION").ok())
                .or_else(|| std::env::var("AWS_DEFAULT_REGION").ok())
                .unwrap_or_else(|| DEFAULT_S3_REGION.to_owned());
            let mut options = S3Options::default()
                .with_region(region)
                .with_anonymous(remote.anonymous)
                .with_allow_http(remote.allow_http);
            if let Some(endpoint) = &remote.endpoint {
                options = options.with_endpoint_url(endpoint.clone());
            }
            let credentials = if remote.anonymous {
                S3Credentials::Anonymous
            } else {
                S3Credentials::FromEnv
            };
            new_s3_object_store_storage(
                options,
                bucket()?,
                prefix_opt,
                Some(credentials),
                Vec::new(),
                Vec::new(),
            )
            .await
            .map_err(storage_error)
        }
        ObjectStoreScheme::GoogleCloudStorage => {
            let credentials = if remote.anonymous {
                GcsCredentials::Anonymous
            } else {
                GcsCredentials::FromEnv
            };
            new_gcs_storage(
                bucket()?,
                prefix_opt,
                Some(credentials),
                None,
                Vec::new(),
                Vec::new(),
            )
            .map_err(storage_error)
        }
        ObjectStoreScheme::MicrosoftAzure => {
            let host = url.host_str().unwrap_or_default();
            let (account, container) = if url.scheme() == "https" {
                // https://{account}.blob.core.windows.net/{container}/prefix
                let account = host.split('.').next().unwrap_or_default().to_owned();
                let container = url
                    .path_segments()
                    .and_then(|mut s| s.next())
                    .filter(|c| !c.is_empty())
                    .ok_or_else(|| unsupported("Azure URL has no container segment".to_owned()))?
                    .to_owned();
                (account, container)
            } else {
                // az://container/prefix, account from the environment.
                let account = std::env::var("AZURE_STORAGE_ACCOUNT_NAME")
                    .or_else(|_| std::env::var("AZURE_STORAGE_ACCOUNT"))
                    .map_err(|_| {
                        unsupported(
                            "set AZURE_STORAGE_ACCOUNT_NAME (or use the \
                             https://{account}.blob.core.windows.net/{container}/... form)"
                                .to_owned(),
                        )
                    })?;
                (account, host.to_owned())
            };
            let credentials = if remote.anonymous {
                AzureCredentials::Anonymous
            } else {
                AzureCredentials::FromEnv
            };
            new_azure_blob_storage(account, container, prefix_opt, Some(credentials), None)
                .await
                .map_err(storage_error)
        }
        ObjectStoreScheme::Http => new_http_storage(location, None, None).map_err(storage_error),
        other => Err(unsupported(format!(
            "Icechunk repositories are not supported on {other:?} stores"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::model::SourceFormat;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/data")
            .join(name)
            .canonicalize()
            .expect("fixture exists")
    }

    fn file_url(name: &str) -> Url {
        Url::from_file_path(fixture(name)).expect("absolute path")
    }

    /// Summaries are compared through JSON: `NaN` fill values make a direct
    /// `PartialEq` comparison fail even for identical summaries, while JSON
    /// renders them as `null`.
    fn json(summary: &DatasetSummary) -> serde_json::Value {
        serde_json::to_value(summary).expect("serializable")
    }

    #[test]
    fn source_parse_distinguishes_paths_and_urls() {
        assert_eq!(
            Source::parse("data/simple.nc").unwrap(),
            Source::Local(PathBuf::from("data/simple.nc"))
        );
        assert_eq!(
            Source::parse("file:///tmp/x.nc").unwrap(),
            Source::Local(PathBuf::from("/tmp/x.nc"))
        );
        assert!(matches!(
            Source::parse("s3://bucket/key.nc").unwrap(),
            Source::Remote(url) if url.scheme() == "s3"
        ));
        assert!(matches!(
            Source::parse("https://example.org/store.zarr").unwrap(),
            Source::Remote(_)
        ));
        // A Windows-ish or odd spec without a scheme stays a path.
        assert!(matches!(
            Source::parse("weird:name.nc").unwrap(),
            Source::Local(_)
        ));
        assert!(matches!(
            Source::parse("file://host/x.nc"),
            Err(MetaError::Unsupported { .. })
        ));
    }

    #[test]
    fn display_name_strips_extension_like_ncdump() {
        assert_eq!(
            Source::parse("fixtures/data/simple.nc")
                .unwrap()
                .display_name(),
            "simple"
        );
        assert_eq!(
            Source::parse("s3://bucket/data/store.zarr/")
                .unwrap()
                .display_name(),
            "store"
        );
        assert_eq!(
            Source::parse("https://host.example/")
                .unwrap()
                .display_name(),
            "host"
        );
        assert_eq!(Source::parse("/").unwrap().display_name(), "dataset");
    }

    #[test]
    fn local_source_matches_direct_dispatch() {
        let via_source = summarize_source(
            &Source::parse(fixture("simple.nc").to_str().unwrap()).unwrap(),
            None,
            &SummarizeOptions::default(),
            &RemoteOptions::default(),
        )
        .expect("summarize");
        let direct = summarize_path(&fixture("simple.nc"), None, &SummarizeOptions::default())
            .expect("summarize");
        assert_eq!(json(&via_source), json(&direct));
    }

    /// The `file://` scheme drives the whole object-store pipeline (scheme
    /// parsing, kind sniffing, GET/HEAD/list) against the fixtures, so it
    /// stands in for a real bucket in offline tests.
    #[test]
    fn zarr_over_object_store_matches_local_reader() {
        for name in ["simple_v3.zarr", "simple_v2.zarr", "tree.zarr"] {
            let opts = SummarizeOptions {
                storage_details: true,
            };
            let remote = summarize_url(&file_url(name), None, &opts, &RemoteOptions::default())
                .unwrap_or_else(|err| panic!("{name}: {err}"));
            let local = crate::zarr::summarize_zarr_with(&fixture(name), &opts).unwrap();
            assert_eq!(json(&remote), json(&local), "{name}");
        }
    }

    #[test]
    fn netcdf_over_object_store_downloads_then_reads() {
        let opts = SummarizeOptions {
            storage_details: true,
        };
        let remote = summarize_url(
            &file_url("simple.nc"),
            None,
            &opts,
            &RemoteOptions::default(),
        )
        .expect("summarize downloaded netcdf");
        let local = summarize_netcdf_with(&fixture("simple.nc"), &opts).unwrap();
        assert_eq!(remote.format, SourceFormat::NetCdf);
        assert_eq!(json(&remote), json(&local));
    }

    #[cfg(feature = "icechunk")]
    #[test]
    fn icechunk_over_object_store_matches_local_reader() {
        let opts = SummarizeOptions::default();
        let remote = summarize_url(
            &file_url("icechunk_repo.icechunk"),
            None,
            &opts,
            &RemoteOptions::default(),
        )
        .expect("summarize icechunk repo");
        let local =
            crate::icechunk::summarize_icechunk(&fixture("icechunk_repo.icechunk")).unwrap();
        assert_eq!(remote.format, SourceFormat::Icechunk);
        assert_eq!(json(&remote), json(&local));
    }

    #[test]
    fn format_hint_overrides_sniffing() {
        let err = summarize_url(
            &file_url("simple.nc"),
            Some(FormatHint::Zarr),
            &SummarizeOptions::default(),
            &RemoteOptions::default(),
        )
        .expect_err("a .nc file is not a Zarr store");
        // The local filesystem store reports "not a directory" for keys
        // below a file as a generic error rather than not-found, so either
        // "no Zarr markers" or "cannot access" is acceptable here.
        assert!(
            matches!(err, MetaError::Invalid { .. } | MetaError::Remote { .. }),
            "{err}"
        );
    }

    #[test]
    fn unrecognized_remote_directory_is_unsupported() {
        let url = Url::from_file_path(fixture("..")).unwrap();
        let err = summarize_url(
            &url,
            None,
            &SummarizeOptions::default(),
            &RemoteOptions::default(),
        )
        .expect_err("fixtures/ is not a dataset");
        assert!(matches!(err, MetaError::Unsupported { .. }), "{err}");
        assert!(err.to_string().contains("--source-format"));
    }

    #[test]
    fn cloud_stores_build_from_urls_without_network() {
        // Building a client performs no I/O; anonymous access needs no
        // credentials to be configured.
        let anon = RemoteOptions {
            anonymous: true,
            ..RemoteOptions::default()
        };
        for spec in [
            "s3://bucket/prefix/store.zarr",
            "gs://bucket/prefix",
            "https://example.org/data/store.zarr",
        ] {
            let url = Url::parse(spec).unwrap();
            let (scheme, _) = ObjectStoreScheme::parse(&url).unwrap();
            build_store(&scheme, &url, &anon).unwrap_or_else(|err| panic!("{spec}: {err}"));
        }
        let url = Url::parse("https://acct.blob.core.windows.net/container/x").unwrap();
        let (scheme, prefix) = ObjectStoreScheme::parse(&url).unwrap();
        assert_eq!(scheme, ObjectStoreScheme::MicrosoftAzure);
        assert_eq!(prefix.as_ref(), "x");
        build_store(&scheme, &url, &anon).expect("azure client from https URL");
    }
}
