//! Errors with locations and exit codes (AC-I2), and the text/JSON output helper (AC-I1).

use serde_json::{json, Map, Value};
use std::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ErrKind {
    Validation,
    Lint,
    Usage,
    Io,
}

impl ErrKind {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrKind::Validation | ErrKind::Lint => 1,
            ErrKind::Usage => 2,
            ErrKind::Io => 3,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            ErrKind::Validation => "validation",
            ErrKind::Lint => "lint",
            ErrKind::Usage => "usage",
            ErrKind::Io => "io",
        }
    }
}

/// Where an error happened. `row`/`col` are 0-based pixel coordinates (row = y, col = x).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Loc {
    pub file: String,
    pub frame: Option<String>,
    pub row: Option<usize>,
    pub col: Option<usize>,
}

impl Loc {
    pub fn file(f: impl Into<String>) -> Loc {
        Loc { file: f.into(), ..Default::default() }
    }
    pub fn frame(mut self, f: impl Into<String>) -> Loc {
        self.frame = Some(f.into());
        self
    }
    pub fn at(mut self, row: usize, col: usize) -> Loc {
        self.row = Some(row);
        self.col = Some(col);
        self
    }
}

impl fmt::Display for Loc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.file)?;
        if let Some(fr) = &self.frame {
            write!(f, ", frame {}", fr)?;
        }
        if let Some(r) = self.row {
            write!(f, ", row {}", r)?;
        }
        if let Some(c) = self.col {
            write!(f, ", col {}", c)?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PxlError {
    pub kind: ErrKind,
    pub msg: String,
    pub loc: Option<Loc>,
    pub hint: Option<String>,
}

pub type Result<T> = std::result::Result<T, PxlError>;

impl PxlError {
    fn new(kind: ErrKind, msg: impl Into<String>) -> PxlError {
        PxlError { kind, msg: msg.into(), loc: None, hint: None }
    }
    pub fn validation(msg: impl Into<String>) -> PxlError {
        PxlError::new(ErrKind::Validation, msg)
    }
    pub fn lint(msg: impl Into<String>) -> PxlError {
        PxlError::new(ErrKind::Lint, msg)
    }
    pub fn usage(msg: impl Into<String>) -> PxlError {
        PxlError::new(ErrKind::Usage, msg)
    }
    pub fn io(msg: impl Into<String>) -> PxlError {
        PxlError::new(ErrKind::Io, msg)
    }
    /// Command or feature not written yet (usage error, exit 2).
    pub fn not_implemented(cmd: &str) -> PxlError {
        PxlError::usage(format!("`{}` is not implemented yet", cmd))
    }
    pub fn with_loc(mut self, loc: Loc) -> PxlError {
        self.loc = Some(loc);
        self
    }
    pub fn with_hint(mut self, hint: impl Into<String>) -> PxlError {
        self.hint = Some(hint.into());
        self
    }
    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("kind".into(), json!(self.kind.name()));
        m.insert("message".into(), json!(self.msg));
        if let Some(l) = &self.loc {
            m.insert(
                "loc".into(),
                json!({"file": l.file, "frame": l.frame, "row": l.row, "col": l.col}),
            );
        }
        if let Some(h) = &self.hint {
            m.insert("hint".into(), json!(h));
        }
        Value::Object(m)
    }
}

impl fmt::Display for PxlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.msg)?;
        if let Some(l) = &self.loc {
            write!(f, " ({})", l)?;
        }
        if let Some(h) = &self.hint {
            write!(f, "; {}", h)?;
        }
        Ok(())
    }
}

impl std::error::Error for PxlError {}

