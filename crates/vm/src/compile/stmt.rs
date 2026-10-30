//! Statements, functions, closures, loops, try/catch.

use mago_span::HasSpan;
use mago_syntax::cst::*;

use super::dialect;
use super::expr::{Want, var_name};
use super::{CResult, Compiler, LoopCtx, TryCtx};
use crate::bytecode::*;

pub(crate) enum FnBody<'a, 'arena> {
    Block(&'a Block<'arena>),
    Expr(&'a Expression<'arena>),
    /// No body: the signature of an abstract or interface method (for the
    /// inheritance checks and reflection).
    Signature,
}

pub(crate) struct FnSpec<'a, 'arena> {
    pub name: Vec<u8>,
    pub display: String,
    pub params: &'a FunctionLikeParameterList<'arena>,
    pub ret: Option<&'a FunctionLikeReturnTypeHint<'arena>>,
    pub body: FnBody<'a, 'arena>,
    pub is_method: bool,
    pub is_closure: bool,
    /// Closure `use` variables (without `$`), get slots after the parameters.
    pub uses: Vec<Vec<u8>>,
    pub line: u32,
}

impl Compiler {
    pub(crate) fn compile_statements<'a, 'arena: 'a>(
        &mut self,
        stmts: impl Iterator<Item = &'a Statement<'arena>>,
    ) -> CResult<()> {
        for s in stmts {
            match self.compile_statement(s) {
                Ok(()) => {}
                // report mode: note the error, go on with the next statement
                Err(e) if self.report.is_some() => self.report.as_mut().expect("report").push(e),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub(crate) fn compile_statement(&mut self, s: &Statement<'_>) -> CResult<()> {
        let line = self.line(s.span());
        if !matches!(s, Statement::OpeningTag(_) | Statement::ClosingTag(_) | Statement::Noop(_)) {
            self.ub().mark_line(line);
        }
        match s {
            Statement::OpeningTag(_) | Statement::ClosingTag(_) | Statement::Noop(_) => Ok(()),
            Statement::Inline(i) => {
                if i.value.iter().all(u8::is_ascii_whitespace) {
                    Ok(())
                } else {
                    self.reject(dialect::INLINE_HTML, i.span())
                }
            }
            Statement::Namespace(n) => self.compile_namespace(n),
            Statement::Use(u) => self.compile_use(u),
            Statement::Class(c) => self.compile_class(c),
            Statement::Interface(i) => self.compile_interface(i),
            Statement::Trait(t) => self.compile_trait(t),
            Statement::Enum(e) => self.compile_enum(e),
            Statement::Block(b) => self.compile_statements(b.statements.iter()),
            Statement::Constant(c) => self.reject_named(dialect::FORBIDDEN_BUILTIN, b"const (global)", c.span()),
            Statement::Function(f) => self.compile_function_decl(f),
            Statement::Declare(d) => self.compile_declare(d),
            Statement::Goto(g) => self.reject(dialect::GOTO, g.span()),
            Statement::Label(l) => self.reject(dialect::GOTO, l.span()),
            Statement::Try(t) => self.compile_try(t),
            Statement::Foreach(f) => self.compile_foreach(f),
            Statement::For(f) => self.compile_for(f),
            Statement::While(w) => self.compile_while(w),
            Statement::DoWhile(d) => self.compile_do_while(d),
            Statement::Continue(c) => self.compile_break_continue(c.level, false, c.span()),
            Statement::Break(b) => self.compile_break_continue(b.level, true, b.span()),
            Statement::Switch(sw) => self.compile_switch(sw),
            Statement::If(i) => self.compile_if(i),
            Statement::Return(r) => {
                let kind = match r.value {
                    None => 0,
                    Some(Expression::Literal(Literal::Null(_))) => 1,
                    Some(_) => 2,
                };
                self.ub().returns.push((r.span(), kind));
                self.compile_return(r.value)
            }
            Statement::Expression(e) => {
                self.check_discarded_result(e.expression)?;
                self.compile_expr_w(e.expression, Want::Discard)
            }
            Statement::Echo(e) => {
                for v in e.values.iter() {
                    self.compile_expr(v)?;
                    self.emit(Op::Echo);
                }
                Ok(())
            }
            Statement::EchoTag(e) => {
                for v in e.values.iter() {
                    self.compile_expr(v)?;
                    self.emit(Op::Echo);
                }
                Ok(())
            }
            Statement::Global(g) => self.reject(dialect::GLOBAL, g.span()),
            Statement::Static(st) => self.reject(dialect::STATIC_VAR, st.span()),
            Statement::HaltCompiler(h) => self.reject(dialect::HALT_COMPILER, h.span()),
            Statement::Unset(u) => {
                for v in u.values.iter() {
                    match v {
                        Expression::Variable(Variable::Direct(d)) => {
                            let slot = self.ub().slot(var_name(d.name));
                            self.emit(Op::UnsetSlot(slot));
                        }
                        other => {
                            let (base, segs) = self.build_path(other)?;
                            self.emit(Op::UnsetPath(Box::new(PathOp { base, segs, mode: WriteMode::Assign })));
                        }
                    }
                }
                Ok(())
            }
            _ => self.err(s.span(), "unsupported statement"),
        }
    }

    /// `sort($a);` as a statement silently loses the sorted array (GRD020).
    fn check_discarded_result(&self, e: &Expression<'_>) -> CResult<()> {
        let mut cur = e;
        while let Expression::Parenthesized(p) = cur {
            cur = p.expression;
        }
        if let Expression::Call(Call::Function(f)) = cur
            && let Expression::Identifier(id) = f.function
        {
            let raw = id.value();
            let bare = raw.strip_prefix(b"\\").unwrap_or(raw);
            if !bare.contains(&b'\\') && dialect::returns_instead_of_byref(bare) {
                return self.reject_named(dialect::DISCARDED_RESULT, bare, id.span());
            }
        }
        Ok(())
    }

    fn compile_namespace(&mut self, n: &Namespace<'_>) -> CResult<()> {
        let name = n.name.as_ref().map(|i| i.value());
        self.names.enter_namespace(name);
        match &n.body {
            NamespaceBody::Implicit(b) => self.compile_statements(b.statements.iter()),
            NamespaceBody::BraceDelimited(b) => {
                self.compile_statements(b.statements.iter())?;
                self.names.enter_namespace(None);
                Ok(())
            }
        }
    }

    fn compile_use(&mut self, u: &Use<'_>) -> CResult<()> {
        fn alias<'a>(item: &'a UseItem<'_>) -> Option<&'a [u8]> {
            item.alias.as_ref().map(|a| a.identifier.value)
        }
        match &u.items {
            UseItems::Sequence(seq) => {
                for item in seq.items.iter() {
                    self.names.use_class(item.name.value(), alias(item));
                }
            }
            UseItems::TypedSequence(ts) => {
                for item in ts.items.iter() {
                    match ts.r#type {
                        UseType::Function(_) => self.names.use_function(item.name.value(), alias(item)),
                        UseType::Const(_) => self.names.use_const(item.name.value(), alias(item)),
                    }
                }
            }
            UseItems::TypedList(tl) => {
                for item in tl.items.iter() {
                    let full = join_ns(tl.namespace.value(), item.name.value());
                    match tl.r#type {
                        UseType::Function(_) => self.names.use_function(&full, alias(item)),
                        UseType::Const(_) => self.names.use_const(&full, alias(item)),
                    }
                }
            }
            UseItems::MixedList(ml) => {
                for mt in ml.items.iter() {
                    let full = join_ns(ml.namespace.value(), mt.item.name.value());
                    match mt.r#type {
                        Some(UseType::Function(_)) => self.names.use_function(&full, alias(&mt.item)),
                        Some(UseType::Const(_)) => self.names.use_const(&full, alias(&mt.item)),
                        None => self.names.use_class(&full, alias(&mt.item)),
                    }
                }
            }
        }
        Ok(())
    }

    fn compile_declare(&mut self, d: &Declare<'_>) -> CResult<()> {
        for item in d.items.iter() {
            if !item.name.value.eq_ignore_ascii_case(b"strict_types") {
                return self.reject(dialect::DECLARE, item.span());
            }
        }
        match &d.body {
            DeclareBody::Statement(s) => self.compile_statement(s),
            DeclareBody::ColonDelimited(b) => self.compile_statements(b.statements.iter()),
        }
    }

    // ---- functions ------------------------------------------------------------

    fn compile_function_decl(&mut self, f: &Function<'_>) -> CResult<()> {
        if let Some(a) = f.ampersand {
            return self.reject(dialect::REFERENCE, a);
        }
        let fq = self.names.declare(f.name.value);
        let line = self.line(f.span());
        let spec = FnSpec {
            name: fq.clone(),
            display: String::from_utf8_lossy(&fq).into_owned(),
            params: &f.parameter_list,
            ret: f.return_type_hint.as_ref(),
            body: FnBody::Block(&f.body),
            is_method: false,
            is_closure: false,
            uses: Vec::new(),
            line,
        };
        if let Some(prev) = self.declared_functions.get(&fq.to_ascii_lowercase()) {
            return self.err(f.span(), format!("Cannot redeclare function {}() (previously declared in {}:{prev})", String::from_utf8_lossy(&fq), self.module.file));
        }
        self.declared_functions.insert(fq.to_ascii_lowercase(), line);
        let unit = self.compile_function_like(spec)?;
        let attributes = self.compile_attributes(&f.attribute_lists)?;
        let name = self.intern(&fq);
        self.module.functions.push(FunctionDecl { name, unit, attributes });
        Ok(())
    }

    /// Compiles a function-like body into a new unit and returns its id.
    pub(crate) fn compile_function_like(&mut self, spec: FnSpec<'_, '_>) -> CResult<UnitId> {
        self.push_unit(spec.display.clone(), spec.line);
        let mixed = self.mixed_vars.get(&spec.params.span().start.offset).cloned().unwrap_or_default();
        {
            let ub = self.ub();
            ub.mixed = mixed;
            ub.function_name = spec.name.clone();
            ub.is_method = spec.is_method;
            ub.is_closure = spec.is_closure;
        }
        // parameters occupy slots 0..n in order
        let mut params: Vec<Param> = Vec::new();
        let mut required = 0u32;
        let mut seen_optional = false;
        for (index, p) in spec.params.parameters.iter().enumerate() {
            if let Some(a) = p.ampersand {
                return self.reject(dialect::REFERENCE, a);
            }
            if p.hooks.is_some() {
                return self.err(p.span(), "property hooks are not supported");
            }
            let pname = var_name(p.variable.name).to_vec();
            if params.iter().any(|q| self.module.str(q.name) == pname.as_slice()) {
                return self.err(p.span(), format!("Redefinition of parameter ${}", String::from_utf8_lossy(&pname)));
            }
            let slot = self.ub().slot(&pname);
            let ty = match &p.hint {
                Some(h) => Some(self.type_hint(h)?),
                None => None,
            };
            let mut promote = None;
            let mut promote_readonly = false;
            for m in p.modifiers.iter() {
                match m {
                    Modifier::Public(_) => promote = Some(Visibility::Public),
                    Modifier::Protected(_) => promote = Some(Visibility::Protected),
                    Modifier::Private(_) => promote = Some(Visibility::Private),
                    Modifier::Readonly(_) => {
                        promote_readonly = true;
                        if promote.is_none() {
                            promote = Some(Visibility::Public);
                        }
                    }
                    other => return self.err(other.span(), "invalid parameter modifier"),
                }
            }
            let variadic = p.ellipsis.is_some();
            // PHP's rules for promoted constructor parameters
            if promote.is_some() {
                let is_ctor = spec.is_method && spec.name.eq_ignore_ascii_case(b"__construct");
                if !is_ctor {
                    return self.err(p.span(), "Cannot declare promoted property outside a constructor");
                }
                if matches!(spec.body, FnBody::Signature) {
                    return self.err(p.span(), "Cannot declare promoted property in an abstract constructor");
                }
                if variadic {
                    return self.err(p.span(), "Cannot declare variadic promoted property");
                }
                if matches!(ty, Some(TypeHint::Callable)) {
                    let class = String::from_utf8_lossy(&self.current_class().map(|c| c.name.clone()).unwrap_or_default()).into_owned();
                    return self.err(p.span(), format!("Property {class}::${} cannot have type callable", String::from_utf8_lossy(&pname)));
                }
            }
            let has_default = p.default_value.is_some();
            if !has_default && !variadic && !seen_optional {
                required = index as u32 + 1;
            }
            if has_default || variadic {
                seen_optional = true;
            }
            let attributes = self.compile_attributes(&p.attribute_lists)?;
            let default_init = match &p.default_value {
                Some(dv) => Some(self.init_expr(dv.value)?),
                None => None,
            };
            let name = self.intern(&pname);
            // GRD021 null rule: a typed, non-nullable parameter without a null default
            let default_null = matches!(p.default_value.as_ref().map(|d| d.value), Some(Expression::Literal(Literal::Null(_))));
            let not_null = !default_null && matches!(&ty, Some(t) if !matches!(t, TypeHint::Mixed | TypeHint::Null | TypeHint::Nullable(_) | TypeHint::Union(_)));
            self.ub().note_assignment(slot, not_null);
            params.push(Param { name, slot, ty, has_default, default_init, variadic, promote, promote_readonly, attributes });
        }
        // default-value prologue
        for (index, p) in spec.params.parameters.iter().enumerate() {
            if let Some(dv) = &p.default_value {
                let slot = params[index].slot;
                let j = self.emit(Op::JumpIfArgPresent { index: index as u32, target: 0 });
                self.compile_expr(dv.value)?;
                self.emit(Op::StoreSlot(slot));
                let l = self.ub().here();
                self.ub().patch(j, l);
            }
        }
        for p in &params {
            if p.promote.is_some() {
                self.ub().unit.uses_this = true;
                self.emit(Op::PromoteParam { slot: p.slot, prop: p.name });
            }
        }
        // closure `use` variables follow the parameters
        for u in &spec.uses {
            self.ub().slot(u);
        }
        let ret = match spec.ret {
            Some(r) => Some(self.type_hint(&r.hint)?),
            None => None,
        };
        {
            let ub = self.ub();
            ub.unit.params = params;
            ub.unit.required_params = required;
            ub.unit.return_type = ret;
        }
        match spec.body {
            FnBody::Block(b) => self.compile_statements(b.statements.iter())?,
            FnBody::Expr(e) => {
                // an arrow function returns its expression
                self.ub().returns.push((e.span(), 2));
                self.compile_expr(e)?;
                self.emit(Op::Return);
            }
            FnBody::Signature => {}
        }
        self.check_returns()?;
        // record slot names for reflection/debugging
        let mut names: Vec<(Slot, Vec<u8>)> = self.ub().slots.iter().map(|(k, v)| (*v, k.clone())).collect();
        names.sort();
        let mut slot_names = vec![0; self.ub().unit.num_slots as usize];
        for (slot, n) in names {
            let id = self.intern(&n);
            slot_names[slot as usize] = id;
        }
        self.ub().unit.slot_names = slot_names;
        Ok(self.pop_unit())
    }

    pub(crate) fn compile_closure(&mut self, c: &Closure<'_>) -> CResult<()> {
        if let Some(s) = &c.r#static {
            return self.reject(dialect::STATIC_CLOSURE, s.span());
        }
        if let Some(a) = c.ampersand {
            return self.reject(dialect::REFERENCE, a);
        }
        let mut uses = Vec::new();
        if let Some(u) = &c.use_clause {
            for v in u.variables.iter() {
                if let Some(a) = v.ampersand {
                    return self.reject(dialect::REFERENCE, a);
                }
                uses.push(var_name(v.variable.name).to_vec());
            }
        }
        let line = self.line(c.span());
        let spec = FnSpec {
            name: b"{closure}".to_vec(),
            display: "{closure}".to_string(),
            params: &c.parameter_list,
            ret: c.return_type_hint.as_ref(),
            body: FnBody::Block(&c.body),
            is_method: self.ub().is_method,
            is_closure: true,
            uses: uses.clone(),
            line,
        };
        let unit = self.compile_function_like(spec)?;
        let bind_this = self.module.units[unit as usize].uses_this;
        let mut captures = Vec::new();
        for u in &uses {
            captures.push(self.ub().slot(u));
        }
        if bind_this {
            self.ub().unit.uses_this = true;
        }
        self.emit(Op::MakeClosure(Box::new(ClosureOp { unit, captures, bind_this })));
        Ok(())
    }

    pub(crate) fn compile_arrow_function(&mut self, f: &ArrowFunction<'_>) -> CResult<()> {
        if let Some(s) = &f.r#static {
            return self.reject(dialect::STATIC_CLOSURE, s.span());
        }
        if let Some(a) = f.ampersand {
            return self.reject(dialect::REFERENCE, a);
        }
        let param_names: Vec<Vec<u8>> =
            f.parameter_list.parameters.iter().map(|p| var_name(p.variable.name).to_vec()).collect();
        let used = super::capture::variables_in(f.expression);
        let mut uses = Vec::new();
        for name in used {
            if param_names.contains(&name) {
                continue;
            }
            if self.ub().lookup_slot(&name).is_some() {
                uses.push(name);
            }
        }
        let line = self.line(f.span());
        let spec = FnSpec {
            name: b"{closure}".to_vec(),
            display: "{closure}".to_string(),
            params: &f.parameter_list,
            ret: f.return_type_hint.as_ref(),
            body: FnBody::Expr(f.expression),
            is_method: self.ub().is_method,
            is_closure: true,
            uses: uses.clone(),
            line,
        };
        let unit = self.compile_function_like(spec)?;
        let bind_this = self.module.units[unit as usize].uses_this;
        let mut captures = Vec::new();
        for u in &uses {
            captures.push(self.ub().slot(u));
        }
        if bind_this {
            self.ub().unit.uses_this = true;
        }
        self.emit(Op::MakeClosure(Box::new(ClosureOp { unit, captures, bind_this })));
        Ok(())
    }

    /// PHP's compile errors for `return` against the declared return type
    /// (generators excepted, whose type must admit Generator).
    fn check_returns(&mut self) -> CResult<()> {
        let ub = self.ub();
        let (Some(ret), returns, is_gen) = (ub.unit.return_type.clone(), std::mem::take(&mut ub.returns), ub.unit.is_generator) else { return Ok(()) };
        if is_gen {
            let ok = match &ret {
                TypeHint::Mixed | TypeHint::Object | TypeHint::Iterable => true,
                TypeHint::Class(n) => matches!(self.module.str(*n).to_ascii_lowercase().as_slice(), b"generator" | b"iterator" | b"traversable"),
                _ => false,
            };
            if !ok {
                let span = returns.first().map(|r| r.0);
                let msg = format!("Generator return type must be a supertype of Generator, {} given", type_display(self, &ret));
                return match span { Some(s) => self.err(s, msg), None => Err(super::CompileError { code: None, message: msg, hint: None, line: self.ub().unit.line, file: self.module.file.clone() }) };
            }
            return Ok(());
        }
        let nullable = match &ret {
            TypeHint::Mixed | TypeHint::Null | TypeHint::Nullable(_) => true,
            TypeHint::Union(p) => p.iter().any(|t| matches!(t, TypeHint::Null)),
            _ => false,
        };
        for (span, kind) in returns {
            match (&ret, kind) {
                (TypeHint::Void, 2) => return self.err(span, "A void function must not return a value"),
                (TypeHint::Void, 1) => return self.err(span, "A void function must not return a value (did you mean \"return;\" instead of \"return null;\"?)"),
                (TypeHint::Void, _) => {}
                (TypeHint::Never, _) => return self.err(span, "A never-returning function must not return"),
                (_, 0) if nullable => return self.err(span, "A function with return type must return a value (did you mean \"return null;\" instead of \"return;\"?)"),
                (_, 0) => return self.err(span, "A function with return type must return a value"),
                _ => {}
            }
        }
        Ok(())
    }

    fn compile_return(&mut self, value: Option<&Expression<'_>>) -> CResult<()> {
        match value {
            Some(v) => self.compile_expr(v)?,
            None => {
                self.emit(Op::PushNull);
            }
        }
        // return through enclosing finally blocks, innermost first
        let innermost = self.ub().tries.iter().rposition(|t| t.has_finally);
        match innermost {
            Some(i) => {
                let at = self.emit(Op::ReturnViaFinally(0));
                self.ub().tries[i].finally_patches.push(at);
            }
            None => {
                self.emit(Op::Return);
            }
        }
        Ok(())
    }

    // ---- control flow ---------------------------------------------------------

    fn compile_if(&mut self, i: &If<'_>) -> CResult<()> {
        let mut end_jumps = Vec::new();
        let mut next = self.compile_false_jumps(i.condition)?;
        match &i.body {
            IfBody::Statement(b) => {
                self.compile_statement(b.statement)?;
                let has_more = !b.else_if_clauses.is_empty() || b.else_clause.is_some();
                if has_more {
                    end_jumps.push(self.emit(Op::Jump(0)));
                }
                for (k, ei) in b.else_if_clauses.iter().enumerate() {
                    let l = self.ub().here();
                    self.patch_all(&next, l);
                    next = self.compile_false_jumps(ei.condition)?;
                    self.compile_statement(ei.statement)?;
                    if k + 1 < b.else_if_clauses.len() || b.else_clause.is_some() {
                        end_jumps.push(self.emit(Op::Jump(0)));
                    }
                }
                let l = self.ub().here();
                self.patch_all(&next, l);
                if let Some(e) = &b.else_clause {
                    self.compile_statement(e.statement)?;
                }
            }
            IfBody::ColonDelimited(b) => {
                self.compile_statements(b.statements.iter())?;
                let has_more = !b.else_if_clauses.is_empty() || b.else_clause.is_some();
                if has_more {
                    end_jumps.push(self.emit(Op::Jump(0)));
                }
                for (k, ei) in b.else_if_clauses.iter().enumerate() {
                    let l = self.ub().here();
                    self.patch_all(&next, l);
                    next = self.compile_false_jumps(ei.condition)?;
                    self.compile_statements(ei.statements.iter())?;
                    if k + 1 < b.else_if_clauses.len() || b.else_clause.is_some() {
                        end_jumps.push(self.emit(Op::Jump(0)));
                    }
                }
                let l = self.ub().here();
                self.patch_all(&next, l);
                if let Some(e) = &b.else_clause {
                    self.compile_statements(e.statements.iter())?;
                }
            }
        }
        let end = self.ub().here();
        for j in end_jumps {
            self.ub().patch(j, end);
        }
        Ok(())
    }

    fn push_loop(&mut self, is_switch: bool) {
        let tries_depth = self.ub().tries.len();
        self.ub().loops.push(LoopCtx { break_patches: Vec::new(), continue_patches: Vec::new(), tries_depth, is_switch });
    }

    /// Pops the loop context; patches breaks to `end` and continues to `cont`.
    fn pop_loop(&mut self, cont: u32, end: u32) {
        let ctx = self.ub().loops.pop().expect("loop ctx");
        for j in ctx.break_patches {
            self.ub().patch(j, end);
        }
        for j in ctx.continue_patches {
            self.ub().patch(j, cont);
        }
    }

    fn compile_break_continue(&mut self, level: Option<&Expression<'_>>, is_break: bool, span: mago_span::Span) -> CResult<()> {
        let mut depth = 1usize;
        if let Some(l) = level {
            match l {
                Expression::Literal(Literal::Integer(i)) if i.value.is_some_and(|v| v >= 1) => depth = i.value.unwrap() as usize,
                _ => return self.err(span, "`break`/`continue` brauchen eine positive Ganzzahl"),
            }
        }
        let loops = self.ub().loops.len();
        if depth > loops {
            return self.err(span, "`break`/`continue` outside a loop");
        }
        let idx = loops - depth;
        // a `finally` between here and the loop runs first (innermost only)
        let loop_tries = self.ub().loops[idx].tries_depth;
        let via_finally = self.ub().tries.iter().enumerate().rev().find(|(ti, t)| *ti >= loop_tries && t.has_finally).map(|(ti, _)| ti);
        let j = match via_finally {
            Some(ti) => {
                let at = self.emit(Op::JumpViaFinally { finally: 0, target: 0 });
                self.ub().tries[ti].finally_patches.push(at);
                at
            }
            None => self.emit(Op::Jump(0)),
        };
        let ctx = &mut self.ub().loops[idx];
        // PHP: `continue` inside a switch acts like `break` of that switch
        if is_break || ctx.is_switch {
            ctx.break_patches.push(j);
        } else {
            ctx.continue_patches.push(j);
        }
        Ok(())
    }

    fn compile_while(&mut self, w: &While<'_>) -> CResult<()> {
        let cond = self.ub().here();
        let exit = self.compile_false_jumps(w.condition)?;
        self.push_loop(false);
        match &w.body {
            WhileBody::Statement(s) => self.compile_statement(s)?,
            WhileBody::ColonDelimited(b) => self.compile_statements(b.statements.iter())?,
        }
        self.emit(Op::Jump(cond));
        let end = self.ub().here();
        self.patch_all(&exit, end);
        self.pop_loop(cond, end);
        Ok(())
    }

    fn compile_do_while(&mut self, d: &DoWhile<'_>) -> CResult<()> {
        let start = self.ub().here();
        self.push_loop(false);
        self.compile_statement(d.statement)?;
        let cond = self.ub().here();
        self.compile_expr(d.condition)?;
        self.emit(Op::JumpIfTrue(start));
        let end = self.ub().here();
        self.pop_loop(cond, end);
        Ok(())
    }

    fn compile_for(&mut self, f: &For<'_>) -> CResult<()> {
        for e in f.initializations.iter() {
            self.compile_expr_w(e, Want::Discard)?;
        }
        let cond = self.ub().here();
        let mut exit = None;
        let n = f.conditions.len();
        for (k, e) in f.conditions.iter().enumerate() {
            if k + 1 == n {
                exit = Some(self.compile_false_jumps(e)?);
            } else {
                self.compile_expr_w(e, Want::Discard)?;
            }
        }
        self.push_loop(false);
        match &f.body {
            ForBody::Statement(s) => self.compile_statement(s)?,
            ForBody::ColonDelimited(b) => self.compile_statements(b.statements.iter())?,
        }
        let cont = self.ub().here();
        for e in f.increments.iter() {
            self.compile_expr_w(e, Want::Discard)?;
        }
        self.emit(Op::Jump(cond));
        let end = self.ub().here();
        if let Some(x) = exit {
            self.patch_all(&x, end);
        }
        self.pop_loop(cont, end);
        Ok(())
    }

    fn compile_foreach(&mut self, f: &Foreach<'_>) -> CResult<()> {
        self.compile_expr(f.expression)?;
        let iter = self.ub().temp_slot();
        self.emit(Op::IterInit(iter));
        let (key_target, value_target) = match &f.target {
            ForeachTarget::Value(v) => (None, v.value),
            ForeachTarget::KeyValue(kv) => (Some(kv.key), kv.value),
        };
        if let Expression::UnaryPrefix(u) = value_target
            && matches!(u.operator, UnaryPrefixOperator::Reference(_))
        {
            return self.reject(dialect::REFERENCE, u.span());
        }
        let top = self.ub().here();
        let next = self.emit(Op::IterNext { iter, end: 0, with_key: key_target.is_some() });
        // stack: [key,] value; foreach variables are bound per iteration
        // and not subject to GRD021 (D10)
        self.compile_bind_target(value_target)?;
        if let Some(k) = key_target {
            self.compile_bind_target(k)?;
        }
        self.push_loop(false);
        match &f.body {
            ForeachBody::Statement(s) => self.compile_statement(s)?,
            ForeachBody::ColonDelimited(b) => self.compile_statements(b.statements.iter())?,
        }
        self.emit(Op::Jump(top));
        let end = self.ub().here();
        self.ub().patch(next, end);
        self.pop_loop(top, end);
        self.emit(Op::UnsetSlot(iter));
        Ok(())
    }

    /// Like `compile_store_target`, but a plain variable is bound without
    /// the GRD021 type rule (`foreach` variables).
    fn compile_bind_target(&mut self, target: &Expression<'_>) -> CResult<()> {
        if let Expression::Variable(Variable::Direct(d)) = target {
            let slot = self.ub().slot(var_name(d.name));
            self.emit(Op::BindSlot(slot));
            return Ok(());
        }
        self.compile_store_target(target)
    }

    /// Pops the value on the stack into an assignment target (variable,
    /// path or destructuring pattern).
    fn compile_store_target(&mut self, target: &Expression<'_>) -> CResult<()> {
        match target {
            Expression::Variable(Variable::Direct(d)) => {
                let slot = self.ub().slot(var_name(d.name));
                self.ub().note_assignment(slot, false);
                self.emit(Op::StoreSlot(slot));
                Ok(())
            }
            Expression::Array(arr) => {
                self.compile_destructure(&arr.elements)?;
                self.emit(Op::Pop);
                Ok(())
            }
            Expression::List(l) => self.reject(dialect::LIST, l.span()),
            other => {
                let tmp = self.ub().temp_slot();
                self.emit(Op::BindSlot(tmp));
                let (base, segs) = self.build_path(other)?;
                self.emit(Op::LoadSlot(tmp));
                self.emit(Op::StorePath(Box::new(PathOp { base, segs, mode: WriteMode::Assign })));
                self.emit(Op::Pop);
                Ok(())
            }
        }
    }

    fn compile_switch(&mut self, sw: &Switch<'_>) -> CResult<()> {
        let subject = self.ub().temp_slot();
        self.compile_expr(sw.expression)?;
        self.emit(Op::BindSlot(subject));
        let cases: Vec<&SwitchCase<'_>> = match &sw.body {
            SwitchBody::BraceDelimited(b) => b.cases.iter().collect(),
            SwitchBody::ColonDelimited(b) => b.cases.iter().collect(),
        };
        let mut case_jumps: Vec<(usize, usize)> = Vec::new(); // (jump index, case index)
        let mut default_index = None;
        for (ci, c) in cases.iter().enumerate() {
            match c {
                SwitchCase::Expression(e) => {
                    self.emit(Op::LoadSlot(subject));
                    self.compile_expr(e.expression)?;
                    self.emit(Op::Bin(BinOp::Identical));
                    let j = self.emit(Op::JumpIfTrue(0));
                    case_jumps.push((j, ci));
                }
                SwitchCase::Default(d) => {
                    if default_index.is_some() {
                        return self.err(d.span(), "Switch statements may only contain one default clause");
                    }
                    default_index = Some(ci)
                }
            }
        }
        let to_default = self.emit(Op::Jump(0));
        self.push_loop(true);
        let mut labels = vec![0u32; cases.len()];
        for (ci, c) in cases.iter().enumerate() {
            labels[ci] = self.ub().here();
            match c {
                SwitchCase::Expression(e) => self.compile_statements(e.statements.iter())?,
                SwitchCase::Default(d) => self.compile_statements(d.statements.iter())?,
            }
        }
        let end = self.ub().here();
        for (j, ci) in case_jumps {
            self.ub().patch(j, labels[ci]);
        }
        match default_index {
            Some(di) => self.ub().patch(to_default, labels[di]),
            None => self.ub().patch(to_default, end),
        }
        self.pop_loop(end, end);
        Ok(())
    }

    // ---- try / catch / finally -----------------------------------------------

    fn compile_try(&mut self, t: &Try<'_>) -> CResult<()> {
        let has_finally = t.finally_clause.is_some();
        self.ub().tries.push(TryCtx { finally_patches: Vec::new(), has_finally });
        let start = self.ub().here();
        self.compile_statements(t.block.statements.iter())?;
        let end = self.ub().here();
        let mut exit_jumps = vec![self.emit(Op::Jump(0))];
        let catch_start = self.ub().here();
        let mut catches = Vec::new();
        for clause in t.catch_clauses.iter() {
            let target = self.ub().here();
            let mut classes = Vec::new();
            self.catch_classes(&clause.hint, &mut classes)?;
            let slot = clause.variable.as_ref().map(|v| self.ub().slot(var_name(v.name)));
            catches.push(Catch { classes, slot, target });
            self.compile_statements(clause.block.statements.iter())?;
            exit_jumps.push(self.emit(Op::Jump(0)));
        }
        let catch_end = self.ub().here();
        let ctx = self.ub().tries.pop().expect("try ctx");
        let finally_label = self.ub().here();
        for j in exit_jumps {
            self.ub().patch(j, finally_label);
        }
        for j in &ctx.finally_patches {
            if let Op::JumpViaFinally { finally, .. } = &mut self.ub().unit.ops[*j] {
                *finally = finally_label;
            } else {
                self.ub().patch(*j, finally_label);
            }
        }
        if let Some(fc) = &t.finally_clause {
            self.compile_statements(fc.block.statements.iter())?;
            let fe = self.emit(Op::FinallyEnd { outer: None });
            // chain to the enclosing finally, if any
            if let Some(outer) = self.ub().tries.iter_mut().rev().find(|t| t.has_finally) {
                outer.finally_patches.push(fe);
            }
        }
        let finally = has_finally.then_some(finally_label);
        self.ub().unit.try_regions.push(TryRegion { start, end, catches, finally });
        if has_finally && catch_end > catch_start {
            // exceptions thrown inside catch blocks still run the finally block
            self.ub().unit.try_regions.push(TryRegion { start: catch_start, end: catch_end, catches: Vec::new(), finally });
        }
        Ok(())
    }

    fn catch_classes(&mut self, hint: &Hint<'_>, out: &mut Vec<StrId>) -> CResult<()> {
        match hint {
            Hint::Identifier(id) => {
                let fq = self.names.resolve_class(id.value());
                out.push(self.intern(&fq));
                Ok(())
            }
            Hint::Union(u) => {
                self.catch_classes(u.left, out)?;
                self.catch_classes(u.right, out)
            }
            Hint::Parenthesized(p) => self.catch_classes(p.hint, out),
            other => self.err(other.span(), "invalid type in `catch`"),
        }
    }

    // ---- types and attributes ---------------------------------------------------

    pub(crate) fn type_hint(&mut self, h: &Hint<'_>) -> CResult<TypeHint> {
        Ok(match h {
            Hint::Identifier(id) => {
                let fq = self.names.resolve_class(id.value());
                // self/static/parent need a class, except in closures (they can be bound)
                if matches!(fq.as_slice(), b"self" | b"static" | b"parent") && self.current_class().is_none() && !self.units.last().is_some_and(|u| u.is_closure) {
                    return self.err(h.span(), format!("Cannot use \"{}\" when no class scope is active", String::from_utf8_lossy(&fq)));
                }
                match fq.as_slice() {
                    b"self" => TypeHint::SelfT,
                    b"static" => TypeHint::Static,
                    b"parent" => TypeHint::Parent,
                    b"int" => TypeHint::Int,
                    b"float" => TypeHint::Float,
                    b"string" => TypeHint::String,
                    b"bool" => TypeHint::Bool,
                    b"array" => TypeHint::Array,
                    b"object" => TypeHint::Object,
                    b"mixed" => TypeHint::Mixed,
                    b"void" => TypeHint::Void,
                    b"never" => TypeHint::Never,
                    b"null" => TypeHint::Null,
                    b"callable" => TypeHint::Callable,
                    b"iterable" => TypeHint::Iterable,
                    b"true" => TypeHint::True,
                    b"false" => TypeHint::False,
                    _ => TypeHint::Class(self.intern(&fq)),
                }
            }
            Hint::Parenthesized(p) => self.type_hint(p.hint)?,
            Hint::Nullable(n) => {
                let inner = self.type_hint(n.hint)?;
                // PHP's compile errors for ?T
                match inner {
                    TypeHint::Mixed => return self.err(h.span(), "Type mixed cannot be marked as nullable since mixed already includes null"),
                    TypeHint::Void => return self.err(h.span(), "Void can only be used as a standalone type"),
                    TypeHint::Never => return self.err(h.span(), "never can only be used as a standalone type"),
                    TypeHint::Null => return self.err(h.span(), "null cannot be marked as nullable"),
                    _ => {}
                }
                TypeHint::Nullable(Box::new(inner))
            }
            Hint::Union(u) => {
                let mut parts = Vec::new();
                flatten_union(self, u, &mut parts)?;
                self.check_union(&parts, h.span())?;
                TypeHint::Union(parts)
            }
            Hint::Intersection(i) => TypeHint::Intersection(vec![self.type_hint(i.left)?, self.type_hint(i.right)?]),
            Hint::Null(_) => TypeHint::Null,
            Hint::True(_) => TypeHint::True,
            Hint::False(_) => TypeHint::False,
            Hint::Array(_) => TypeHint::Array,
            Hint::Callable(_) => TypeHint::Callable,
            Hint::Static(_) | Hint::Self_(_) | Hint::Parent(_) if self.current_class().is_none() && !self.units.last().is_some_and(|u| u.is_closure) => {
                let w = match h { Hint::Static(_) => "static", Hint::Self_(_) => "self", _ => "parent" };
                return self.err(h.span(), format!("Cannot use \"{w}\" when no class scope is active"));
            }
            Hint::Static(_) => TypeHint::Static,
            Hint::Self_(_) => TypeHint::SelfT,
            Hint::Parent(_) => TypeHint::Parent,
            Hint::Void(_) => TypeHint::Void,
            Hint::Never(_) => TypeHint::Never,
            Hint::Float(_) => TypeHint::Float,
            Hint::Bool(_) => TypeHint::Bool,
            Hint::Integer(_) => TypeHint::Int,
            Hint::String(_) => TypeHint::String,
            Hint::Object(_) => TypeHint::Object,
            Hint::Mixed(_) => TypeHint::Mixed,
            Hint::Iterable(_) => TypeHint::Iterable,
        })
    }

    /// PHP's compile errors for union types: standalone types, duplicates
    /// (iterable counts as array|Traversable), true|false, object with a class.
    fn check_union(&self, parts: &[TypeHint], span: mago_span::Span) -> CResult<()> {
        let mut seen: Vec<Vec<u8>> = Vec::new();
        let (mut has_true, mut has_false, mut has_object, mut first_class) = (false, false, false, None);
        for p in parts {
            let names: Vec<Vec<u8>> = match p {
                TypeHint::Mixed => return self.err(span, "Type mixed can only be used as a standalone type"),
                TypeHint::Void => return self.err(span, "Void can only be used as a standalone type"),
                TypeHint::Never => return self.err(span, "never can only be used as a standalone type"),
                TypeHint::Iterable => vec![b"array".to_vec(), b"Traversable".to_vec()],
                TypeHint::Class(n) => {
                    first_class.get_or_insert(*n);
                    vec![self.module.str(*n).to_vec()]
                }
                TypeHint::Bool => vec![b"bool".to_vec(), b"true".to_vec(), b"false".to_vec()],
                other => vec![type_keyword(other).as_bytes().to_vec()],
            };
            has_true |= matches!(p, TypeHint::True);
            has_false |= matches!(p, TypeHint::False);
            has_object |= matches!(p, TypeHint::Object);
            for n in names {
                if seen.iter().any(|s| s.eq_ignore_ascii_case(&n)) {
                    // PHP names the later member as written; for bool it is the true/false part
                    let shown = match p {
                        TypeHint::Bool => parts.iter().find(|q| matches!(q, TypeHint::True | TypeHint::False)).map(|q| type_keyword(q).as_bytes().to_vec()).unwrap_or(n),
                        _ => n,
                    };
                    return self.err(span, format!("Duplicate type {} is redundant", String::from_utf8_lossy(&shown)));
                }
                seen.push(n);
            }
        }
        if has_true && has_false {
            return self.err(span, "Type contains both true and false, bool must be used instead");
        }
        if has_object && let Some(c) = first_class {
            return self.err(span, format!("Type {}|object contains both object and a class type, which is redundant", String::from_utf8_lossy(self.module.str(c))));
        }
        Ok(())
    }

    pub(crate) fn compile_attributes(&mut self, lists: &Sequence<'_, AttributeList<'_>>) -> CResult<Vec<AttributeDecl>> {
        let mut out = Vec::new();
        for list in lists.iter() {
            for a in list.attributes.iter() {
                let fq = self.names.resolve_class(a.name.value());
                let name = self.intern(&fq);
                let mut args = Vec::new();
                if let Some(al) = &a.argument_list {
                    for arg in al.arguments.iter() {
                        match arg {
                            PartialArgument::Positional(p) => {
                                let init = self.init_expr(p.value)?;
                                args.push((None, init));
                            }
                            PartialArgument::Named(n) => {
                                let init = self.init_expr(n.value)?;
                                let nm = self.intern(n.name.value);
                                args.push((Some(nm), init));
                            }
                            other => return self.err(other.span(), "invalid attribute argument"),
                        }
                    }
                }
                out.push(AttributeDecl { name, args });
            }
        }
        Ok(out)
    }
}

