use crate::diag::Span;

#[derive(Clone, Debug)]
pub struct Crate {
    /// `package <ident>` — mandatory first item of every file
    pub pkg: String,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Fn(Fn),
    Struct(Struct),
    Enum(EnumDef),
    Extern(Fn),
    Interface(Interface),
    Import(Import),
    Global(Global),
}

/// `enum Name { V(t, ..), W {a: t}, U }` — tagged union. Variant payload
/// key `E.V` (module-qualified enums qualify only the enum part).
#[derive(Clone, Debug)]
pub struct EnumDef {
    pub name: String,
    pub variants: Vec<Variant>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Variant {
    pub name: String,
    pub payload: VariantPayload,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum VariantPayload {
    Unit,
    Pos(Vec<TyRef>),
    Named(Vec<(String, TyRef)>),
}

/// `import "path" [as alias] [.{name, ...}]`
/// canonical module name = file stem of the resolved path; `alias` binds a
/// different local name; `selective` names also bind unqualified.
#[derive(Clone, Debug)]
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

/// inverse of `callee_name`: "a.b.c" -> Field{Field{a,b},c} — rebuilds
/// the receiver/method object behind a flattened dotted ident
pub fn unflatten(path: &str, span: Span) -> Option<Expr> {
    let mut it = path.split('.');
    let mut e = Expr {
        kind: ExprKind::Ident(it.next()?.to_string()),
        span,
    };
    for n in it {
        e = Expr {
            kind: ExprKind::Field { obj: Box::new(e), name: n.to_string() },
            span,
        };
    }
    Some(e)
}



/// top-level `let name[: ty] = init` — process-wide state, initialized
/// in oxl_main prologue (order: before user main runs)
#[derive(Clone, Debug)]
pub struct Global {
    pub name: String,
    pub ty: Option<TyRef>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Interface {
    pub name: String,
    /// constraint surface: ops the type must support. v0: fixed vocab —
    /// "eq" (==), "ordered" (< >), "add" (+)
    pub ops: Vec<String>,
    /// method decls — a non-empty list makes the interface a *type*:
    /// `var w Writer` is a fat {data, vtab} pair; satisfaction is
    /// structural (impl must carry each method)
    pub methods: Vec<MethodSig>,
    pub span: Span,
}

/// `name(p: t, ..) -> t` inside an interface body — no receiver, no body
#[derive(Clone, Debug)]
pub struct MethodSig {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: RetTy,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Fn {
    pub name: String,
    /// `fn (r T) m(...)` — receiver; after module qualification `name`
    /// is the dotted `T.m` form and `params[0]`-ish typing treats the
    /// receiver as an implicit first argument
    pub recv: Option<Param>,
    pub ty_params: Vec<(String, String)>, // [(T, constraint)] — "" = any
    pub params: Vec<Param>,
    pub ret: RetTy,
    pub body: Option<Block>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Param {
    pub name: String,
    pub ty: TyRef,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub struct Struct {
    pub name: String,
    pub fields: Vec<Param>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum RetTy {
    Void,
    One(TyRef),
    Pair(TyRef), // -> [T, err]
}

#[derive(Clone, Debug)]
pub struct TyRef {
    pub kind: TyKind,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum TyKind {
    Named(String),
    Own(Box<TyRef>),
    Ref(Box<TyRef>),
    Mut(Box<TyRef>),
    Chan,
    Slice(Box<TyRef>),
    /// `fn(int, int) -> bool` / `fn(a: int) -> int` — fn value type;
    /// ret None = void. `[T, err]` returns are not supported in v0.2.
    Fn { params: Vec<TyRef>, ret: Option<Box<TyRef>> },
}

/// canonical `own T`/`[]T`/`fn(..) -> T` text (fmt printer + hover sigs)
pub fn ty_text(t: &TyRef) -> String {
    match &t.kind {
        TyKind::Named(n) => n.clone(),
        TyKind::Own(i) => format!("own {}", ty_text(i)),
        TyKind::Ref(i) => format!("ref {}", ty_text(i)),
        TyKind::Mut(i) => format!("mut {}", ty_text(i)),
        TyKind::Chan => "chan".into(),
        TyKind::Slice(i) => format!("[]{}", ty_text(i)),
        TyKind::Fn { params, ret } => {
            let ps = params.iter().map(ty_text).collect::<Vec<_>>().join(", ");
            match ret {
                Some(r) => format!("fn({ps}) -> {}", ty_text(r)),
                None => format!("fn({ps})"),
            }
        }
    }
}

/// `fn name(params) -> ret` signature text (hover display)
pub fn fn_sig_text(f: &Fn) -> String {
    let mut s = String::from("fn ");
    if let Some(r) = &f.recv {
        s.push_str(&format!("({} {}) ", r.name, ty_text(&r.ty)));
    }
    s.push_str(&f.name);
    if !f.ty_params.is_empty() {
        let ps: Vec<String> = f
            .ty_params
            .iter()
            .map(|(t, c)| if c.is_empty() { t.clone() } else { format!("{t} {c}") })
            .collect();
        s.push_str(&format!("[{}]", ps.join(", ")));
    }
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, ty_text(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    s.push_str(&format!("({params})"));
    match &f.ret {
        RetTy::Void => {}
        RetTy::One(t) => s.push_str(&format!(" -> {}", ty_text(t))),
        RetTy::Pair(t) => s.push_str(&format!(" -> [{}, err]", ty_text(t))),
    }
    s
}

#[derive(Clone, Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Bind {
    Name(String),
    Blank,
}

#[derive(Clone, Debug)]
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
    Select { arms: Vec<SelectArm>, span: Span },
    Defer { call: Expr, span: Span },
    Unsafe { body: Block, span: Span },
    /// codegen-internal: emitted by the AST hoist before the hoisted
    /// operand statements of a `return` whose operands suspend — the
    /// frame emitter drains scopes+defers here so operand evaluation
    /// (including its suspends) starts only after the drain, per the
    /// B5 return-operand timing rule
    ExitBoundary { span: Span },
    Break { span: Span },
    Continue { span: Span },
}

#[derive(Clone, Debug)]
pub struct SelectArm {
    pub kind: SelKind,
    pub body: Block,
}

#[derive(Clone, Debug)]
pub enum SelKind {
    /// `v := recv(ch)` / `recv(ch)` — parked receiver
    Recv { chan: Expr, bind: Option<String>, span: Span },
    /// `timeout(ms)` — fires once after ms via a runtime timer
    Timeout { ms: Expr, span: Span },
    /// `else` — non-blocking default
    Else { span: Span },
}

#[derive(Clone, Debug)]
pub struct MatchArm {
    pub pat: Pattern,
    pub body: MatchBody,
    pub span: Span,
}

#[derive(Clone, Debug)]
pub enum Pattern {
    /// `_` — covers the rest
    Wild,
    /// `E.V` unit variant — no bindings
    VarUnit { path: String },
    /// `E.V(x, ..)` — positional payload binds
    VarPos { path: String, binds: Vec<String> },
    /// `E.V{a, ..}` — named payload binds
    VarNamed { path: String, binds: Vec<String> },
    /// literal constant — int/float/str/bool (non-enum scrutinee)
    Lit(Expr),
    /// bare ident — binds the scrutinee itself
    Bind(String),
}

#[derive(Clone, Debug)]
pub enum MatchBody {
    /// `=> expr` — value arm
    Expr(Expr),
    /// `=> { stmts }` — statement arm
    Block(Block),
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
    /// `e as T` — v0.2 only `E.V` (enum variant payload extraction)
    As { e: Box<Expr>, ty: TyRef },
    /// `match e { arms }` — value match when arms are `=> expr`
    Match { e: Box<Expr>, arms: Vec<MatchArm> },
    /// `s[i]` — bounds-checked slice index (lvalue)
    Index { obj: Box<Expr>, idx: Box<Expr> },
    /// `s[a:b]` — slice view, shared backing (lo/hi optional)
    Slice { obj: Box<Expr>, lo: Option<Box<Expr>>, hi: Option<Box<Expr>> },
    /// `{ |a: int, b: int| stmts }` — closure; captures enclosing
    /// locals by value into a generated env struct
    Closure { params: Vec<Param>, body: Block },
    /// a type in expression position — `fn(int) -> int`; only valid
    /// as `mkslice`'s element-type arg
    TyLit(TyRef),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    BitAnd, BitOr, Xor,
    Eq, Ne, Lt, Le, Gt, Ge,
    And, Or,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UnOp {
    Neg,
    Not,
}
