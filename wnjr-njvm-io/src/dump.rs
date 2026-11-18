//! Stable, diffable text form of an NJVM-IR module.
//!
//! [`dump_module`] renders a [`Module`] as plain text. The format is meant for
//! diffing two frontends' output (or two versions of one), so it is fixed:
//!
//! - One item per line, two-space indentation, `\n` line endings, no trailing
//!   spaces. Module sections are printed in [`Module`] field order and only when
//!   non-empty.
//! - Functions are numbered in the unified function index space (imports
//!   first), as the verifier and encoder do.
//! - Labels are printed by id: `L3:` declares label 3, `jmp L3` jumps to it.
//! - An expression is a tree, one node per line; a node's operands follow it,
//!   indented one level, in field order.
//! - Structured ops print a header line, their operand expressions indented,
//!   then each child body introduced by a keyword at the op's own level
//!   (`then`/`else`, `case`/`default`, `catch`, `fallback`) and closed by
//!   `end`. `block` and `loop` bodies follow the header directly.
//! - Constants print the declared type and the stored value. Floats print the
//!   raw bits in hex (what is on the wire) followed by the value in
//!   parentheses (Rust's `{:?}` of the float).
//!
//! The text is a view, not a second interchange format: there is no parser.

use std::fmt::Write;

use wnjr_core::njvm::{
    AtomicOp, BinaryOp, ConstVal, Expr, Function, ImportKind, InitExpr, MemType, Module, Op,
    SegmentMode, Type, UnaryOp,
};

/// Render `module` as text. `version` is the envelope version printed in the
/// header line.
pub fn dump_module(module: &Module, version: u32) -> String {
    let mut p = Printer::default();
    p.line(format_args!("njvm-ir version {version}"));
    p.module(module);
    p.out
}

#[derive(Default)]
struct Printer {
    out: String,
    indent: usize,
}

impl Printer {
    fn line(&mut self, args: std::fmt::Arguments<'_>) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.write_fmt(args).unwrap();
        self.out.push('\n');
    }

