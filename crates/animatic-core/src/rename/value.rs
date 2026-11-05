//! Variant-like values of the rename language.
//!
//! The legacy interpreter used Delphi Variants and its behaviour depends on the exact
//! variant type (e.g. string literals are WideStrings and therefore *falsy*). The types
//! are kept distinct so patterns written for AniDB O'Matic evaluate identically.

use std::cmp::Ordering;

use super::EvalError;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// varEmpty – identity of `+`.
    Empty,
    /// varInteger (truthy when non-zero; objects evaluate to this).
    Int(i64),
    /// varByte (`count`, falsy).
    Byte(i64),
    /// varInt64 (`strtoint`, Int64 properties; falsy).
    Int64(i64),
    /// varDouble (number literals, `/`).
    Double(f64),
    Bool(bool),
    /// varString / AnsiString (truthy when non-empty).
    AStr(String),
    /// varOleStr / WideString (always falsy).
    WStr(String),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Int(i) => *i != 0,
            Value::Double(d) => *d != 0.0,
            Value::Bool(b) => *b,
            Value::AStr(s) => !s.is_empty(),
            _ => false,
        }
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::AStr(_) | Value::WStr(_))
    }

    fn is_integer(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Byte(_) | Value::Int64(_))
    }

    fn int_value(&self) -> Option<i64> {
        match self {
            Value::Int(i) | Value::Byte(i) | Value::Int64(i) => Some(*i),
            _ => None,
        }
    }

    /// `VarToWStr` – conversion used for the final result and for `+` casts.
    pub fn to_text(&self) -> String {
        match self {
            Value::Empty => String::new(),
            Value::Int(i) | Value::Byte(i) | Value::Int64(i) => i.to_string(),
            Value::Double(d) => float_to_str(*d),
            Value::Bool(b) => if *b { "True" } else { "False" }.to_string(),
            Value::AStr(s) | Value::WStr(s) => s.clone(),
        }
    }

    /// Function argument conversion (`ToString` of the legacy evaluator): doubles and empty
    /// values are not convertible.
    pub fn to_arg_string(&self) -> Result<String, String> {
        match self {
            Value::Bool(b) => Ok(if *b { "True" } else { "False" }.to_string()),
            Value::Int(i) | Value::Byte(i) | Value::Int64(i) => Ok(i.to_string()),
            Value::AStr(s) | Value::WStr(s) => Ok(s.clone()),
            Value::Double(_) | Value::Empty => {
                Err("PARSERUN: Syntax error, value can not be converted to a string.".to_string())
            }
        }
    }

    /// Function argument conversion (`ToInt`).
    pub fn to_arg_int(&self) -> Result<i64, String> {
        match self {
            Value::Bool(b) => Ok(i64::from(*b)),
            Value::Int(i) | Value::Byte(i) | Value::Int64(i) => Ok(*i),
            Value::Double(d) => Ok(d.round_ties_even() as i64),
            Value::AStr(s) | Value::WStr(s) => Ok(str_to_int_def(s, 0)),
            Value::Empty => {
                Err("PARSERUN: Syntax error, value can not be converted to an integer.".to_string())
            }
        }
    }

    fn to_double(&self) -> Result<f64, EvalError> {
        match self {
            Value::Empty => Ok(0.0),
            Value::Int(i) | Value::Byte(i) | Value::Int64(i) => Ok(*i as f64),
            Value::Double(d) => Ok(*d),
            Value::Bool(b) => Ok(if *b { -1.0 } else { 0.0 }),
            Value::AStr(s) | Value::WStr(s) => s.trim().parse::<f64>().map_err(|_| {
                EvalError::new(format!(
                    "Could not convert variant of type (String) into type (Double): '{s}'"
                ))
            }),
        }
    }
}

/// Delphi `StrToIntDef` (accepts `$hex`).
pub fn str_to_int_def(s: &str, default: i64) -> i64 {
    let t = s.trim();
    if let Some(h) = t.strip_prefix('$') {
        return i64::from_str_radix(h, 16).unwrap_or(default);
    }
    t.parse().unwrap_or(default)
}