impl From<std::io::Error> for PxlError {
    fn from(e: std::io::Error) -> PxlError {
        PxlError::io(e.to_string())
    }
}
impl From<serde_json::Error> for PxlError {
    fn from(e: serde_json::Error) -> PxlError {
        PxlError::validation(format!("invalid JSON: {}", e))
    }
}
impl From<toml::de::Error> for PxlError {
    fn from(e: toml::de::Error) -> PxlError {
        PxlError::validation(format!("invalid TOML: {}", e.message()))
    }
}
impl From<anyhow::Error> for PxlError {
    fn from(e: anyhow::Error) -> PxlError {
        match e.downcast::<PxlError>() {
            Ok(p) => p,
            Err(e) => match e.downcast::<std::io::Error>() {
                Ok(io) => PxlError::from(io),
                Err(e) => PxlError::validation(format!("{:#}", e)),
            },
        }
    }
}

/// Collects a command's output. Text mode prints `line`s; JSON mode prints exactly one object built
/// from `set`/`emit` fields (plus `ok`), and drops the text. Warnings go to stderr (and to `warnings`
/// in JSON).
pub struct Out {
    pub json: bool,
    agent: Option<String>,
    text: String,
    fields: Map<String, Value>,
    warnings: Vec<String>,
}

impl Out {
    pub fn new(json: bool, agent: Option<String>) -> Out {
        Out { json, agent, text: String::new(), fields: Map::new(), warnings: Vec::new() }
    }
    /// The `--agent` identity, if given.
    pub fn agent(&self) -> Option<&str> {
        self.agent.as_deref()
    }
    /// Append one line of human text (ignored in JSON mode).
    pub fn line(&mut self, s: impl AsRef<str>) {
        if !self.json {
            self.text.push_str(s.as_ref());
            self.text.push('\n');
        }
    }
    /// Set a JSON field (ignored in text mode).
    pub fn set(&mut self, key: &str, v: Value) {
        if self.json {
            self.fields.insert(key.to_string(), v);
        }
    }
    /// Text and JSON in one call. An object `v` is merged into the result; anything else goes under "data".
    pub fn emit(&mut self, text: impl AsRef<str>, v: Value) {
        if self.json {
            match v {
                Value::Object(m) => {
                    for (k, x) in m {
                        self.fields.insert(k, x);
                    }
                }
                other => {
                    self.fields.insert("data".into(), other);
                }
            }
        } else {
            let t = text.as_ref();
            self.text.push_str(t);
            if !t.ends_with('\n') {
                self.text.push('\n');
            }
        }
    }
    pub fn warn(&mut self, s: impl Into<String>) {
        self.warnings.push(s.into());
    }
    /// Render the final stdout payload and stderr text, and return the exit code.
    pub fn render(&self, res: &Result<()>) -> (String, String, i32) {
        let stored = crate::versions::take_notes();
        let mut err = String::new();
        for w in &self.warnings {
            err.push_str(&format!("warning: {}\n", w));
        }
        if self.json {
            let mut m = Map::new();
            m.insert("ok".into(), json!(res.is_ok()));
            for (k, v) in &self.fields {
                m.insert(k.clone(), v.clone());
            }
            if !self.warnings.is_empty() {
                m.insert("warnings".into(), json!(self.warnings));
            }
            if !stored.is_empty() {
                m.insert("store".into(), json!(stored));
            }
            if let Err(e) = res {
                m.insert("error".into(), e.to_json());
            }
            let code = res.as_ref().err().map_or(0, |e| e.exit_code());
            (format!("{}\n", Value::Object(m)), err, code)
        } else {
            let mut o = self.text.clone();
            for s in &stored {
                o.push_str(s);
                o.push('\n');
            }
            let mut code = 0;
            if let Err(e) = res {
                err.push_str(&format!("error: {}\n", e.msg));
                if let Some(l) = &e.loc {
                    err.push_str(&format!("  at {}\n", l));
                }
                if let Some(h) = &e.hint {
                    err.push_str(&format!("  hint: {}\n", h));
                }
                code = e.exit_code();
            }
            (o, err, code)
        }
    }
    /// Print everything and return the process exit code.
    pub fn finish(self, res: Result<()>) -> i32 {
        use std::io::Write;
        let (o, e, code) = self.render(&res);
        let so = std::io::stdout();
        let _ = so.lock().write_all(o.as_bytes());
        let _ = std::io::stderr().write_all(e.as_bytes());
        code
    }
}
