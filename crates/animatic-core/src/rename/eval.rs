//! Evaluator of the rename language (built-in and application functions, objects).

use std::cmp::Ordering;

use super::EvalError;
use super::parser::{BinOp, Node};
use super::value::{self, Value};

/// A bound object (anime, episode, file, group, local hash, mylist entry) exposing
/// published properties by (case-insensitive) name.
#[derive(Debug, Clone, Default)]
pub struct Object {
    pub class: &'static str,
    pub props: Vec<(&'static str, Value)>,
}

impl Object {
    pub fn new(class: &'static str) -> Object {
        Object {
            class,
            props: Vec::new(),
        }
    }

    pub fn with(mut self, name: &'static str, v: Value) -> Object {
        self.props.push((name, v));
        self
    }

    pub fn prop(&self, name: &str) -> Option<&Value> {
        self.props
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn id(&self) -> i64 {
        match self.prop("Id") {
            Some(Value::Int(i)) => *i,
            _ => 0,
        }
    }
}

/// Database access needed by the application functions.
pub trait DataSource {
    /// Anime object for `getanime`.
    fn anime(&self, aid: i64) -> Option<Object>;
    /// Episode object for `getepisode`.
    fn episode(&self, eid: i64) -> Option<Object>;
    /// Resolve a language argument (numeric legacy id, name or code) to a language code.
    fn language(&self, lang: &str) -> Option<String>;
    /// Anime titles as (type, language code, name) in storage order.
    fn anime_titles(&self, aid: i64) -> Vec<(i64, String, String)>;
    /// Episode titles as (language code, name) in storage order.
    fn episode_titles(&self, eid: i64) -> Vec<(String, String)>;
}

struct UserFunction {
    name: String,
    bodies: Vec<Node>,
}

pub struct Evaluator<'a> {
    source: &'a dyn DataSource,
    variables: Vec<(String, Value)>,
    objects: Vec<(String, Option<Object>)>,
    functions: Vec<UserFunction>,
    /// Soft (non fatal) errors, as the legacy evaluator's error list.
    pub errors: Vec<String>,
}

impl<'a> Evaluator<'a> {
    pub fn new(source: &'a dyn DataSource) -> Self {
        Evaluator {
            source,
            variables: Vec::new(),
            objects: Vec::new(),
            functions: Vec::new(),
            errors: Vec::new(),
        }
    }

    pub fn set_variable(&mut self, name: &str, v: Value) {
        if let Some(slot) = self
            .variables
            .iter_mut()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
        {
            slot.1 = v;
        } else {
            self.variables.push((name.to_string(), v));
        }
    }

    pub fn variables(&self) -> &[(String, Value)] {
        &self.variables
    }

    pub fn objects(&self) -> &[(String, Option<Object>)] {
        &self.objects
    }

