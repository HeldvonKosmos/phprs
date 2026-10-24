//! Compiler: mago CST → `bytecode::Module`.
//!
//! One `Compiler` per file. Each function, method, closure and the top-level
//! script becomes a `CodeUnit` built by a `UnitBuilder` (slots, labels, loop
//! and try bookkeeping). Names are resolved against the current namespace and
//! `use` imports; the VM links them at load time.

pub mod dialect;
mod expr;
mod names;
mod stmt;
mod class;
mod capture;

use std::collections::HashMap;

use mago_allocator::LocalArena;
use mago_database::file::File;
use mago_span::{HasSpan, Span};
use mago_syntax::cst::Program;
use mago_syntax::parser::parse_file;

use crate::bytecode::*;
use dialect::Rule;

fn is_cmp(op: BinOp) -> bool {
    matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Identical | BinOp::NotIdentical)
}
use names::Names;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompileError {
    pub code: Option<&'static str>,
    pub message: String,
    pub hint: Option<&'static str>,
    pub line: u32,
    pub file: String,
}

impl std::fmt::Display for CompileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.code {
            Some(code) => write!(f, "error[{code}]: {}", self.message)?,
            None => write!(f, "error: {}", self.message)?,
        }
        write!(f, "\n  --> {}:{}", self.file, self.line)?;
        if let Some(hint) = self.hint
            && !hint.is_empty()
        {
            write!(f, "\n  = Hinweis: {hint}")?;
        }
        Ok(())
    }
}

pub type CResult<T> = Result<T, CompileError>;

/// Compile one `.grd` file. Parses with mago, then compiles.
pub fn compile_source(file_name: &str, source: &[u8]) -> CResult<Module> {
    let arena = LocalArena::new();
    let file = File::ephemeral(
        std::borrow::Cow::Owned(file_name.as_bytes().to_vec()),
        std::borrow::Cow::Owned(normalize_open_tag(source)),
    );
    let program = parse_file(&arena, &file);
    if let Some(err) = program.errors.first() {
        return Err(CompileError {
            code: None,
            message: format!("Syntaxfehler: {err}"),
            hint: None,
            line: line_of(source, err.span().start.offset),
            file: file_name.to_string(),
        });
    }
    let mut c = Compiler::new(file_name, source);
    c.compile_program(program)?;
    Ok(c.module)
}

/// `<?grd` is the dialect's opening tag (DIALECT §2); `<?php` stays accepted.
/// Both are five bytes, so byte offsets and line numbers are unchanged.
pub fn normalize_open_tag(source: &[u8]) -> Vec<u8> {
    let bom = if source.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    let mut out = source.to_vec();
    if out[bom..].starts_with(b"<?grd") {
        out[bom..bom + 5].copy_from_slice(b"<?php");
    }
    out
}

pub fn line_of(source: &[u8], offset: u32) -> u32 {
    (source[..(offset as usize).min(source.len())].iter().filter(|b| **b == b'\n').count() + 1) as u32
}

/// Class being compiled (for `self`, `__CLASS__`, promotion).
#[derive(Debug, Clone)]
pub(crate) struct ClassCtx {
    pub name: Vec<u8>,
    #[allow(dead_code)]
    pub parent: Option<Vec<u8>>,
    pub kind: ClassKind,
}

pub(crate) struct LoopCtx {
    pub break_patches: Vec<usize>,
    pub continue_patches: Vec<usize>,
    /// Number of try contexts open when the loop started.
    pub tries_depth: usize,
    pub is_switch: bool,
}

pub(crate) struct TryCtx {
    /// Jumps to patch with the finally label (ReturnViaFinally, end-of-try jumps).
    pub finally_patches: Vec<usize>,
    pub has_finally: bool,
}

pub(crate) struct UnitBuilder {
    pub unit: CodeUnit,
    /// Op indices that are jump targets (labels taken with `here()`).
    pub label_targets: std::collections::HashSet<Label>,
    pub slots: HashMap<Vec<u8>, Slot>,
    pub loops: Vec<LoopCtx>,
    pub tries: Vec<TryCtx>,
    pub function_name: Vec<u8>,
    pub is_method: bool,
    pub is_closure: bool,
    pub last_line: u32,
}

impl UnitBuilder {
    pub fn new(name: String, line: u32) -> Self {
        UnitBuilder {
            unit: CodeUnit::new(name, line),
            label_targets: std::collections::HashSet::new(),
            slots: HashMap::new(),
            loops: Vec::new(),
            tries: Vec::new(),
            function_name: Vec::new(),
            is_method: false,
            is_closure: false,
            last_line: 0,
        }
    }
    pub fn emit(&mut self, op: Op) -> usize {
        if let Some(idx) = self.try_fuse(&op) {
            return idx;
        }
        self.unit.ops.push(op);
        self.unit.ops.len() - 1
    }