fn flatten_union(c: &mut Compiler, u: &UnionHint<'_>, out: &mut Vec<TypeHint>) -> CResult<()> {
    for side in [u.left, u.right] {
        match side {
            Hint::Union(inner) => flatten_union(c, inner, out)?,
            other => out.push(c.type_hint(other)?),
        }
    }
    Ok(())
}

fn join_ns(ns: &[u8], name: &[u8]) -> Vec<u8> {
    let mut v = ns.strip_suffix(b"\\").unwrap_or(ns).to_vec();
    v.push(b'\\');
    v.extend_from_slice(name);
    v
}

/// The keyword of a built-in type (for messages).
fn type_keyword(t: &TypeHint) -> &'static str {
    match t {
        TypeHint::Mixed => "mixed",
        TypeHint::Null => "null",
        TypeHint::Bool => "bool",
        TypeHint::True => "true",
        TypeHint::False => "false",
        TypeHint::Int => "int",
        TypeHint::Float => "float",
        TypeHint::String => "string",
        TypeHint::Array => "array",
        TypeHint::Object => "object",
        TypeHint::Iterable => "iterable",
        TypeHint::Callable => "callable",
        TypeHint::Void => "void",
        TypeHint::Never => "never",
        TypeHint::Static => "static",
        TypeHint::SelfT => "self",
        TypeHint::Parent => "parent",
        _ => "",
    }
}

/// A type as PHP writes it in messages.
fn type_display(c: &Compiler, t: &TypeHint) -> String {
    match t {
        TypeHint::Class(n) => String::from_utf8_lossy(c.module.str(*n)).into_owned(),
        TypeHint::Nullable(i) => format!("?{}", type_display(c, i)),
        TypeHint::Union(p) => p.iter().map(|x| type_display(c, x)).collect::<Vec<_>>().join("|"),
        TypeHint::Intersection(p) => p.iter().map(|x| type_display(c, x)).collect::<Vec<_>>().join("&"),
        other => type_keyword(other).to_string(),
    }
}

