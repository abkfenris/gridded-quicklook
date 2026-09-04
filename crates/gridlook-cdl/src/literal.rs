//! CDL literals: how `ncdump` spells attribute values and names.
//!
//! Numbers carry a type suffix so the text round-trips through `ncgen` with
//! the same type: `b` byte, `UB` ubyte, `s` short, `US` ushort, (none) int,
//! `U` uint, `LL` int64, `ULL` uint64, `f` float, (none) double. Floats use
//! C's `%.7g` / `%.15g` with trailing zeros trimmed but the decimal point
//! kept (`15.f`, `0.1`, `1.e+20f`), and non-finite values spell out as
//! `NaNf`, `Infinityf`, `-Infinity`. Strings are double-quoted with C-style
//! escapes.

use gridlook_meta::{AttrScalar, AttrValue, NumKind};

/// How to spell the two "wide" numeric variants, which mean different things
/// depending on where the attribute came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberPolicy {
    /// NetCDF: `Int` is `NC_INT64` (suffix `LL`), `Float` is `NC_DOUBLE`.
    NetCdf,
    /// JSON sources (Zarr, Icechunk): integers and floats are untyped JSON
    /// numbers and print bare, since `5LL` would claim a type the store
    /// never recorded.
    Json,
}

/// Renders an attribute value as a CDL literal list (`1.5f, 2.f`, `"text"`,
/// `1b, 2b`). An empty list renders as an empty string, which CDL accepts
/// (`name = ;`).
pub fn attr_literal(value: &AttrValue, policy: NumberPolicy) -> String {
    let kind = value.num_kind();
    value
        .scalars()
        .into_iter()
        .map(|scalar| scalar_literal(scalar, kind, policy))
        .collect::<Vec<_>>()
        .join(", ")
}

fn scalar_literal(scalar: AttrScalar<'_>, kind: Option<NumKind>, policy: NumberPolicy) -> String {
    match scalar {
        AttrScalar::Text(s) => cdl_string(s),
        AttrScalar::Int(i) => {
            let suffix = match kind {
                Some(NumKind::I8) => "b",
                Some(NumKind::I16) => "s",
                Some(NumKind::I64) if policy == NumberPolicy::NetCdf => "LL",
                _ => "",
            };
            format!("{i}{suffix}")
        }
        AttrScalar::UInt(u) => {
            let suffix = match kind {
                Some(NumKind::U8) => "UB",
                Some(NumKind::U16) => "US",
                Some(NumKind::U32) => "U",
                Some(NumKind::U64) => "ULL",
                _ => "",
            };
            format!("{u}{suffix}")
        }
        AttrScalar::F32(f) => float_literal(f.into(), 7, "f"),
        AttrScalar::F64(f) => {
            let suffix = "";
            let _ = policy;
            float_literal(f, 15, suffix)
        }
    }
}

/// A float the way ncdump prints it: `%.{sig}g` with trailing zeros trimmed
/// (keeping the decimal point) plus `suffix`; `NaN`/`Infinity`/`-Infinity`
/// (plus `suffix`) when not finite.
pub fn float_literal(value: f64, significant_digits: usize, suffix: &str) -> String {
    if value.is_nan() {
        return format!("NaN{suffix}");
    }
    if value.is_infinite() {
        return if value < 0.0 {
            format!("-Infinity{suffix}")
        } else {
            format!("Infinity{suffix}")
        };
    }
    format!("{}{suffix}", fmt_g(value, significant_digits))
}

/// Emulates C's `%.Ng` followed by ncdump's `tztrim`: `N` significant
/// digits, scientific notation when the exponent is below -4 or at least
/// `N`, trailing zeros in the fraction removed but the decimal point kept.
pub fn fmt_g(value: f64, precision: usize) -> String {
    let precision = precision.max(1);
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.".to_owned()
        } else {
            "0.".to_owned()
        };
    }
    let sign = if value < 0.0 { "-" } else { "" };
    // `{:e}` rounds to the requested significant digits and reports the
    // exponent *after* rounding, so 9.9999999 at 7 digits is 1.000000e1.
    let sci = format!("{:.*e}", precision - 1, value.abs());
    let (mantissa, exponent) = sci
        .split_once('e')
        .expect("Rust `{:e}` always has an exponent");
    let exponent: i32 = exponent.parse().expect("exponent is an integer");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();

    if exponent < -4 || exponent >= precision as i32 {
        let (first, rest) = digits.split_at(1);
        let rest = rest.trim_end_matches('0');
        return format!(
            "{sign}{first}.{rest}e{}{:02}",
            if exponent < 0 { '-' } else { '+' },
            exponent.abs()
        );
    }

    let (int_part, frac_part) = if exponent >= 0 {
        let split = exponent as usize + 1;
        (digits[..split].to_owned(), digits[split..].to_owned())
    } else {
        let zeros = "0".repeat((-exponent - 1) as usize);
        ("0".to_owned(), format!("{zeros}{digits}"))
    };
    let frac_part = frac_part.trim_end_matches('0');
    format!("{sign}{int_part}.{frac_part}")
}