    /// Superinstructions: fuse operand loads into binary ops and comparisons
    /// into conditional jumps. Only ops that no label can target are
    /// replaced (the middle of an expression), so jump targets stay valid.
    fn try_fuse(&mut self, op: &Op) -> Option<usize> {
        let n = self.unit.ops.len();
        match op {
            Op::Bin(bop) if n >= 2 && !self.label_boundary(n - 1) => {
                let fused = match (&self.unit.ops[n - 2], &self.unit.ops[n - 1]) {
                    (Op::LoadSlot(a), Op::LoadSlot(b)) => Op::BinSS { a: *a, b: *b, op: *bop },
                    (Op::LoadSlot(a), Op::PushInt(i)) if i32::try_from(*i).is_ok() => Op::BinSI { a: *a, imm: *i as i32, op: *bop },
                    (Op::LoadSlot(a), Op::PushConst(c)) => Op::BinSC { a: *a, c: *c, op: *bop },
                    _ => return None,
                };
                self.unit.ops.truncate(n - 2);
                self.unit.ops.push(fused);
                Some(n - 2)
            }
            Op::JumpIfFalse(target) if n >= 1 => {
                let fused = match &self.unit.ops[n - 1] {
                    Op::BinSS { a, b, op } if is_cmp(*op) => Op::JumpUnlessSS { a: *a, b: *b, op: *op, target: *target },
                    Op::BinSI { a, imm, op } if is_cmp(*op) => Op::JumpUnlessSI { a: *a, imm: *imm, op: *op, target: *target },
                    _ => return None,
                };
                self.unit.ops.truncate(n - 1);
                self.unit.ops.push(fused);
                Some(n - 1)
            }
            _ => None,
        }
    }

    /// True if some jump already targets op index `idx` (then it must stay).
    fn label_boundary(&self, idx: usize) -> bool {
        self.label_targets.contains(&(idx as Label))
    }
    pub fn here(&mut self) -> Label {
        let l = self.unit.ops.len() as Label;
        self.label_targets.insert(l);
        l
    }
    pub fn patch(&mut self, at: usize, label: Label) {
        match &mut self.unit.ops[at] {
            Op::Jump(l)
            | Op::JumpIfFalse(l)
            | Op::JumpIfTrue(l)
            | Op::JumpIfTrueKeep(l)
            | Op::JumpIfNullKeep(l)
            | Op::JumpIfNotNullKeep(l)
            | Op::ReturnViaFinally(l) => *l = label,
            Op::JumpViaFinally { target, .. } => *target = label,
            Op::JumpUnlessSS { target, .. } | Op::JumpUnlessSI { target, .. } => *target = label,
            Op::IterNext { end, .. } => *end = label,
            Op::JumpIfArgPresent { target, .. } => *target = label,
            Op::FinallyEnd { outer } => *outer = Some(label),
            other => panic!("patch on non-jump op {other:?}"),
        }
    }
    pub fn slot(&mut self, name: &[u8]) -> Slot {
        if let Some(s) = self.slots.get(name) {
            return *s;
        }
        let s = self.unit.num_slots;
        self.unit.num_slots += 1;
        self.slots.insert(name.to_vec(), s);
        s
    }
    pub fn lookup_slot(&self, name: &[u8]) -> Option<Slot> {
        self.slots.get(name).copied()
    }
    /// Anonymous slot for temporaries (iterators, match subjects).
    pub fn temp_slot(&mut self) -> Slot {
        let s = self.unit.num_slots;
        self.unit.num_slots += 1;
        s
    }
    pub fn arg_spec(&mut self, spec: ArgSpec) -> u32 {
        self.unit.arg_specs.push(spec);
        (self.unit.arg_specs.len() - 1) as u32
    }
    pub fn mark_line(&mut self, line: u32) {
        if line != self.last_line {
            self.last_line = line;
            let at = self.unit.ops.len() as u32;
            // two marks at the same op: the later one wins
            if let Some(last) = self.unit.lines.last_mut()
                && last.0 == at
            {
                last.1 = line;
            } else {
                self.unit.lines.push((at, line));
            }
        }
    }
}