    fn nested(&mut self, f: impl FnOnce(&mut Self)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    fn module(&mut self, m: &Module) {
        if !m.types.is_empty() {
            self.line(format_args!("types:"));
            self.nested(|p| {
                for (i, t) in m.types.iter().enumerate() {
                    p.line(format_args!("type {i}: {} -> {}", tys(&t.params), tys(&t.results)));
                }
            });
        }
        if !m.imports.is_empty() {
            self.line(format_args!("imports:"));
            self.nested(|p| {
                for (i, im) in m.imports.iter().enumerate() {
                    let kind = match &im.kind {
                        ImportKind::Function(t) => format!("func type {t}"),
                        ImportKind::Table(t) => format!("table {}", limits(t.min, t.max)),
                        ImportKind::Memory(mt) => format!(
                            "memory {}{}",
                            limits(mt.min, mt.max),
                            if mt.shared { " shared" } else { "" }
                        ),
                        ImportKind::Global(g) => {
                            format!("global {}{}", if g.mutable { "mut " } else { "" }, ty(g.ty))
                        }
                        ImportKind::Tag(t) => format!("tag {}", tys(&t.params)),
                    };
                    p.line(format_args!("import {i} {:?}.{:?}: {kind}", im.module, im.name));
                }
            });
        }
        if !m.tables.is_empty() {
            self.line(format_args!("tables:"));
            self.nested(|p| {
                for (i, t) in m.tables.iter().enumerate() {
                    p.line(format_args!("table {i}: {}", limits(t.min, t.max)));
                }
            });
        }
        if !m.memories.is_empty() {
            self.line(format_args!("memories:"));
            self.nested(|p| {
                for (i, mt) in m.memories.iter().enumerate() {
                    let shared = if mt.shared { " shared" } else { "" };
                    p.line(format_args!("memory {i}: {}{shared}", limits(mt.min, mt.max)));
                }
            });
        }
        if !m.globals.is_empty() {
            self.line(format_args!("globals:"));
            self.nested(|p| {
                for (i, g) in m.globals.iter().enumerate() {
                    let mutable = if g.ty.mutable { "mut " } else { "" };
                    p.line(format_args!("global {i}: {mutable}{} = {}", ty(g.ty.ty), init(&g.init)));
                }
            });
        }
        if !m.tags.is_empty() {
            self.line(format_args!("tags:"));
            self.nested(|p| {
                for (i, t) in m.tags.iter().enumerate() {
                    p.line(format_args!("tag {i}: {}", tys(&t.params)));
                }
            });
        }
        if !m.exports.is_empty() {
            self.line(format_args!("exports:"));
            self.nested(|p| {
                for e in &m.exports {
                    let kind = format!("{:?}", e.kind).to_lowercase();
                    p.line(format_args!("export {:?}: {kind} {}", e.name, e.index));
                }
            });
        }
        if let Some(s) = m.start {
            self.line(format_args!("start: func {s}"));
        }
        if !m.elements.is_empty() {
            self.line(format_args!("elements:"));
            self.nested(|p| {
                for (i, e) in m.elements.iter().enumerate() {
                    let funcs: Vec<String> = e.funcs.iter().map(u32::to_string).collect();
                    p.line(format_args!("elem {i}: {} funcs [{}]", mode(&e.mode, "table"), funcs.join(", ")));
                }
            });
        }
        if !m.data.is_empty() {
            self.line(format_args!("data:"));
            self.nested(|p| {
                for (i, d) in m.data.iter().enumerate() {
                    let hex: Vec<String> = d.bytes.iter().map(|b| format!("{b:02x}")).collect();
                    p.line(format_args!("data {i}: {} bytes {} [{}]", mode(&d.mode, "memory"), d.bytes.len(), hex.join(" ")));
                }
            });
        }
        if let Some(g) = m.stack_pointer {
            self.line(format_args!("stack_pointer: global {g}"));
        }
        if let Some(g) = m.tls_base {
            self.line(format_args!("tls_base: global {g}"));
        }
        let first = m.imported_functions();
        for (i, f) in m.functions.iter().enumerate() {
            self.function(first + i as u32, f);
        }
    }

    fn function(&mut self, idx: u32, f: &Function) {
        self.line(format_args!("func {idx}: {} -> {}", tys(&f.params), tys(&f.results)));
        self.nested(|p| {
            p.line(format_args!("locals: {}", tys(&f.locals)));
            p.body(&f.body);
        });
    }

    fn body(&mut self, ops: &[Op]) {
        for op in ops {
            self.op(op);
        }
    }

    /// A child body introduced by `keyword` at the current level.
    fn section(&mut self, keyword: std::fmt::Arguments<'_>, ops: &[Op]) {
        self.line(keyword);
        self.nested(|p| p.body(ops));
    }

    fn exprs(&mut self, es: &[&Expr]) {
        self.nested(|p| {
            for e in es {
                p.expr(e);
            }
        });
    }

    fn op(&mut self, op: &Op) {
        match op {
            Op::Label { id, params } if params.is_empty() => self.line(format_args!("L{id}:")),
            Op::Label { id, params } => self.line(format_args!("L{id} {}:", tys(params))),
            Op::Jmp { target } => self.line(format_args!("jmp L{target}")),
            Op::JmpIf { cond, target } => {
                self.line(format_args!("jmp_if L{target}"));
                self.exprs(&[cond]);
            }
            Op::BrTable { index, targets, default } => {
                let ts: Vec<String> = targets.iter().map(|t| format!("L{t}")).collect();
                self.line(format_args!("br_table [{}] default L{default}", ts.join(", ")));
                self.exprs(&[index]);
            }
            Op::GotoIndirect { target } => {
                self.line(format_args!("goto_indirect"));
                self.exprs(&[target]);
            }
            Op::Store { ty, ptr, val, offset, align } => {
                self.line(format_args!("store {} offset={offset} align={align}", mem(*ty)));
                self.exprs(&[ptr, val]);
            }
            Op::Block { body } => {
                self.line(format_args!("block"));
                self.nested(|p| p.body(body));
                self.line(format_args!("end"));
            }
            Op::Loop { body } => {
                self.line(format_args!("loop"));
                self.nested(|p| p.body(body));
                self.line(format_args!("end"));
            }
            Op::If { cond, then_body, else_body } => {
                self.line(format_args!("if"));
                self.exprs(&[cond]);
                self.section(format_args!("then"), then_body);
                self.section(format_args!("else"), else_body);
                self.line(format_args!("end"));
            }
            Op::Return { vals } => {
                self.line(format_args!("return"));
                self.exprs(&vals.iter().collect::<Vec<_>>());
            }
            Op::Unreachable => self.line(format_args!("unreachable")),
            Op::Switch { scrutinee, cases, default } => {
                self.line(format_args!("switch"));
                self.exprs(&[scrutinee]);
                for (v, ops) in cases {
                    self.section(format_args!("case {}", constval(v)), ops);
                }
                self.section(format_args!("default"), default);
                self.line(format_args!("end"));
            }
            Op::TypeSwitch { scrutinee, cases } => {
                self.line(format_args!("type_switch"));
                self.exprs(&[scrutinee]);
                for (t, ops) in cases {
                    self.section(format_args!("case {}", ty(*t)), ops);
                }
                self.line(format_args!("end"));
            }
            Op::Call { func_idx, args } => {
                self.line(format_args!("call func {func_idx}"));
                self.exprs(&args.iter().collect::<Vec<_>>());
            }
            Op::CallIndirect { type_idx, table_idx, callee, args } => {
                self.line(format_args!("call_indirect type {type_idx} table {table_idx}"));
                let mut es = vec![callee];
                es.extend(args.iter());
                self.exprs(&es);
            }
            Op::TryTable { body, catches } => {
                self.line(format_args!("try_table"));
                self.nested(|p| p.body(body));
                for c in catches {
                    let bind: Vec<String> = c.bind.iter().map(|l| format!("{l}")).collect();
                    let bind = if bind.is_empty() { String::new() } else { format!(" bind {}", bind.join(" ")) };
                    match c.tag {
                        Some(tag) => self.section(format_args!("catch tag {tag}{bind}"), &c.body),
                        None => self.section(format_args!("catch_all"), &c.body),
                    }
                }
                self.line(format_args!("end"));
            }
            Op::Throw { tag_idx, args } => {
                self.line(format_args!("throw tag {tag_idx}"));
                self.exprs(&args.iter().collect::<Vec<_>>());
            }
            Op::Setjmp { buf, dest } => {
                self.line(format_args!("setjmp dest={dest}"));
                self.exprs(&[buf]);
            }
            Op::Longjmp { buf, val } => {
                self.line(format_args!("longjmp"));
                self.exprs(&[buf, val]);
            }
            Op::Await { channel, fallback } => {
                self.line(format_args!("await"));
                self.exprs(&[channel]);
                self.section(format_args!("fallback"), fallback);
                self.line(format_args!("end"));
            }
            Op::Yield { val } => {
                self.line(format_args!("yield"));
                self.exprs(&[val]);
            }
            Op::LocalSet { idx, val } => {
                self.line(format_args!("local.set {idx}"));
                self.exprs(&[val]);
            }
            Op::GlobalSet { idx, val } => {
                self.line(format_args!("global.set {idx}"));
                self.exprs(&[val]);
            }
            Op::MemoryCopy { dest, src, len } => {
                self.line(format_args!("memory.copy"));
                self.exprs(&[dest, src, len]);
            }
            Op::MemoryFill { dest, val, len } => {
                self.line(format_args!("memory.fill"));
                self.exprs(&[dest, val, len]);
            }
            Op::MemorySize => self.line(format_args!("memory.size")),
            Op::MemoryGrow { delta } => {
                self.line(format_args!("memory.grow"));
                self.exprs(&[delta]);
            }
            Op::Atomic { op, width, offset } => {
                self.line(format_args!("atomic {} width={width} offset={offset}", atomic(*op)));
            }
            Op::Const { ty: t, val } => self.line(format_args!("const {} {}", ty(*t), constval(val))),
            Op::Drop { val } => {
                self.line(format_args!("drop"));
                self.exprs(&[val]);
            }
            Op::Nop => self.line(format_args!("nop")),
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Const { ty: t, val } => self.line(format_args!("const {} {}", ty(*t), constval(val))),
            Expr::Binary { op, lhs, rhs, ty: t } => {
                self.line(format_args!("{} {}", binop(*op), ty(*t)));
                self.exprs(&[lhs, rhs]);
            }
            Expr::Unary { op, val, ty: t } => {
                self.line(format_args!("{} {}", unop(*op), ty(*t)));
                self.exprs(&[val]);
            }
            Expr::Convert { from, to, val } => {
                self.line(format_args!("convert {} -> {}", ty(*from), ty(*to)));
                self.exprs(&[val]);
            }
            Expr::Select { cond, then_val, else_val } => {
                self.line(format_args!("select"));
                self.exprs(&[cond, then_val, else_val]);
            }
            Expr::Load { ty: t, ptr, offset, align } => {
                self.line(format_args!("load {} offset={offset} align={align}", mem(*t)));
                self.exprs(&[ptr]);
            }
            Expr::LocalGet(i) => self.line(format_args!("local.get {i}")),
            Expr::GlobalGet(i) => self.line(format_args!("global.get {i}")),
            Expr::LocalTee { idx, val } => {
                self.line(format_args!("local.tee {idx}"));
                self.exprs(&[val]);
            }
            Expr::Call { func_idx, args } => {
                self.line(format_args!("call func {func_idx}"));
                self.exprs(&args.iter().collect::<Vec<_>>());
            }
            Expr::CallIndirect { type_idx, table_idx, callee, args } => {
                self.line(format_args!("call_indirect type {type_idx} table {table_idx}"));
                let mut es: Vec<&Expr> = vec![callee];
                es.extend(args.iter());
                self.exprs(&es);
            }
            Expr::LabelAddr(l) => self.line(format_args!("label.addr L{l}")),
        }
    }
}

fn ty(t: Type) -> &'static str {
    match t {
        Type::I8 => "i8",
        Type::U8 => "u8",
        Type::I16 => "i16",
        Type::U16 => "u16",
        Type::I32 => "i32",
        Type::U32 => "u32",
        Type::I64 => "i64",
        Type::U64 => "u64",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Ptr => "ptr",
    }
}