/// Delphi `FloatToStr` (15 significant digits, general format).
pub fn float_to_str(d: f64) -> String {
    if d.is_nan() {
        return "NAN".into();
    }
    if d.is_infinite() {
        return if d > 0.0 { "INF".into() } else { "-INF".into() };
    }
    if d == d.trunc() && d.abs() < 1e15 {
        return format!("{}", d as i64);
    }
    let exp = d.abs().log10().floor() as i32;
    if (-5..15).contains(&exp) {
        let decimals = (14 - exp).max(0) as usize;
        let s = format!("{d:.decimals$}");
        let s = s.trim_end_matches('0').trim_end_matches('.');
        s.to_string()
    } else {
        let s = format!("{d:.14E}");
        let (m, e) = s.split_once('E').unwrap_or((&s, "0"));
        let m = m.trim_end_matches('0').trim_end_matches('.');
        let e: i32 = e.parse().unwrap_or(0);
        format!("{m}E{}{:02}", if e < 0 { "-" } else { "" }, e.abs())
    }
}

/// `+` of the expression evaluator: string casts first, then `VarAdd`.
pub fn op_add(l: Value, r: Value) -> Result<Value, EvalError> {
    match (&l, &r) {
        (Value::AStr(_), _) | (_, Value::AStr(_)) => Ok(Value::AStr(l.to_text() + &r.to_text())),
        (Value::WStr(_), _) | (_, Value::WStr(_)) => Ok(Value::WStr(l.to_text() + &r.to_text())),
        _ => var_add(l, r),
    }
}

/// Raw `VarAdd` (used by `for` and user functions): strings concatenate, numeric strings
/// mixed with numbers are converted to numbers.
pub fn var_add(l: Value, r: Value) -> Result<Value, EvalError> {
    match (&l, &r) {
        (Value::Empty, _) => Ok(r),
        (_, Value::Empty) => Ok(l),
        (a, b) if a.is_string() && b.is_string() => {
            let s = a.to_text() + &b.to_text();
            Ok(
                if matches!(a, Value::AStr(_)) || matches!(b, Value::AStr(_)) {
                    Value::AStr(s)
                } else {
                    Value::WStr(s)
                },
            )
        }
        _ => arith(l, r, '+'),
    }
}

/// `-`, `*`, numeric `+`: integers stay integers, anything else becomes a double.
pub fn arith(l: Value, r: Value, op: char) -> Result<Value, EvalError> {
    let int_like =
        |v: &Value| v.is_integer() || matches!(v, Value::Bool(_)) || matches!(v, Value::Empty);
    if int_like(&l) && int_like(&r) {
        let a = match l {
            Value::Bool(b) => -i64::from(b),
            Value::Empty => 0,
            ref v => v.int_value().unwrap_or(0),
        };
        let b = match r {
            Value::Bool(b) => -i64::from(b),
            Value::Empty => 0,
            ref v => v.int_value().unwrap_or(0),
        };
        let v = match op {
            '+' => a.wrapping_add(b),
            '-' => a.wrapping_sub(b),
            _ => a.wrapping_mul(b),
        };
        let wide = matches!(l, Value::Int64(_)) || matches!(r, Value::Int64(_));
        return Ok(if wide { Value::Int64(v) } else { Value::Int(v) });
    }
    let a = l.to_double()?;
    let b = r.to_double()?;
    Ok(Value::Double(match op {
        '+' => a + b,
        '-' => a - b,
        _ => a * b,
    }))
}

pub fn divide(l: Value, r: Value) -> Result<Value, EvalError> {
    let b = r.to_double()?;
    if b == 0.0 {
        return Err(EvalError::new("Division by zero".into()));
    }
    Ok(Value::Double(l.to_double()? / b))
}

pub fn int_div(l: Value, r: Value, modulo: bool) -> Result<Value, EvalError> {
    let a = l.to_double()?.round_ties_even() as i64;
    let b = r.to_double()?.round_ties_even() as i64;
    if b == 0 {
        return Err(EvalError::new("Division by zero".into()));
    }
    Ok(Value::Int(if modulo { a % b } else { a / b }))
}

