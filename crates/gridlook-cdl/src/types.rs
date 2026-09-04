//! Mapping the model's numpy-style dtype strings to CDL type names.

/// The CDL type name for a numpy-style dtype (`float32` → `float`,
/// `int8` → `byte`, `|S8` → `char`, `object`/`<U8` → `string`).
///
/// Types with no CDL equivalent (`bool`, `complex64`, `compound`,
/// `datetime64[ns]`, Zarr extension types) pass through verbatim: the output
/// then is not valid CDL for `ncgen`, but it says what the data is, which
/// is what a header dump is for.
pub fn cdl_type_name(dtype: &str) -> &str {
    match dtype {
        "float32" => "float",
        "float64" => "double",
        "int8" => "byte",
        "uint8" => "ubyte",
        "int16" => "short",
        "uint16" => "ushort",
        "int32" => "int",
        "uint32" => "uint",
        "int64" => "int64",
        "uint64" => "uint64",
        "object" => "string",
        d if d.starts_with("|S") => "char",
        d if d.starts_with("<U") || d.starts_with(">U") || d.starts_with("|U") => "string",
        other => other,
    }
}

/// For a fixed-width bytes dtype (`|S8`), the width; `None` otherwise.
pub fn fixed_string_length(dtype: &str) -> Option<u64> {
    dtype.strip_prefix("|S")?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_numpy_names_to_cdl() {
        assert_eq!(cdl_type_name("float32"), "float");
        assert_eq!(cdl_type_name("float64"), "double");
        assert_eq!(cdl_type_name("int8"), "byte");
        assert_eq!(cdl_type_name("uint8"), "ubyte");
        assert_eq!(cdl_type_name("int16"), "short");
        assert_eq!(cdl_type_name("uint16"), "ushort");
        assert_eq!(cdl_type_name("int32"), "int");
        assert_eq!(cdl_type_name("uint32"), "uint");
        assert_eq!(cdl_type_name("int64"), "int64");
        assert_eq!(cdl_type_name("uint64"), "uint64");
        assert_eq!(cdl_type_name("|S8"), "char");
        assert_eq!(cdl_type_name("<U8"), "string");
        assert_eq!(cdl_type_name("object"), "string");
        assert_eq!(cdl_type_name("bool"), "bool");
        assert_eq!(cdl_type_name("complex64"), "complex64");
        assert_eq!(cdl_type_name("datetime64[ns]"), "datetime64[ns]");
    }

    #[test]
    fn fixed_string_width() {
        assert_eq!(fixed_string_length("|S8"), Some(8));
        assert_eq!(fixed_string_length("float32"), None);
    }
}