fn tys(ts: &[Type]) -> String {
    let v: Vec<&str> = ts.iter().map(|t| ty(*t)).collect();
    format!("({})", v.join(", "))
}

fn mem(t: MemType) -> &'static str {
    match t {
        MemType::I8 => "i8",
        MemType::U8 => "u8",
        MemType::I16 => "i16",
        MemType::U16 => "u16",
        MemType::I32 => "i32",
        MemType::I64 => "i64",
        MemType::F32 => "f32",
        MemType::F64 => "f64",
    }
}

fn binop(o: BinaryOp) -> &'static str {
    match o {
        BinaryOp::Add => "add",
        BinaryOp::Sub => "sub",
        BinaryOp::Mul => "mul",
        BinaryOp::Div => "div",
        BinaryOp::Rem => "rem",
        BinaryOp::And => "and",
        BinaryOp::Or => "or",
        BinaryOp::Xor => "xor",
        BinaryOp::Shl => "shl",
        BinaryOp::Shr => "shr",
        BinaryOp::Rotl => "rotl",
        BinaryOp::Rotr => "rotr",
        BinaryOp::Lt => "lt",
        BinaryOp::Le => "le",
        BinaryOp::Gt => "gt",
        BinaryOp::Ge => "ge",
        BinaryOp::Eq => "eq",
        BinaryOp::Ne => "ne",
        BinaryOp::Min => "min",
        BinaryOp::Max => "max",
        BinaryOp::Copysign => "copysign",
    }
}

