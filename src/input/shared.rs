use pyo3::prelude::*;
use pyo3::sync::GILOnceCell;
use pyo3::{intern, Py, PyAny, Python};

use jiter::{JsonErrorType, NumberInt};

use crate::errors::{ErrorTypeDefaults, ValError, ValResult};

use super::{EitherFloat, EitherInt, Input};
static ENUM_META_OBJECT: GILOnceCell<Py<PyAny>> = GILOnceCell::new();

pub fn get_enum_meta_object(py: Python) -> &Bound<'_, PyAny> {
    ENUM_META_OBJECT
        .get_or_init(py, || {
            py.import_bound(intern!(py, "enum"))
                .and_then(|enum_module| enum_module.getattr(intern!(py, "EnumMeta")))
                .unwrap()
                .into()
        })
        .bind(py)
}

pub fn str_as_bool<'py>(input: &(impl Input<'py> + ?Sized), str: &str) -> ValResult<bool> {
    if str == "0"
        || str.eq_ignore_ascii_case("f")
        || str.eq_ignore_ascii_case("n")
        || str.eq_ignore_ascii_case("no")
        || str.eq_ignore_ascii_case("off")
        || str.eq_ignore_ascii_case("false")
    {
        Ok(false)
    } else if str == "1"
        || str.eq_ignore_ascii_case("t")
        || str.eq_ignore_ascii_case("y")
        || str.eq_ignore_ascii_case("on")
        || str.eq_ignore_ascii_case("yes")
        || str.eq_ignore_ascii_case("true")
    {
        Ok(true)
    } else {
        Err(ValError::new(ErrorTypeDefaults::BoolParsing, input))
    }
}

pub fn int_as_bool<'py>(input: &(impl Input<'py> + ?Sized), int: i64) -> ValResult<bool> {
    if int == 0 {
        Ok(false)
    } else if int == 1 {
        Ok(true)
    } else {
        Err(ValError::new(ErrorTypeDefaults::BoolParsing, input))
    }
}

/// Strip underscores from strings so that 1_000 can be parsed to 1000
/// Ignore any unicode stuff since this has to be digits and underscores
/// and if it's not subsequent parsing will just fail
fn strip_underscores(s: &str) -> Option<String> {
    // Leading and trailing underscores are not valid in Python (e.g. `int('__1__')` fails)
    // so we match that behavior here.
    // Double consecutive underscores are also not valid
    // If there are no underscores at all, no need to replace anything
    if s.starts_with('_') || s.ends_with('_') || !s.contains('_') || s.contains("__") {
        // no underscores to strip
        return None;
    }
    Some(s.replace('_', ""))
}

/// parse a string as an int
pub fn str_as_int<'py>(input: &(impl Input<'py> + ?Sized), str: &str) -> ValResult<EitherInt<'py>> {
    let str = str.trim();

    // we have to call `NumberInt::try_from` directly first so we fail fast if the string is too long
    match NumberInt::try_from(str.as_bytes()) {
        Ok(NumberInt::Int(i)) => return Ok(EitherInt::I64(i)),
        Ok(NumberInt::BigInt(i)) => return Ok(EitherInt::BigInt(i)),
        Err(e) => {
            if e.error_type == JsonErrorType::NumberOutOfRange {
                return Err(ValError::new(ErrorTypeDefaults::IntParsingSize, input));
            }
        }
    }

    if let Some(str_stripped) = strip_decimal_zeros(str) {
        _parse_str(input, str_stripped)
    } else if let Some(str_stripped) = strip_underscores(str) {
        _parse_str(input, &str_stripped)
    } else {
        Err(ValError::new(ErrorTypeDefaults::IntParsing, input))
    }
}

/// parse a float as a float
pub fn str_as_float<'py>(input: &(impl Input<'py> + ?Sized), str: &str) -> ValResult<EitherFloat<'py>> {
    match str.trim().parse() {
        Ok(float) => Ok(EitherFloat::F64(float)),
        Err(_) => match strip_underscores(str).and_then(|stripped| stripped.parse().ok()) {
            Some(float) => Ok(EitherFloat::F64(float)),
            None => Err(ValError::new(ErrorTypeDefaults::FloatParsing, input)),
        },
    }
}

/// parse a string as an int, `input` is required here to get lifetimes to match up
/// max length of the input is 4300 which is checked by jiter, see
/// https://docs.python.org/3/whatsnew/3.11.html#other-cpython-implementation-changes and
/// https://github.com/python/cpython/issues/95778 for more info in that length bound
fn _parse_str<'py>(input: &(impl Input<'py> + ?Sized), str: &str) -> ValResult<EitherInt<'py>> {
    match NumberInt::try_from(str.as_bytes()) {
        Ok(jiter::NumberInt::Int(i)) => Ok(EitherInt::I64(i)),
        Ok(jiter::NumberInt::BigInt(i)) => Ok(EitherInt::BigInt(i)),
        Err(e) => match e.error_type {
            JsonErrorType::NumberOutOfRange => Err(ValError::new(ErrorTypeDefaults::IntParsingSize, input)),
            _ => Err(ValError::new(ErrorTypeDefaults::IntParsing, input)),
        },
    }
}

/// we don't want to parse as f64 then call `float_as_int` as it can loose precision for large ints, therefore
/// we strip `.0+` manually instead, then parse as i64
fn strip_decimal_zeros(s: &str) -> Option<&str> {
    if let Some(i) = s.find('.') {
        if s[i + 1..].chars().all(|c| c == '0') {
            return Some(&s[..i]);
        }
    }
    None
}

pub fn float_as_int<'py>(input: &(impl Input<'py> + ?Sized), float: f64) -> ValResult<EitherInt<'py>> {
    if float.is_infinite() || float.is_nan() {
        Err(ValError::new(ErrorTypeDefaults::FiniteNumber, input))
    } else if float % 1.0 != 0.0 {
        Err(ValError::new(ErrorTypeDefaults::IntFromFloat, input))
    } else if (i64::MIN as f64) < float && float < (i64::MAX as f64) {
        Ok(EitherInt::I64(float as i64))
    } else {
        Err(ValError::new(ErrorTypeDefaults::IntParsingSize, input))
    }
}

pub fn decimal_as_int<'py>(
    input: &(impl Input<'py> + ?Sized),
    decimal: &Bound<'py, PyAny>,
) -> ValResult<EitherInt<'py>> {
    let py = decimal.py();
    if !decimal.call_method0(intern!(py, "is_finite"))?.extract::<bool>()? {
        return Err(ValError::new(ErrorTypeDefaults::FiniteNumber, input));
    }
    let (numerator, denominator) = decimal
        .call_method0(intern!(py, "as_integer_ratio"))?
        .extract::<(Bound<'_, PyAny>, Bound<'_, PyAny>)>()?;
    if denominator.extract::<i64>().map_or(true, |d| d != 1) {
        return Err(ValError::new(ErrorTypeDefaults::IntFromFloat, input));
    }
    Ok(EitherInt::Py(numerator))
}
