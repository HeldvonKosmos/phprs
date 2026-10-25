//! Compiled code as plain data (`RUNTIME_ARCHITECTURE.md` §4): no pointers,
//! no lifetimes, serialisable. Names are `StrId`s into the module's string
//! table; the linker/VM resolves them to classes, functions and property slots.

pub type StrId = u32;
pub type ConstId = u32;
pub type UnitId = u32;
pub type Slot = u32;
pub type Label = u32;

/// VM-wide selector for a method or property name (assigned by the link step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SelId(pub u32);

/// Index into the VM's function table (assigned by the link step).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FunctionId(pub u32);

/// A constant in the pool. Arrays only contain constants.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Const {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Array(Vec<(ConstKey, Const)>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ConstKey {
    Int(i64),
    Str(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    Concat,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    Identical,
    NotIdentical,
    Lt,
    Le,
    Gt,
    Ge,
    Spaceship,
    Coalesce, // only as compound `??=`
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum UnOp {
    Neg,
    Plus,
    Not,
    BitNot,
    CastInt,
    CastFloat,
    CastString,
    CastBool,
    CastArray,
    CastObject,
}

/// How a class is named at a use site.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClassRef {
    Named(StrId),
    /// Resolved by the link step.
    Id(crate::value::ClassId),
    SelfRef,
    Parent,
    Static,
    /// Class name (string) or object taken from the stack.
    Dynamic,
}