pub struct Compiler<'src> {
    pub module: Module,
    source: &'src [u8],
    strings: HashMap<Vec<u8>, StrId>,
    pub(crate) names: Names,
    pub(crate) class_stack: Vec<ClassCtx>,
    pub(crate) units: Vec<UnitBuilder>,
    anon_class_counter: u32,
}

impl<'src> Compiler<'src> {
    pub fn new(file_name: &str, source: &'src [u8]) -> Self {
        let mut module = Module::default();
        module.file = file_name.to_string();
        Compiler {
            module,
            source,
            strings: HashMap::new(),
            names: Names::new(),
            class_stack: Vec::new(),
            units: Vec::new(),
            anon_class_counter: 0,
        }
    }

    // ---- errors -------------------------------------------------------

    pub(crate) fn line(&self, span: Span) -> u32 {
        line_of(self.source, span.start.offset)
    }

    pub(crate) fn err<T>(&self, span: Span, message: impl Into<String>) -> CResult<T> {
        Err(CompileError { code: None, message: message.into(), hint: None, line: self.line(span), file: self.module.file.clone() })
    }

    pub(crate) fn reject<T>(&self, rule: Rule, span: Span) -> CResult<T> {
        Err(CompileError {
            code: Some(rule.code),
            message: rule.reason.to_string(),
            hint: Some(rule.hint),
            line: self.line(span),
            file: self.module.file.clone(),
        })
    }

    pub(crate) fn reject_named<T>(&self, rule: Rule, name: &[u8], span: Span) -> CResult<T> {
        Err(CompileError {
            code: Some(rule.code),
            message: format!("`{}`: {}", String::from_utf8_lossy(name), rule.reason),
            hint: Some(rule.hint),
            line: self.line(span),
            file: self.module.file.clone(),
        })
    }

    // ---- pools ----------------------------------------------------------

    pub(crate) fn intern(&mut self, s: &[u8]) -> StrId {
        if let Some(id) = self.strings.get(s) {
            return *id;
        }
        let id = self.module.strings.len() as StrId;
        self.module.strings.push(s.to_vec());
        self.strings.insert(s.to_vec(), id);
        id
    }

    #[allow(dead_code)]
    pub(crate) fn intern_str(&mut self, s: &str) -> StrId {
        self.intern(s.as_bytes())
    }

    pub(crate) fn constant(&mut self, c: Const) -> ConstId {
        // small pool: linear dedupe for scalars keeps the pool compact
        if !matches!(c, Const::Array(_))
            && let Some(i) = self.module.consts.iter().position(|x| *x == c)
        {
            return i as ConstId;
        }
        self.module.consts.push(c);
        (self.module.consts.len() - 1) as ConstId
    }

    // ---- units ----------------------------------------------------------

    pub(crate) fn ub(&mut self) -> &mut UnitBuilder {
        self.units.last_mut().expect("no unit under construction")
    }

    pub(crate) fn emit(&mut self, op: Op) -> usize {
        self.ub().emit(op)
    }

    pub(crate) fn push_unit(&mut self, name: String, line: u32) {
        self.units.push(UnitBuilder::new(name, line));
    }

    /// Finish the innermost unit and store it in the module.
    pub(crate) fn pop_unit(&mut self) -> UnitId {
        let ub = self.units.pop().expect("unit stack");
        let mut names: Vec<(Slot, Vec<u8>)> = ub.slots.iter().map(|(k, v)| (*v, k.clone())).collect();
        names.sort();
        let mut unit = ub.unit;
        if unit.slot_names.is_empty() {
            let mut slot_names = vec![0; unit.num_slots as usize];
            for (slot, n) in names {
                let id = self.intern(&n);
                slot_names[slot as usize] = id;
            }
            unit.slot_names = slot_names;
        }
        // every unit ends with an implicit `return null`
        unit.ops.push(Op::PushNull);
        unit.ops.push(Op::Return);
        self.module.units.push(unit);
        (self.module.units.len() - 1) as UnitId
    }

    pub(crate) fn current_class(&self) -> Option<&ClassCtx> {
        self.class_stack.last()
    }

    pub(crate) fn next_anon_class_name(&mut self) -> Vec<u8> {
        self.anon_class_counter += 1;
        format!("class@anonymous#{}", self.anon_class_counter).into_bytes()
    }

    // ---- entry ----------------------------------------------------------

    pub fn compile_program(&mut self, program: &Program<'_>) -> CResult<()> {
        self.push_unit("{main}".to_string(), 1);
        self.compile_statements(program.statements.iter())?;
        let main = self.pop_unit();
        self.module.main = main;
        Ok(())
    }
}