fn unop(o: UnaryOp) -> &'static str {
    match o {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
        UnaryOp::Abs => "abs",
        UnaryOp::Sqrt => "sqrt",
        UnaryOp::Ceil => "ceil",
        UnaryOp::Floor => "floor",
        UnaryOp::Trunc => "trunc",
        UnaryOp::Nearest => "nearest",
        UnaryOp::Clz => "clz",
        UnaryOp::Ctz => "ctz",
        UnaryOp::Popcnt => "popcnt",
    }
}

fn atomic(o: AtomicOp) -> &'static str {
    match o {
        AtomicOp::Load => "load",
        AtomicOp::Store => "store",
        AtomicOp::Add => "add",
        AtomicOp::Sub => "sub",
        AtomicOp::And => "and",
        AtomicOp::Or => "or",
        AtomicOp::Xor => "xor",
        AtomicOp::Xchg => "xchg",
        AtomicOp::Cmpxchg => "cmpxchg",
        AtomicOp::Wait => "wait",
        AtomicOp::Notify => "notify",
        AtomicOp::Fence => "fence",
    }
}

/// The stored value, tagged with its storage kind so a mismatch against the
/// declared type stays visible.
fn constval(v: &ConstVal) -> String {
    match v {
        ConstVal::I32(x) => format!("i32:{x}"),
        ConstVal::I64(x) => format!("i64:{x}"),
        ConstVal::F32(b) => format!("f32:0x{b:08x} ({:?})", f32::from_bits(*b)),
        ConstVal::F64(b) => format!("f64:0x{b:016x} ({:?})", f64::from_bits(*b)),
    }
}

fn init(i: &InitExpr) -> String {
    match i {
        InitExpr::Const(v) => format!("const {}", constval(v)),
        InitExpr::GlobalGet(g) => format!("global.get {g}"),
    }
}

fn mode(m: &SegmentMode, space: &str) -> String {
    match m {
        SegmentMode::Passive => "passive".to_string(),
        SegmentMode::Active { index, offset } => format!("active {space} {index} offset {}", init(offset)),
    }
}

fn limits(min: u32, max: Option<u32>) -> String {
    match max {
        Some(max) => format!("min {min} max {max}"),
        None => format!("min {min}"),
    }
}