/// One step of an lvalue path (`$a[k]`, `$a[]`, `$o->p`, `$o->{$e}`).
/// Keys and dynamic names come from the stack, in order.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PathSeg {
    Dim,
    Append,
    Prop(StrId),
    /// Linked property name.
    PropSel(SelId),
    PropDyn,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PathBase {
    Slot(Slot),
    This,
    StaticProp(ClassRef, StrId),
    /// Object handle (or array value) taken from the stack, below the keys.
    Stack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum WriteMode {
    Assign,
    Compound(BinOp),
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PathOp {
    pub base: PathBase,
    pub segs: Vec<PathSeg>,
    pub mode: WriteMode,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClosureOp {
    pub unit: UnitId,
    pub captures: Vec<Slot>,
    pub bind_this: bool,
}

/// Arguments of a call: one entry per pushed argument.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ArgSpec {
    pub names: Vec<Option<StrId>>,
    pub spread: Vec<bool>,
}

impl ArgSpec {
    pub fn count(&self) -> usize {
        self.names.len()
    }
    pub fn is_simple(&self) -> bool {
        self.names.iter().all(Option::is_none) && self.spread.iter().all(|s| !s)
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Op {
    Nop,
    Pop,
    Dup,
    PushConst(ConstId),
    PushNull,
    PushTrue,
    PushFalse,
    PushInt(i64),

    LoadSlot(Slot),
    /// `$x ?? …`, `isset($x)`: undefined reads as null.
    LoadSlotQuiet(Slot),
    /// Pops. A local keeps the type of its first value (GRD021, D10).
    StoreSlot(Slot),
    /// Pops, without the type rule: `foreach` variables (bound per
    /// iteration) and compiler temporaries.
    BindSlot(Slot),
    UnsetSlot(Slot),
    LoadThis,

    Bin(BinOp),
    /// Fused `LoadSlot(a); LoadSlot(b); Bin(op)`.
    BinSS { a: Slot, b: Slot, op: BinOp },
    /// Fused `LoadSlot(a); PushInt(imm); Bin(op)`.
    BinSI { a: Slot, imm: i32, op: BinOp },
    /// Fused `LoadSlot(a); PushConst(c); Bin(op)`.
    BinSC { a: Slot, c: ConstId, op: BinOp },
    /// Fused comparison + `JumpIfFalse`: jumps when `a op b` is false.
    JumpUnlessSS { a: Slot, b: Slot, op: BinOp, target: Label },
    JumpUnlessSI { a: Slot, imm: i32, op: BinOp, target: Label },
    Un(UnOp),
    /// Truthiness as bool.
    ToBool,
    IsNull,
    IsNotNull,

    Jump(Label),
    /// Pops.
    JumpIfFalse(Label),
    JumpIfTrue(Label),
    /// Peeks: jumps with the value kept, otherwise pops it.
    JumpIfTrueKeep(Label),
    /// Peeks: jumps if null; the value stays on the stack either way (`?->`).
    JumpIfNullKeep(Label),
    /// Peeks: jumps with the value kept, otherwise pops it (`??`).
    JumpIfNotNullKeep(Label),

    NewArray(u32),
    /// arr, val → arr
    ArrayAppend,
    /// arr, key, val → arr
    ArrayInsert,
    /// arr, iterable → arr
    ArraySpread,
    /// arr, key → val (UndefinedKeyError if missing)
    FetchDim,
    FetchDimQuiet,
    /// Superinstructions: `FetchDim` with the container and key loaded from
    /// slots (`$a[$i]`), or only the key (`$expr[$i]`, container on the stack).
    FetchDimSS { a: Slot, k: Slot },
    FetchDimS(Slot),
    /// obj → val
    FetchProp(StrId),
    FetchPropQuiet(StrId),
    /// Linked forms of FetchProp/FetchPropQuiet.
    FetchPropSel(SelId),
    FetchPropSelQuiet(SelId),
    /// obj, name → val
    FetchPropDyn,
    FetchPropDynQuiet,
    /// `isset($o->name)`, the last property of an isset: pushes a bool. Like PHP's
    /// has_property: the value if visible, else `__isset` (never `__get`).
    IssetProp(StrId),
    /// `isset($o->$name)`: name on the stack above the object.
    IssetPropDyn,
    FetchStaticProp(ClassRef, StrId),
    FetchClassConst(ClassRef, StrId),
    /// Global constant with optional global fallback for namespaced code.
    FetchConst(StrId, Option<StrId>),
    FetchClassName(ClassRef),

    /// Stack: [base if Stack] keys… value → result
    StorePath(Box<PathOp>),
    /// Stack: [base if Stack] keys…
    UnsetPath(Box<PathOp>),
    /// Stack: slot-less increments on a plain variable.
    IncSlot { slot: Slot, mode: WriteMode },
    /// Same without pushing a result (statement context).
    IncSlotDiscard { slot: Slot, mode: WriteMode },
    /// `$slot op= value` with the slot value taken (unshared) for in-place
    /// string appends; pushes the result unless `discard`.
    CompoundSlot { slot: Slot, op: BinOp, discard: bool },

    CallFunc { name: StrId, fallback: Option<StrId>, spec: u32 },
    /// Linked function call.
    CallFuncId { id: FunctionId, spec: u32 },
    /// obj, args… → result
    CallMethod { name: StrId, spec: u32 },
    /// Linked method call.
    CallMethodSel { sel: SelId, spec: u32 },
    /// obj, name, args… → result
    CallMethodDyn { spec: u32 },
    CallStatic { class: ClassRef, name: StrId, spec: u32 },
    /// Linked static call.
    CallStaticSel { class: ClassRef, sel: SelId, spec: u32 },
    /// callable, args… → result
    CallValue { spec: u32 },
    New { class: ClassRef, spec: u32 },
    /// Pops the value.
    Return,
    /// Return inside `try { } finally { }`: parks the value and jumps.
    ReturnViaFinally(Label),
    /// `break`/`continue` out of a `try { } finally { }`: parks the jump
    /// target and runs the finally block first.
    JumpViaFinally { finally: Label, target: Label },

    MakeClosure(Box<ClosureOp>),
    CallableFromFunc { name: StrId, fallback: Option<StrId> },
    /// obj → callable
    CallableFromMethod(StrId),
    CallableFromStatic(ClassRef, StrId),

    Throw,
    /// End of a finally block: resumes the parked action (return/throw/none).
    FinallyEnd { outer: Option<Label> },

    /// Pops the iterable, stores iterator state in the slot.
    IterInit(Slot),
    /// Pushes [key,] value or jumps to `end` when exhausted.
    IterNext { iter: Slot, end: Label, with_key: bool },

    InstanceOf(ClassRef),
    Clone,
    Echo,
    /// Pops the subject; throws UnhandledMatchError.
    ThrowUnhandledMatch,
    /// Pops value and key (null for auto).
    Yield { with_key: bool },
    YieldFrom,
    /// `JumpIfArgPresent { index, target }` skips a default-value prologue.
    JumpIfArgPresent { index: u32, target: Label },
    /// Promotes constructor parameter from slot into `$this->prop`.
    PromoteParam { slot: Slot, prop: StrId },
    /// Line marker for error messages.
    Line(u32),
    /// REST1: `[e1, e2, …]` — pops `n` values into a list.
    ArrayList(u32),
    /// REST1: `['a' => e1, 'b' => e2, …]` with constant, distinct keys — copies
    /// the template (constant array with these keys, already hashed) and
    /// fills in the top `n` values in key order.
    ArrayTemplate { tpl: ConstId, n: u32 },
    /// OPT1: `top op $slot` (the stack top is the left operand).
    BinTS { s: Slot, op: BinOp },
    /// OPT1: `top op const`.
    BinTC { c: ConstId, op: BinOp },
    /// OPT1: `top op imm`.
    BinTI { imm: i32, op: BinOp },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TypeHint {
    Mixed,
    Null,
    Bool,
    True,
    False,
    Int,
    Float,
    String,
    Array,
    Object,
    Iterable,
    Callable,
    Void,
    Never,
    Static,
    SelfT,
    Parent,
    Class(StrId),
    Nullable(Box<TypeHint>),
    Union(Vec<TypeHint>),
    Intersection(Vec<TypeHint>),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Param {
    pub name: StrId,
    pub slot: Slot,
    pub ty: Option<TypeHint>,
    pub has_default: bool,
    /// The default as a link-time value (for reflection).
    pub default_init: Option<Init>,
    pub variadic: bool,
    pub promote: Option<Visibility>,
    pub promote_readonly: bool,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Catch {
    pub classes: Vec<StrId>,
    pub slot: Option<Slot>,
    pub target: Label,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TryRegion {
    pub start: Label,
    pub end: Label,
    pub catches: Vec<Catch>,
    pub finally: Option<Label>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CodeUnit {
    pub name: String,
    pub ops: Vec<Op>,
    pub num_slots: u32,
    pub slot_names: Vec<StrId>,
    pub params: Vec<Param>,
    pub required_params: u32,
    pub return_type: Option<TypeHint>,
    pub try_regions: Vec<TryRegion>,
    pub arg_specs: Vec<ArgSpec>,
    pub is_generator: bool,
    pub uses_this: bool,
    pub line: u32,
    /// Line table (PERF1, replaces `Op::Line`): `(op index, line)` where a new
    /// source line starts, in index order.
    #[serde(default)]
    pub lines: Vec<(u32, u32)>,
    /// GRD021: `true` for a local that may not take `null` once it holds a
    /// value (first assignment evidently non-null, or a non-nullable typed
    /// parameter). Missing entries mean "may".
    #[serde(default)]
    pub slot_not_null: Vec<bool>,
    /// GRD021: `true` for a local without a static type (`mixed`, the first
    /// non-null value had no evident type); the VM does not check it.
    #[serde(default)]
    pub slot_mixed: Vec<bool>,
}

impl CodeUnit {
    /// Source line of the op at `ip` (the unit's own line before the first entry).
    pub fn line_at(&self, ip: usize) -> u32 {
        let i = self.lines.partition_point(|(at, _)| (*at as usize) <= ip);
        if i == 0 { self.line } else { self.lines[i - 1].1 }
    }

    pub fn new(name: String, line: u32) -> Self {
        CodeUnit {
            name,
            ops: Vec::new(),
            num_slots: 0,
            slot_names: Vec::new(),
            params: Vec::new(),
            required_params: 0,
            return_type: None,
            try_regions: Vec::new(),
            arg_specs: Vec::new(),
            is_generator: false,
            uses_this: false,
            line,
            lines: Vec::new(),
            slot_not_null: Vec::new(),
            slot_mixed: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

/// A value computed at link time: either a constant or a tiny code unit.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Init {
    Const(ConstId),
    Unit(UnitId),
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AttributeDecl {
    pub name: StrId,
    pub args: Vec<(Option<StrId>, Init)>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassConstDecl {
    pub name: StrId,
    pub value: Init,
    pub visibility: Visibility,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PropDecl {
    pub name: StrId,
    pub default: Option<Init>,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_readonly: bool,
    pub ty: Option<TypeHint>,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MethodDecl {
    pub name: StrId,
    pub unit: Option<UnitId>, // None = abstract
    /// Parameters and return type of an abstract method (a unit without body).
    #[serde(default)]
    pub sig_unit: Option<UnitId>,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EnumCaseDecl {
    pub name: StrId,
    pub value: Option<Init>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClassDecl {
    pub name: StrId,
    pub kind: ClassKind,
    pub parent: Option<StrId>,
    pub interfaces: Vec<StrId>,
    pub traits: Vec<StrId>,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
    pub consts: Vec<ClassConstDecl>,
    pub props: Vec<PropDecl>,
    pub methods: Vec<MethodDecl>,
    pub cases: Vec<EnumCaseDecl>,
    pub enum_backing: Option<TypeHint>,
    pub attributes: Vec<AttributeDecl>,
    pub line: u32,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FunctionDecl {
    pub name: StrId,
    pub unit: UnitId,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Module {
    pub file: String,
    pub strings: Vec<Vec<u8>>,
    pub consts: Vec<Const>,
    pub units: Vec<CodeUnit>,
    pub classes: Vec<ClassDecl>,
    pub functions: Vec<FunctionDecl>,
    pub main: UnitId,
}

impl Module {
    pub fn str(&self, id: StrId) -> &[u8] {
        &self.strings[id as usize]
    }
    pub fn str_lossy(&self, id: StrId) -> String {
        String::from_utf8_lossy(&self.strings[id as usize]).into_owned()
    }
}

#[cfg(test)]
mod size_tests {
    #[test]
    fn op_is_small() {
        let size = std::mem::size_of::<super::Op>();
        assert!(size <= 24, "Op is {size} bytes");
        assert_eq!(std::mem::size_of::<crate::value::Val>(), 16);
    }
}

#[cfg(test)]
mod serde_tests {
    #[test]
    fn module_round_trips_through_postcard() {
        let src = b"<?grd\nnamespace A; use B\\C; #[Attr(1)] class K extends C { const X = [1, 'a' => 2.5]; private ?int $n = null; public function m(int $a, string ...$r): static { $f = fn($x) => $x + $a; try { return $this; } finally { $this->n = 1; } } }\nfunction g() { yield 1; }\n$k = new K(); echo $k->m(1)?->n ?? 'x';";
        let m = crate::compile::compile_source("t.grd", src).expect("compile");
        let bytes = postcard::to_allocvec(&m).expect("serialize");
        let back: super::Module = postcard::from_bytes(&bytes).expect("deserialize");
        assert_eq!(m, back);
        assert!(bytes.len() > 100);
    }
}
