//! AST for the LPC subset (mirrors LDMud's parse.y structures at a coarse grain).

use std::rc::Rc;

pub type ExprId = usize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add, Sub, Mul, Div, Mod,
    Assign, AddAssign, SubAssign, MulAssign, DivAssign, ModAssign,
    AssignBitAnd, AssignBitOr, AssignBitXor, AssignLsh, AssignRsh,
    Eq, Ne, Lt, Gt, Le, Ge,
    And, Or,
    BitAnd, BitOr, BitXor, Lsh, Rsh,
    Range, // .. inside index
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UnOp { Neg, LNot, BNot, PreIncr, PreDecr, PostIncr, PostDecr, Plus }

#[derive(Debug, Clone)]
pub enum Expr {
    IntConst(i64),
    FloatConst(f64),
    StrConst(Rc<str>),
    ArrayLit(Vec<ExprId>),
    MappingLit(Vec<(ExprId, ExprId)>),
    Ident(String),                       // variable or efun context
    QuotedIdent(String),                 // #'name
    ClosureFunc { name: String },        // #'name
    LambdaStr(ExprId),                   // handled by bind_lambda efun at runtime
    Pre(UnOp, ExprId),
    Post(UnOp, ExprId),
    Bin(BinOp, ExprId, ExprId),
    Ternary(ExprId, ExprId, ExprId),
    Index(ExprId, ExprId),               // a[b]
    Slice(ExprId, ExprId, Option<ExprId>), // a[b..c] or a[b...]
    Call { name: Option<ExprId>, args: Vec<ExprId>, inherit_path: Option<String> },
    DirectCall { name: String, args: Vec<ExprId> },
    // Call { name: None } => direct identifier call handled by vm; Some(callee) indirect/call_out-like or ob-> via CallOther
    CallOther { ob: ExprId, name: Option<ExprId>, lfun_name: Option<String>, args: Vec<ExprId> }, // ob->f(...) or ob->(clo)(...)
    Cast(String, ExprId),
    Catch(ExprId),
    Throw(ExprId),
    Comma(Vec<ExprId>),
    ClosureInline { body_id: usize, nargs: usize },
}

#[derive(Debug, Clone)]
pub struct ArgInfo {
    pub name: String,
    pub ty: Option<String>,
    pub default: Option<ExprId>, // varargs default
}


#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Visibility { Public, Private, Protected, Static }

#[derive(Debug, Clone)]
pub struct FuncDefInfo {
    pub name: String,
    pub ret_ty: Option<String>,
    pub args: Vec<ArgInfo>,
    pub varargs: bool,
    pub vis: Visibility,
    pub nomask: bool,
    pub virtual_flag: bool,
    pub body_id: usize, // program.func_bodies index
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct VarInit {
    pub name: String,
    pub ty: Option<String>,
    pub init: Option<ExprId>,
    pub vis: Visibility,
    pub nosave: bool,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Expr(ExprId),
    Decl(VarInit),
    If { cond: ExprId, then: Vec<Stmt>, els: Vec<Stmt> },
    While { cond: ExprId, body: Vec<Stmt> },
    DoWhile { cond: ExprId, body: Vec<Stmt> },
    For { init: Vec<Stmt>, cond: Option<ExprId>, step: Vec<Stmt>, body: Vec<Stmt> },
    Foreach { vars: Vec<String>, iter: ExprId, range_hi: Option<ExprId>, body: Vec<Stmt> },
    Switch { expr: ExprId, cases: Vec<(Vec<ExprId>, Vec<Stmt>)>, default: Vec<Stmt> },
    Return(Option<ExprId>),
    Break,
    Continue,
    Block(Vec<Stmt>),
    Empty,
}

#[derive(Debug, Clone)]
pub struct TopLevel {
    pub inherits: Vec<String>,
    pub decls: Vec<Decl>,
}

#[derive(Debug, Clone)]
pub enum Decl {
    Func(FuncDefInfo),
    Var(VarInit),
}

/// A compiled source file: statements + constant pool of expressions.
#[derive(Debug)]
pub struct Compiled {
    pub filename: String,
    pub inherits: Vec<String>,
    pub vars: Vec<VarInit>,
    pub funcs: Vec<FuncDefInfo>,
    pub bodies: Vec<Vec<Stmt>>,
    pub exprs: Vec<Expr>,
    pub func_index: std::collections::HashMap<String, usize>,
    /// inline-closure parameter name aliases: body_id -> [(name, $-index)]
    pub inline_params: std::collections::HashMap<usize, Vec<(String, usize)>>,
}

impl Compiled {
    pub fn new(filename: &str) -> Compiled {
        Compiled {
            filename: filename.to_string(),
            inherits: Vec::new(),
            vars: Vec::new(),
            funcs: Vec::new(),
            bodies: Vec::new(),
            exprs: Vec::new(),
            func_index: std::collections::HashMap::new(),
            inline_params: std::collections::HashMap::new(),
        }
    }

    pub fn add_expr(&mut self, e: Expr) -> ExprId {
        self.exprs.push(e);
        self.exprs.len() - 1
    }
}