fn parse_bool(s: &str) -> Result<bool, EvalError> {
    let t = s.trim();
    if t.eq_ignore_ascii_case("true") {
        return Ok(true);
    }
    if t.eq_ignore_ascii_case("false") {
        return Ok(false);
    }
    t.parse::<f64>().map(|v| v != 0.0).map_err(|_| {
        EvalError::new(format!(
            "Could not convert variant of type (String) into type (Boolean): '{s}'"
        ))
    })
}

/// `VarCmp` of the legacy runtime.
pub fn compare(l: &Value, r: &Value) -> Result<Ordering, EvalError> {
    let bool_int = |b: bool| if b { -1i64 } else { 0 };
    match (l, r) {
        (a, b) if a.is_integer() && b.is_integer() => Ok(a.int_value().cmp(&b.int_value())),
        (a, b) if a.is_string() && b.is_string() => Ok(a.to_text().cmp(&b.to_text())),
        (Value::Bool(a), Value::Bool(b)) => Ok(bool_int(*a).cmp(&bool_int(*b))),
        (Value::Bool(a), s) if s.is_string() => {
            Ok(bool_int(*a).cmp(&bool_int(parse_bool(&s.to_text())?)))
        }
        (s, Value::Bool(b)) if s.is_string() => {
            Ok(bool_int(parse_bool(&s.to_text())?).cmp(&bool_int(*b)))
        }
        (Value::Bool(a), i) if i.is_integer() => Ok(bool_int(*a).cmp(&i.int_value().unwrap_or(0))),
        (i, Value::Bool(b)) if i.is_integer() => Ok(i.int_value().unwrap_or(0).cmp(&bool_int(*b))),
        (Value::Empty, Value::Empty) => Ok(Ordering::Equal),
        (Value::Empty, s) if s.is_string() => Ok(String::new().cmp(&s.to_text())),
        (s, Value::Empty) if s.is_string() => Ok(s.to_text().cmp(&String::new())),
        _ => {
            let a = l.to_double()?;
            let b = r.to_double()?;
            Ok(a.partial_cmp(&b).unwrap_or(Ordering::Equal))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn float_formatting_matches_delphi_general_format() {
        assert_eq!(float_to_str(2.0), "2");
        assert_eq!(float_to_str(1.5), "1.5");
        assert_eq!(float_to_str(-3.0), "-3");
        assert_eq!(float_to_str(1.0 / 3.0), "0.333333333333333");
        assert_eq!(float_to_str(1e20), "1E20");
    }

    #[test]
    fn truthiness_follows_variant_types() {
        assert!(Value::Int(1).truthy());
        assert!(!Value::WStr("abc".into()).truthy());
        assert!(Value::AStr("abc".into()).truthy());
        assert!(!Value::Int64(5).truthy());
        assert!(!Value::Byte(5).truthy());
        assert!(Value::Double(0.5).truthy());
    }

    #[test]
    fn plus_prefers_strings() {
        assert_eq!(
            op_add(Value::WStr("a".into()), Value::Int(3)).unwrap(),
            Value::WStr("a3".into())
        );
        assert_eq!(
            op_add(Value::AStr(String::new()), Value::Double(5.0)).unwrap(),
            Value::AStr("5".into())
        );
        assert_eq!(
            op_add(Value::Int(1), Value::Double(2.0)).unwrap(),
            Value::Double(3.0)
        );
        assert!(var_add(Value::AStr(String::new()), Value::Int(5)).is_err());
    }

    #[test]
    fn comparisons_coerce_like_varcmp() {
        assert_eq!(
            compare(&Value::Double(1.0), &Value::WStr("1".into())).unwrap(),
            Ordering::Equal
        );
        assert_eq!(
            compare(&Value::WStr("10".into()), &Value::WStr("9".into())).unwrap(),
            Ordering::Less
        );
        assert!(compare(&Value::Double(1.0), &Value::WStr("x".into())).is_err());
        assert_eq!(
            compare(&Value::Bool(false), &Value::Double(1.0)).unwrap(),
            Ordering::Less
        );
    }
}