    fn variable(&self, name: &str) -> Option<&Value> {
        self.variables
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    /// Bind an object. The first binding of a name wins (later ones are ignored).
    pub fn set_object(&mut self, name: &str, obj: Option<Object>) {
        self.objects.push((name.to_string(), obj));
    }

    fn object(&self, name: &str) -> Option<&Object> {
        self.objects
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .and_then(|(_, o)| o.as_ref())
    }

    fn default_object(&self) -> Option<&Object> {
        self.object("self")
            .or_else(|| self.objects.first().and_then(|(_, o)| o.as_ref()))
    }

    pub fn run(&mut self, tree: Option<&Node>) -> Result<Value, EvalError> {
        self.errors.clear();
        match tree {
            None => Ok(Value::Bool(false)),
            Some(n) => self.eval(n),
        }
    }

    fn soft(&mut self, msg: String) {
        self.errors.push(msg);
    }

    fn arg_string(&mut self, v: &Value) -> String {
        match v.to_arg_string() {
            Ok(s) => s,
            Err(e) => {
                self.soft(e);
                String::new()
            }
        }
    }

    fn arg_int(&mut self, v: &Value) -> i64 {
        match v.to_arg_int() {
            Ok(i) => i,
            Err(e) => {
                self.soft(e);
                0
            }
        }
    }

    fn eval(&mut self, n: &Node) -> Result<Value, EvalError> {
        match n {
            Node::Literal(v) => Ok(v.clone()),
            Node::Not(r) => {
                let v = self.eval(r)?;
                Ok(Value::Bool(!v.truthy()))
            }
            Node::Binary(op, l, r) => self.binary(*op, l.as_deref(), r),
            Node::Symbol { parent, name } => self.symbol(parent.as_deref(), name),
            Node::Call { name, args, .. } => self.call(name, args),
        }
    }

    fn binary(&mut self, op: BinOp, l: Option<&Node>, r: &Node) -> Result<Value, EvalError> {
        let left = match l {
            Some(n) => self.eval(n)?,
            None => Value::Bool(false),
        };
        match op {
            BinOp::And => {
                if !left.truthy() {
                    return Ok(Value::Bool(false));
                }
                return Ok(Value::Bool(self.eval(r)?.truthy()));
            }
            BinOp::Or => {
                if left.truthy() {
                    return Ok(Value::Bool(true));
                }
                return Ok(Value::Bool(self.eval(r)?.truthy()));
            }
            _ => {}
        }
        let right = self.eval(r)?;
        let cmp = |o: Ordering, want: &[Ordering]| Value::Bool(want.contains(&o));
        Ok(match op {
            BinOp::Xor => Value::Bool(left.truthy() != right.truthy()),
            BinOp::Add => value::op_add(left, right)?,
            BinOp::Sub => value::arith(left, right, '-')?,
            BinOp::Mul => value::arith(left, right, '*')?,
            BinOp::Div => value::divide(left, right)?,
            BinOp::IntDiv => value::int_div(left, right, false)?,
            BinOp::Mod => value::int_div(left, right, true)?,
            BinOp::Eq => cmp(value::compare(&left, &right)?, &[Ordering::Equal]),
            BinOp::Ne => cmp(
                value::compare(&left, &right)?,
                &[Ordering::Less, Ordering::Greater],
            ),
            BinOp::Lt => cmp(value::compare(&left, &right)?, &[Ordering::Less]),
            BinOp::Le => cmp(
                value::compare(&left, &right)?,
                &[Ordering::Less, Ordering::Equal],
            ),
            BinOp::Gt => cmp(value::compare(&left, &right)?, &[Ordering::Greater]),
            BinOp::Ge => cmp(
                value::compare(&left, &right)?,
                &[Ordering::Greater, Ordering::Equal],
            ),
            BinOp::And | BinOp::Or => unreachable!(),
        })
    }

    /// Object a node designates when used as a property parent.
    fn object_of(&self, n: &Node) -> Option<Object> {
        match n {
            Node::Symbol { parent: None, name } => self.object(name).cloned(),
            _ => None,
        }
    }

    fn symbol(&mut self, parent: Option<&Node>, name: &str) -> Result<Value, EvalError> {
        match parent {
            None => {
                if let Some(v) = self.variable(name) {
                    return Ok(v.clone());
                }
                if self.object(name).is_some() {
                    // Objects evaluate to a non-zero integer (their address in the original).
                    return Ok(Value::Int(1));
                }
                match self.default_object() {
                    Some(o) => Ok(o.prop(name).cloned().unwrap_or(Value::Bool(false))),
                    None => Err(EvalError::new(format!(
                        "TRuleEvaluator.TRuleEvaluatorNodeObjectValue.ExecuteNode: no default object for {name}!"
                    ))),
                }
            }
            Some(p) => match self.object_of(p) {
                Some(o) => Ok(o.prop(name).cloned().unwrap_or(Value::Bool(false))),
                None => Err(EvalError::new(format!(
                    "TRuleEvaluator.TRuleEvaluatorNodeObjectValue.ExecuteNode: Attribute {name} parent object not found!"
                ))),
            },
        }
    }

    fn eval_args(&mut self, args: &[Node]) -> Result<Vec<Value>, EvalError> {
        args.iter().map(|a| self.eval(a)).collect()
    }

    fn count_error(&mut self, func: &str, text: &str, n: usize) {
        self.soft(format!(
            "PARSERUN: Function {func}: {text}, found {n} parameters."
        ));
    }

    fn call(&mut self, name: &str, args: &[Node]) -> Result<Value, EvalError> {
        let lname = name.to_ascii_lowercase();
        let n = args.len();
        let empty_a = Value::AStr(String::new());
        match lname.as_str() {
            "if" => {
                if !(2..=3).contains(&n) {
                    self.count_error("if", "If can only have 2 or 3 parameters", n);
                    return Ok(empty_a);
                }
                let c = self.eval(&args[0])?;
                if c.truthy() {
                    self.eval(&args[1])
                } else if n == 3 {
                    self.eval(&args[2])
                } else {
                    Ok(empty_a)
                }
            }
            "in" => {
                if n < 2 {
                    self.count_error("in", "In must have at least 2 parameters", n);
                    return Ok(Value::Bool(false));
                }
                let x = self.eval(&args[0])?;
                for a in &args[1..] {
                    let v = self.eval(a)?;
                    if value::compare(&x, &v)? == Ordering::Equal {
                        return Ok(Value::Bool(true));
                    }
                }
                Ok(Value::Bool(false))
            }
            "contains" => {
                if n != 2 {
                    self.count_error("contains", "Contains must have 2 parameters", n);
                    return Ok(Value::Bool(false));
                }
                let v = self.eval_args(args)?;
                let needle = self.arg_string(&v[0]);
                let hay = self.arg_string(&v[1]);
                Ok(Value::Bool(!needle.is_empty() && hay.contains(&needle)))
            }
            "join" => {
                if n < 1 {
                    self.count_error("join", "Join must have at least 1 parameter", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let sep = self.arg_string(&v[0]);
                let mut parts = Vec::new();
                for x in &v[1..] {
                    let s = self.arg_string(x);
                    if !s.is_empty() {
                        parts.push(s);
                    }
                }
                if parts.is_empty() {
                    Ok(empty_a)
                } else {
                    Ok(Value::WStr(parts.join(&sep)))
                }
            }
            "lc" | "uc" => {
                if n != 1 {
                    let cap = if lname == "lc" { "Lc" } else { "Uc" };
                    self.count_error(&lname, &format!("{cap} must have 1 parameter"), n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                Ok(Value::AStr(if lname == "lc" {
                    s.to_ascii_lowercase()
                } else {
                    s.to_ascii_uppercase()
                }))
            }
            "length" => {
                if n != 1 {
                    self.count_error("length", "Length must have 1 parameter", n);
                    return Ok(Value::Byte(0));
                }
                let v = self.eval(&args[0])?;
                match v.to_arg_string() {
                    Ok(s) => Ok(Value::Int(s.encode_utf16().count() as i64)),
                    Err(e) => {
                        self.soft(e);
                        Ok(Value::Byte(0))
                    }
                }
            }
            "limit" => {
                if !(2..=3).contains(&n) {
                    self.count_error("limit", "Limit must have 2 or 3 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let s = self.arg_string(&v[0]);
                let max = self.arg_int(&v[1]);
                let suffix = if n == 3 {
                    self.arg_string(&v[2])
                } else {
                    String::new()
                };
                let chars: Vec<char> = s.chars().collect();
                if (chars.len() as i64) > max {
                    let keep = (max - suffix.chars().count() as i64).max(0) as usize;
                    Ok(Value::WStr(
                        chars[..keep.min(chars.len())].iter().collect::<String>() + &suffix,
                    ))
                } else {
                    Ok(Value::WStr(s))
                }
            }
            "max" | "min" => {
                if n < 2 {
                    self.count_error("max", "Max must have at least 2 parameters", n);
                    return Ok(Value::Int(0));
                }
                let v = self.eval_args(args)?;
                let mut acc: Option<i64> = None;
                for x in &v {
                    let i = self.arg_int(x);
                    acc = Some(match acc {
                        None => i,
                        Some(a) if lname == "max" => a.max(i),
                        Some(a) => a.min(i),
                    });
                }
                Ok(Value::Int(acc.unwrap_or(0)))
            }
            "numeric" => {
                if n != 1 {
                    self.count_error("numeric", "Numeric must have 1 parameter", n);
                    return Ok(Value::Bool(true));
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                if s.is_empty() {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(
                    s.chars()
                        .enumerate()
                        .all(|(i, c)| c.is_ascii_digit() || (i == 0 && c == '-')),
                ))
            }
            "pad" | "padl" | "padr" => {
                if !(2..=3).contains(&n) {
                    self.count_error(&lname, &format!("{lname} can only have 3 parameters"), n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let s = self.arg_string(&v[0]);
                let width = self.arg_int(&v[1]);
                let fill = if n == 3 {
                    self.arg_string(&v[2]).chars().next().unwrap_or(' ')
                } else {
                    ' '
                };
                let len = s.chars().count() as i64;
                if len >= width {
                    return Ok(Value::WStr(s));
                }
                let missing = (width - len) as usize;
                let rep = |k: usize| std::iter::repeat_n(fill, k).collect::<String>();
                Ok(Value::WStr(match lname.as_str() {
                    "padl" => rep(missing) + &s,
                    "padr" => s + &rep(missing),
                    _ => rep(missing / 2) + &s + &rep(missing - missing / 2),
                }))
            }
            "replace" | "replacei" | "replacerepeat" | "replacerepeati" => {
                if !(2..=3).contains(&n) {
                    self.count_error(&lname, "Replace must have 2 or 3 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let s = self.arg_string(&v[0]);
                let f = self.arg_string(&v[1]);
                let r = if n == 3 {
                    self.arg_string(&v[2])
                } else {
                    String::new()
                };
                let ignore_case = lname.ends_with('i');
                Ok(Value::AStr(string_replace(&s, &f, &r, ignore_case)))
            }
            "sc" => {
                if n != 1 {
                    self.count_error("sc", "Sc must have 1 parameter", n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                Ok(Value::WStr(sentence_case(&s)))
            }
            "tc" => {
                if n != 1 {
                    self.count_error("tc", "Tc must have 1 parameter", n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                Ok(Value::WStr(
                    s.to_lowercase()
                        .split(' ')
                        .map(sentence_case)
                        .collect::<Vec<_>>()
                        .join(" "),
                ))
            }
            "set" => {
                if n != 2 {
                    self.count_error("set", "Set must have 2 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let var = self.arg_string(&v[0]);
                if !var.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
                    self.soft(format!("PARSERUN: Function set: Set first parameter must be a valid identifier, found \"{var}\"."));
                    return Ok(empty_a);
                }
                self.set_variable(&var, v[1].clone());
                Ok(empty_a)
            }
            "split" => {
                if n != 3 {
                    self.count_error("split", "Split must have 3 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let s = self.arg_string(&v[0]);
                let d = self.arg_string(&v[1]);
                let i = self.arg_int(&v[2]);
                let parts: Vec<&str> = if d.is_empty() {
                    vec![s.as_str()]
                } else {
                    s.split(d.as_str()).collect()
                };
                match usize::try_from(i - 1).ok().and_then(|k| parts.get(k)) {
                    Some(p) if !p.is_empty() => Ok(Value::WStr((*p).to_string())),
                    _ => Ok(empty_a),
                }
            }
            "start" | "starti" => {
                if n != 2 {
                    self.count_error(&lname, "Start must have 2 parameters", n);
                    return Ok(Value::Bool(false));
                }
                let v = self.eval_args(args)?;
                let s = self.arg_string(&v[0]);
                let p = self.arg_string(&v[1]);
                Ok(Value::Bool(if lname == "start" {
                    s.starts_with(&p)
                } else {
                    s.to_lowercase().starts_with(&p.to_lowercase())
                }))
            }
            "strtoint" => {
                if n != 1 {
                    self.count_error("strtoint", "Strtoint can only have 1 parameter", n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                Ok(Value::Int64(value::str_to_int_def(&s, 0)))
            }
            "substr" | "copy" => {
                if !(2..=3).contains(&n) {
                    self.count_error(&lname, "Substr must have 2 or 3 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let s: Vec<char> = self.arg_string(&v[0]).chars().collect();
                let start = self.arg_int(&v[1]);
                let len = s.len() as i64;
                if start < 1 || start > len {
                    return Ok(empty_a);
                }
                let count = if n == 3 { self.arg_int(&v[2]) } else { len };
                let take = count.min(len - start + 1).max(0) as usize;
                let from = (start - 1) as usize;
                Ok(Value::WStr(s[from..from + take].iter().collect()))
            }
            "trim" => {
                if n != 1 {
                    self.count_error("trim", "Trim must have 1 parameter", n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let s = self.arg_string(&v);
                Ok(Value::WStr(s.trim_matches(|c: char| c <= ' ').to_string()))
            }
            "count" => {
                if n != 2 {
                    self.count_error("count", "Count can only have 2 parameters", n);
                    return Ok(Value::Byte(0));
                }
                let v = self.eval_args(args)?;
                let s: Vec<char> = self.arg_string(&v[0]).chars().collect();
                let sub: Vec<char> = self.arg_string(&v[1]).chars().collect();
                if sub.is_empty() || sub.len() > s.len() {
                    return Ok(Value::Byte(0));
                }
                let hits = (0..=s.len() - sub.len())
                    .filter(|&i| s[i..i + sub.len()] == sub[..])
                    .count();
                Ok(Value::Byte(hits as i64))
            }
            "for" => {
                if n != 4 {
                    self.count_error("for", "For can only have 4 parameters", n);
                    return Ok(empty_a);
                }
                let var_v = self.eval(&args[0])?;
                let var = self.arg_string(&var_v);
                let from_v = self.eval(&args[1])?;
                let to_v = self.eval(&args[2])?;
                let from = self.arg_int(&from_v);
                let to = self.arg_int(&to_v);
                let mut acc = empty_a;
                let mut i = from;
                while i <= to {
                    self.set_variable(&var, Value::Int(i));
                    let body = self.eval(&args[3])?;
                    acc = value::var_add(acc, body)?;
                    i += 1;
                }
                Ok(acc)
            }
            "function" => {
                if n < 2 {
                    self.count_error("function", "Function can only have 2 parameters", n);
                    return Ok(empty_a);
                }
                let nv = self.eval(&args[0])?;
                let fname = self.arg_string(&nv);
                if !self
                    .functions
                    .iter()
                    .any(|f| f.name.eq_ignore_ascii_case(&fname))
                {
                    self.functions.push(UserFunction {
                        name: fname,
                        bodies: args[1..].to_vec(),
                    });
                }
                Ok(empty_a)
            }
            "at" | "et" => {
                if n != 1 {
                    let cap = if lname == "at" { "At" } else { "Et" };
                    self.count_error(&lname, &format!("{cap} must have 1 parameter"), n);
                    return Ok(empty_a);
                }
                let v = self.eval(&args[0])?;
                let lang = self.arg_string(&v);
                let obj_name = if lname == "at" { "A" } else { "E" };
                let Some(id) = self.object(obj_name).map(Object::id) else {
                    return Err(EvalError::new(format!(
                        "Function {lname}: object {obj_name} is not available"
                    )));
                };
                Ok(Value::WStr(if lname == "at" {
                    self.anime_title(id, &lang)
                } else {
                    self.episode_title(id, &lang)
                }))
            }
            "atr" | "etr" => {
                if n != 2 {
                    self.count_error(&lname, "At must have 2 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let id = self.arg_int(&v[0]);
                let lang = self.arg_string(&v[1]);
                Ok(Value::WStr(if lname == "atr" {
                    self.anime_title(id, &lang)
                } else {
                    self.episode_title(id, &lang)
                }))
            }
            "getanime" | "getepisode" => {
                if n != 2 {
                    self.count_error("getaanime", "Getanime must have 2 parameters", n);
                    return Ok(empty_a);
                }
                let v = self.eval_args(args)?;
                let oname = self.arg_string(&v[0]);
                let id = self.arg_int(&v[1]);
                let obj = if lname == "getanime" {
                    self.source.anime(id)
                } else {
                    self.source.episode(id)
                };
                self.set_object(&oname, obj);
                Ok(empty_a)
            }
            _ => self.user_call(name, args),
        }
    }

    fn user_call(&mut self, name: &str, args: &[Node]) -> Result<Value, EvalError> {
        let Some(idx) = self
            .functions
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name))
        else {
            return Ok(Value::Bool(false));
        };
        let vals = self.eval_args(args)?;
        for (i, v) in vals.iter().enumerate() {
            let s = self.arg_string(v);
            self.set_variable(&format!("param{}", i + 1), Value::WStr(s));
        }
        let bodies = self.functions[idx].bodies.clone();
        let mut acc = Value::Empty;
        for b in &bodies {
            let v = self.eval(b)?;
            acc = value::var_add(acc, v)?;
        }
        Ok(acc)
    }

    fn anime_title(&self, aid: i64, lang: &str) -> String {
        let Some(code) = self.source.language(lang) else {
            return String::new();
        };
        let mut main = None;
        for (kind, l, name) in self.source.anime_titles(aid) {
            if l != code {
                continue;
            }
            if kind == 4 {
                return name;
            }
            if kind == 1 {
                main = Some(name);
            }
        }
        main.unwrap_or_default()
    }

    fn episode_title(&self, eid: i64, lang: &str) -> String {
        let Some(code) = self.source.language(lang) else {
            return String::new();
        };
        self.source
            .episode_titles(eid)
            .into_iter()
            .find(|(l, _)| *l == code)
            .map(|(_, n)| n)
            .unwrap_or_default()
    }
}

/// `StringReplace(s, f, r, [rfReplaceAll] (+ rfIgnoreCase))`.
fn string_replace(s: &str, f: &str, r: &str, ignore_case: bool) -> String {
    if f.is_empty() {
        return s.to_string();
    }
    if !ignore_case {
        return s.replace(f, r);
    }
    let hay: Vec<char> = s.chars().collect();
    let pat: Vec<char> = f.to_lowercase().chars().collect();
    let lower: Vec<char> = s
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < hay.len() {
        if i + pat.len() <= hay.len() && lower[i..i + pat.len()] == pat[..] {
            out.push_str(r);
            i += pat.len();
        } else {
            out.push(hay[i]);
            i += 1;
        }
    }
    out
}

fn sentence_case(s: &str) -> String {
    let lower = s.to_lowercase();
    let mut chars = lower.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