/// A double-quoted CDL string with ncdump's escapes: `\"`, `\\`, `\n`,
/// `\t`, `\r`, `\b`, `\f`, `\v`, and `\ooo` for other control bytes.
/// Non-ASCII text passes through as UTF-8. (ncdump breaks lines after an
/// embedded newline only for classic-format files; the netCDF-4 behavior
/// of keeping the string on one line is used throughout.)
pub fn cdl_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A dimension/variable/attribute/group name as CDL spells it: characters
/// other than letters, digits, `_`, `@` and non-ASCII are backslash-escaped
/// (`my\-var`), matching ncdump's `escaped_name`.
pub fn cdl_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '@' || !c.is_ascii() {
            out.push(c);
        } else {
            out.push('\\');
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_follow_ncdump_g_formatting() {
        assert_eq!(fmt_g(1.5, 7), "1.5");
        assert_eq!(fmt_g(15.0, 7), "15.");
        assert_eq!(fmt_g(0.1, 7), "0.1");
        assert_eq!(fmt_g(-2.25, 7), "-2.25");
        assert_eq!(fmt_g(0.0, 7), "0.");
        assert_eq!(fmt_g(1e20, 7), "1.e+20");
        assert_eq!(fmt_g(1.5e-7, 7), "1.5e-07");
        assert_eq!(fmt_g(123456789.0, 7), "1.234568e+08");
        assert_eq!(fmt_g(9999999.9, 7), "1.e+07");
        assert_eq!(fmt_g(0.0001, 7), "0.0001");
        assert_eq!(fmt_g(0.00001, 7), "1.e-05");
        // f32 values widened to f64 print at 7 digits, as ncdump does.
        assert_eq!(fmt_g(0.1f32 as f64, 7), "0.1");
        assert_eq!(fmt_g(0.1, 15), "0.1");
        assert_eq!(fmt_g(1.0 / 3.0, 15), "0.333333333333333");
    }

    #[test]
    fn float_literals_carry_suffix_and_spell_non_finite() {
        assert_eq!(float_literal(1.5, 7, "f"), "1.5f");
        assert_eq!(float_literal(f64::NAN, 7, "f"), "NaNf");
        assert_eq!(float_literal(f64::INFINITY, 7, "f"), "Infinityf");
        assert_eq!(float_literal(f64::NEG_INFINITY, 15, ""), "-Infinity");
        assert_eq!(float_literal(f64::NAN, 15, ""), "NaN");
    }

    #[test]
    fn integer_literals_carry_type_suffixes() {
        let n = NumberPolicy::NetCdf;
        assert_eq!(
            attr_literal(&AttrValue::Int8List(vec![0, 1, 2]), n),
            "0b, 1b, 2b"
        );
        assert_eq!(attr_literal(&AttrValue::UInt8(255), n), "255UB");
        assert_eq!(attr_literal(&AttrValue::Int16(-3), n), "-3s");
        assert_eq!(attr_literal(&AttrValue::UInt16(65535), n), "65535US");
        assert_eq!(attr_literal(&AttrValue::Int32(7), n), "7");
        assert_eq!(
            attr_literal(&AttrValue::UInt32(4294967295), n),
            "4294967295U"
        );
        assert_eq!(attr_literal(&AttrValue::Int(9), n), "9LL");
        assert_eq!(
            attr_literal(&AttrValue::UInt64(18446744073709551614), n),
            "18446744073709551614ULL"
        );
        assert_eq!(attr_literal(&AttrValue::Float32(1.5), n), "1.5f");
        assert_eq!(attr_literal(&AttrValue::Float(0.1), n), "0.1");
        assert_eq!(
            attr_literal(&AttrValue::Float32List(vec![0.0, 100.0]), n),
            "0.f, 100.f"
        );
    }

    #[test]
    fn json_numbers_print_bare() {
        let j = NumberPolicy::Json;
        assert_eq!(attr_literal(&AttrValue::Int(9), j), "9");
        assert_eq!(attr_literal(&AttrValue::IntList(vec![1, 2]), j), "1, 2");
        assert_eq!(attr_literal(&AttrValue::Float(2.5), j), "2.5");
        // Narrow types keep their suffix regardless of policy.
        assert_eq!(attr_literal(&AttrValue::Float32(2.5), j), "2.5f");
    }

    #[test]
    fn strings_are_quoted_and_escaped() {
        assert_eq!(cdl_string("plain"), "\"plain\"");
        assert_eq!(cdl_string("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(cdl_string("a\\b"), "\"a\\\\b\"");
        assert_eq!(cdl_string("line1\nline2\t!"), "\"line1\\nline2\\t!\"");
        assert_eq!(cdl_string("bell\u{7}"), "\"bell\\007\"");
        assert_eq!(cdl_string("température"), "\"température\"");
        assert_eq!(
            attr_literal(
                &AttrValue::TextList(vec!["a".into(), "b".into()]),
                NumberPolicy::NetCdf
            ),
            "\"a\", \"b\""
        );
        assert_eq!(
            attr_literal(&AttrValue::IntList(Vec::new()), NumberPolicy::Json),
            ""
        );
    }

    #[test]
    fn names_escape_punctuation() {
        assert_eq!(cdl_name("temperature"), "temperature");
        assert_eq!(cdl_name("my-var"), "my\\-var");
        assert_eq!(cdl_name("a.b c"), "a\\.b\\ c");
        assert_eq!(cdl_name("größe"), "größe");
    }
}
