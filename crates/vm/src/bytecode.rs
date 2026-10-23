//! Compiled code as plain data (`RUNTIME_ARCHITECTURE.md` §4): no pointers,
//! no lifetimes, serialisable. Names are `StrId`s into the module's string
//! table; the linker/VM resolves them to classes, functions and property slots.

pub type StrId = u32;
pub type ConstId = u32;
pub type UnitId = u32;
pub type Slot = u32;
pub type Label = u32;

/// A constant in the pool. Arrays only contain constants.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Vec<u8>),
    Array(Vec<(ConstKey, Const)>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConstKey {
    Int(i64),
    Str(Vec<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClassRef {
    Named(StrId),
    SelfRef,
    Parent,
    Static,
    /// Class name (string) or object taken from the stack.
    Dynamic,
}

/// One step of an lvalue path (`$a[k]`, `$a[]`, `$o->p`, `$o->{$e}`).
/// Keys and dynamic names come from the stack, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Dim,
    Append,
    Prop(StrId),
    PropDyn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathBase {
    Slot(Slot),
    This,
    StaticProp(ClassRef, StrId),
    /// Object handle (or array value) taken from the stack, below the keys.
    Stack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteMode {
    Assign,
    Compound(BinOp),
    PreInc,
    PreDec,
    PostInc,
    PostDec,
}

/// Arguments of a call: one entry per pushed argument.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, PartialEq)]
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
    /// Pops.
    StoreSlot(Slot),
    UnsetSlot(Slot),
    LoadThis,

    Bin(BinOp),
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
    JumpIfNullKeep(Label),
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
    /// obj → val
    FetchProp(StrId),
    FetchPropQuiet(StrId),
    /// obj, name → val
    FetchPropDyn,
    FetchPropDynQuiet,
    FetchStaticProp(ClassRef, StrId),
    FetchClassConst(ClassRef, StrId),
    /// Global constant with optional global fallback for namespaced code.
    FetchConst(StrId, Option<StrId>),
    FetchClassName(ClassRef),

    /// Stack: [base if Stack] keys… value → result
    StorePath { base: PathBase, segs: Vec<PathSeg>, mode: WriteMode },
    /// Stack: [base if Stack] keys…
    UnsetPath { base: PathBase, segs: Vec<PathSeg> },
    /// Stack: slot-less increments on a plain variable.
    IncSlot { slot: Slot, mode: WriteMode },

    CallFunc { name: StrId, fallback: Option<StrId>, spec: u32 },
    /// obj, args… → result
    CallMethod { name: StrId, spec: u32 },
    /// obj, name, args… → result
    CallMethodDyn { spec: u32 },
    CallStatic { class: ClassRef, name: StrId, spec: u32 },
    /// callable, args… → result
    CallValue { spec: u32 },
    New { class: ClassRef, spec: u32 },
    /// Pops the value.
    Return,
    /// Return inside `try { } finally { }`: parks the value and jumps.
    ReturnViaFinally(Label),

    MakeClosure { unit: UnitId, captures: Vec<Slot>, bind_this: bool },
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
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: StrId,
    pub slot: Slot,
    pub ty: Option<TypeHint>,
    pub has_default: bool,
    pub variadic: bool,
    pub promote: Option<Visibility>,
    pub promote_readonly: bool,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Catch {
    pub classes: Vec<StrId>,
    pub slot: Option<Slot>,
    pub target: Label,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TryRegion {
    pub start: Label,
    pub end: Label,
    pub catches: Vec<Catch>,
    pub finally: Option<Label>,
}

#[derive(Debug, Clone, PartialEq)]
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
}

impl CodeUnit {
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

/// A value computed at link time: either a constant or a tiny code unit.
#[derive(Debug, Clone, PartialEq)]
pub enum Init {
    Const(ConstId),
    Unit(UnitId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct AttributeDecl {
    pub name: StrId,
    pub args: Vec<(Option<StrId>, Init)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ClassConstDecl {
    pub name: StrId,
    pub value: Init,
    pub visibility: Visibility,
    pub is_final: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PropDecl {
    pub name: StrId,
    pub default: Option<Init>,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_readonly: bool,
    pub ty: Option<TypeHint>,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MethodDecl {
    pub name: StrId,
    pub unit: Option<UnitId>, // None = abstract
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_final: bool,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnumCaseDecl {
    pub name: StrId,
    pub value: Option<Init>,
}

#[derive(Debug, Clone, PartialEq)]
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

#[derive(Debug, Clone, PartialEq)]
pub struct FunctionDecl {
    pub name: StrId,
    pub unit: UnitId,
    pub attributes: Vec<AttributeDecl>,
}

#[derive(Debug, Clone, PartialEq, Default)]
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
