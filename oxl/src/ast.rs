use crate::diag::Span;

#[derive(Debug)]
pub struct Crate {
    pub items: Vec<Item>,
}

#[derive(Debug)]
pub enum Item {
    Fn(Fn),
    Struct(Struct),
    Extern(Fn),
    Interface(Interface),
    Import(Import),
    Global(Global),
}

/// `import "path" [as alias] [.{name, ...}]`
/// canonical module name = file stem of the resolved path; `alias` binds a
/// different local name; `selective` names also bind unqualified.
#[derive(Debug)]
pub struct Import {
    pub path: String,
    pub alias: Option<String>,
    pub selective: Vec<String>,
    pub span: Span,
}

/// qualified call name: `f` / `mod.f` (Field{obj:Ident} flattens to "mod.f")
pub fn callee_name(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(n) => Some(n.clone()),
        ExprKind::Field { obj, name } => Some(format!("{}.{}", callee_name(obj)?, name)),
        _ => None,
    }
}

/// top-level `let name[: ty] = init` — process-wide state, initialized
/// in oxl_main prologue (order: before user main runs)
#[derive(Debug)]
pub struct Global {
    pub name: String,
    pub ty: Option<TyRef>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Debug)]
pub struct Interface {
    pub name: String,
    /// constraint surface: ops the type must support. v0: fixed vocab —
    /// "eq" (==), "ordered" (< >), "add" (+)
    pub ops: Vec<String>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Fn {
    pub name: String,
    pub ty_params: Vec<(String, String)>, // [(T, constraint)] — "" = any
    pub params: Vec<Param>,
    pub ret: RetTy,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Debug)]
pub struct Param {
    pub name: String,
    pub ty: TyRef,
    pub span: Span,
}

#[derive(Debug)]
pub struct Struct {
    pub name: String,
    pub fields: Vec<Param>,
    pub span: Span,
}

#[derive(Debug)]
pub enum RetTy {
    Void,
    One(TyRef),
    Pair(TyRef), // -> [T, err]
}

#[derive(Debug)]
pub struct TyRef {
    pub kind: TyKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum TyKind {
    Named(String),
    Own(Box<TyRef>),
    Ref(Box<TyRef>),
    Mut(Box<TyRef>),
    Chan,
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug)]
pub enum Bind {
    Name(String),
    Blank,
}

#[derive(Debug)]
pub enum Stmt {
    Let { name: String, ty: Option<TyRef>, init: Expr, span: Span },
    Let2 { a: Bind, b: Bind, call: Expr, span: Span },
    Assign { target: Expr, val: Expr, span: Span },
    Expr(Expr),
    Return { vals: Vec<Expr>, span: Span },
    If { cond: Expr, then: Block, els: Option<Block>, span: Span },
    For { cond: Expr, body: Block, span: Span },
    Scope { body: Block, span: Span },
    Spawn { call: Expr, pinned: bool, blocking: bool, span: Span },
    Defer { call: Expr, span: Span },
    Unsafe { body: Block, span: Span },
    Break { span: Span },
    Continue { span: Span },
}

#[derive(Clone, Debug)]
pub struct Expr {
    pub kind: ExprKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum ExprKind {
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Nil,
    Ident(String),
    Call { f: Box<Expr>, args: Vec<Expr> },
    Field { obj: Box<Expr>, name: String },
    StructLit { name: String, fields: Vec<(String, Expr)> },
    Bin { op: BinOp, l: Box<Expr>, r: Box<Expr> },
    Un { op: UnOp, e: Box<Expr> },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}
