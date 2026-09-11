# FAQ

## Why won't the app open my extension-less GRIB file?

NCEP publishes GRIB products with no file extension at all (e.g.
`gfs.t00z.pgrb2.0p25.f003`). The Quick Look preview still works for these —
the extension sniffs the file's `GRIB` signature, so pressing Space in the
Finder previews them regardless of name.

The document-based app is different: macOS's Open panel and Finder's
"Open With" filter on declared content types, which are matched by file
extension before the app ever sees the file. An extension-less GRIB file is
just `public.data` to the system, so it is greyed out in the Open panel and
can't be routed to ndLook.

**Workaround:** rename the file with a `.grib2` extension (any of `.grib`,
`.grib2`, `.grb`, `.grb2`, `.gb2` works). The file's contents are untouched;
GRIB tooling identifies messages by signature, not by name.
