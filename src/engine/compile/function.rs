//! Function Compilation
//!
//! Handles compilation of PHP function definitions and closures

use super::context::CompileContext;
use super::statement::parse_statement_block;
use crate::engine::lexer::{Lexer, Token, TokenType};
use crate::engine::types::Val;

use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for generating unique closure names
static CLOSURE_COUNTER: AtomicU64 = AtomicU64::new(0);

fn is_punct(token: &Token, ch: &str) -> bool {
    token.token_type == TokenType::T_STRING && token.value.as_ref().map(|s| s.as_str()) == Some(ch)
}

fn is_union_sep(token: &Token) -> bool {
    token.token_type == TokenType::T_STRING && token.value.as_ref().map(|s| s.as_str()) == Some("|")
}

fn is_intersection_sep(token: &Token) -> bool {
    token.token_type == TokenType::T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
        && token.value.as_ref().map(|s| s.as_str()) == Some("&")
}

/// Check if a token is a type hint keyword
fn is_type_hint(token: &Token) -> bool {
    if token.token_type == TokenType::T_STRING
        && let Some(ref val) = token.value
    {
        return matches!(
            val.as_str(),
            "int"
                | "string"
                | "float"
                | "bool"
                | "array"
                | "object"
                | "mixed"
                | "void"
                | "never"
                | "self"
                | "static"
                | "iterable"
        );
    }
    token.token_type == TokenType::T_ARRAY || token.token_type == TokenType::T_CALLABLE
}

/// Parsed parameter list for a user-defined function: `param_names`,
/// `variadic_param_name` (`...$rest`), `ref_flags` (pass-by-reference for
/// each param) and `defaults` (source text of each default expression).
pub(crate) type ParamList = (
    Vec<String>,
    Option<String>,
    Vec<bool>,
    Vec<Option<String>>,
    Vec<Option<PromotedParam>>,
);

/// Constructor property promotion: `public readonly int $x`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PromotedParam {
    pub visibility: crate::engine::types::Visibility,
    pub readonly: bool,
}

/// Parse parameter list (the opening '(' has already been consumed)
/// Returns (param_names, variadic_param_name, ref_flags).
pub(crate) fn parse_params(
    lexer: &mut Lexer,
    context: &mut CompileContext,
) -> Result<ParamList, String> {
    let mut params = Vec::new();
    let mut ref_flags = Vec::new();
    let mut defaults: Vec<Option<String>> = Vec::new();
    let mut promoted: Vec<Option<PromotedParam>> = Vec::new();
    let mut variadic = None;
    let mut current_token = lexer.next_token()?;

    if is_punct(&current_token, ")") {
        return Ok((params, variadic, ref_flags, defaults, promoted));
    }

    loop {
        // Constructor promotion modifiers: public/protected/private [readonly]
        let mut promotion: Option<PromotedParam> = None;
        loop {
            use crate::engine::types::Visibility;
            let vis = match current_token.token_type {
                TokenType::T_PUBLIC => Some(Visibility::Public),
                TokenType::T_PROTECTED => Some(Visibility::Protected),
                TokenType::T_PRIVATE => Some(Visibility::Private),
                _ => None,
            };
            if let Some(v) = vis {
                let p = promotion.get_or_insert(PromotedParam { visibility: v, readonly: false });
                p.visibility = v;
                current_token = lexer.next_token()?;
                continue;
            }
            if current_token.token_type == TokenType::T_READONLY {
                let p = promotion.get_or_insert(PromotedParam { visibility: Visibility::Public, readonly: true });
                p.readonly = true;
                current_token = lexer.next_token()?;
                continue;
            }
            break;
        }

        // Skip nullable type hint: ?type
        if is_punct(&current_token, "?") {
            current_token = lexer.next_token()?;
        }

        // Skip type hint: int, string, float, bool, array, etc.
        // Also handles union types (int|string) and intersection types (Countable&ArrayAccess)
        if is_type_hint(&current_token) || current_token.token_type == TokenType::T_STRING {
            current_token = lexer.next_token()?;
            while is_union_sep(&current_token) || is_intersection_sep(&current_token) {
                current_token = lexer.next_token()?; // skip | or &
                if is_punct(&current_token, "?") {
                    current_token = lexer.next_token()?;
                }
                if is_type_hint(&current_token) || current_token.token_type == TokenType::T_STRING {
                    current_token = lexer.next_token()?;
                }
            }
        }

        // Check for variadic: ...$param
        let is_variadic = if current_token.token_type == TokenType::T_ELLIPSIS {
            current_token = lexer.next_token()?;
            true
        } else {
            false
        };

        // Pass-by-reference: &$param
        let is_ref = if current_token.token_type == TokenType::T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
        {
            current_token = lexer.next_token()?;
            true
        } else {
            false
        };

        if current_token.token_type != TokenType::T_VARIABLE {
            return Err(format!(
                "Expected variable parameter in function definition, got {:?}",
                current_token.token_type
            ));
        }

        let param_name = current_token.value.as_ref().unwrap().as_str();

        if is_variadic {
            variadic = Some(param_name.to_string());
            params.push(param_name.to_string());
            ref_flags.push(is_ref);
            defaults.push(None);
            promoted.push(None);
            current_token = lexer.next_token()?;
            // Variadic must be the last parameter
            if is_punct(&current_token, ",") {
                return Err("Variadic parameter must be the last parameter".to_string());
            }
            if !is_punct(&current_token, ")") {
                return Err("Expected ')' after variadic parameter".to_string());
            }
            break;
        }

        params.push(param_name.to_string());
        ref_flags.push(is_ref);
        promoted.push(promotion);

        current_token = lexer.next_token()?;

        // Default value: keep its source text; it is compiled into a prologue
        // of the function body (`$p = $p ?? (<default>)`) so that it is
        // evaluated at call time in the callee's scope.
        if current_token.token_type == TokenType::T_EQUAL {
            let start = lexer.peek_token()?.offset;
            // Compile into a scratch context: we only need the end offset.
            let mut scratch = context.child();
            let (_, next_token) = super::expression::parse_expression(lexer, &mut scratch)?;
            defaults.push(Some(lexer.source_slice(start, next_token.offset)));
            current_token = next_token;
        } else {
            defaults.push(None);
        }

        if is_punct(&current_token, ")") {
            break;
        }

        if !is_punct(&current_token, ",") {
            return Err("Expected ',' or ')' after parameter".to_string());
        }

        current_token = lexer.next_token()?;
    }

    Ok((params, variadic, ref_flags, defaults, promoted))
}

/// Skip a return type declaration after `)` if present (`: ?Foo\Bar|null`,
/// `: static`, `: (A&B)|null`). Returns the first token after the type, i.e.
/// `{`, `;` (abstract/interface methods), `=>` (arrow functions) or `use`.
pub(crate) fn skip_return_type(lexer: &mut Lexer) -> Result<Token, String> {
    let token = lexer.next_token()?;
    if !is_punct(&token, ":") {
        return Ok(token);
    }
    loop {
        let t = lexer.next_token()?;
        let is_type_part = match t.token_type {
            TokenType::T_STRING => {
                let v = t.value.as_ref().map(|s| s.as_str()).unwrap_or("");
                matches!(v, "?" | "|" | "&" | "(" | ")")
                    || v.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_' || c == '\\')
            }
            TokenType::T_STATIC
            | TokenType::T_ARRAY
            | TokenType::T_CALLABLE
            | TokenType::T_NS_SEPARATOR
            | TokenType::T_AMPERSAND_FOLLOWED_BY_VAR_OR_VARARG
            | TokenType::T_AMPERSAND_NOT_FOLLOWED_BY_VAR_OR_VARARG => true,
            _ => false,
        };
        if !is_type_part || t.token_type == TokenType::T_EOF {
            return Ok(t);
        }
    }
}

/// Parse `use ($var1, $var2, ...)` clause for closures
/// Returns list of captured variable names
fn parse_use_clause(lexer: &mut Lexer) -> Result<Vec<String>, String> {
    let mut captures = Vec::new();
    let paren = lexer.next_token()?;
    if !is_punct(&paren, "(") {
        return Err("Expected '(' after 'use'".to_string());
    }

    let mut current_token = lexer.next_token()?;
    if is_punct(&current_token, ")") {
        return Ok(captures);
    }

    loop {
        if current_token.token_type != TokenType::T_VARIABLE {
            return Err("Expected variable in use clause".to_string());
        }
        captures.push(current_token.value.as_ref().unwrap().as_str().to_string());

        current_token = lexer.next_token()?;
        if is_punct(&current_token, ")") {
            break;
        }
        if !is_punct(&current_token, ",") {
            return Err("Expected ',' or ')' in use clause".to_string());
        }
        current_token = lexer.next_token()?;
    }

    Ok(captures)
}

/// Emit `$param = $param ?? (<default>);` for every parameter with a default,
/// at the start of `body_context`. Omitted (and `null`) arguments therefore
/// take the default, evaluated in the callee's own scope.
pub(crate) fn emit_default_param_prologue(
    body_context: &mut CompileContext,
    params: &[String],
    defaults: &[Option<String>],
) -> Result<(), String> {
    for (name, default) in params.iter().zip(defaults) {
        let Some(src) = default else { continue };
        let var = name.strip_prefix('$').unwrap_or(name);
        let stmt = format!("${var} = ${var} ?? ({src});");
        let mut sub = Lexer::new(&stmt);
        let first = sub.next_token()?;
        let after = crate::engine::compile::statement::parse_statement(&mut sub, body_context, first)
            .map_err(|e| format!("in default value of ${var}: {e}"))?;
        if after.token_type != TokenType::T_EOF {
            return Err(format!("in default value of ${var}: unexpected trailing tokens"));
        }
    }
    Ok(())
}

/// Compile function body into a separate context and return the finalized OpArray
/// Handles optional return type declaration before the opening brace
fn compile_function_body(
    lexer: &mut Lexer,
    context: &mut CompileContext,
    function_name: &str,
    lineno: u32,
    params: &ParamList,
) -> Result<crate::engine::vm::OpArray, String> {
    let mut func_context = context.child();
    func_context.set_line(lineno);
    func_context.op_array.function_name = Some(function_name.to_string());
    emit_default_param_prologue(&mut func_context, &params.0, &params.3)?;

    // Skip optional return type declaration (: type) and find '{'
    let brace_token = skip_return_type(lexer)?;
    if !is_punct(&brace_token, "{") {
        return Err("Expected '{' after function parameters".to_string());
    }

    parse_statement_block(lexer, &mut func_context)?;
    let nested = std::mem::take(&mut func_context.function_table);
    context.function_table.merge(nested);
    // Generators no longer collect yielded values into an array — they emit
    // Yield opcodes that suspend execution. The implicit return at the end
    // of the function body handles generator completion.
    Ok(func_context.finalize())
}

/// Compile function definition: function name($param1, $param2) { body }
pub fn compile_function(lexer: &mut Lexer, context: &mut CompileContext) -> Result<Token, String> {
    let name_token = lexer.next_token()?;

    // If the next token is '(' — this is a closure used as a statement (rare but valid)
    if is_punct(&name_token, "(") {
        let (_val, next_token) = compile_closure_inner(lexer, context, name_token.lineno)?;
        return Ok(next_token);
    }

    if name_token.token_type != TokenType::T_STRING {
        return Err("Expected function name after 'function'".to_string());
    }

    let short_name = name_token.value.as_ref().unwrap().as_str();
    let qualified_name = match context.current_namespace.as_deref() {
        Some(ns) if !ns.is_empty() => format!("{ns}\\{short_name}"),
        _ => short_name.to_string(),
    };
    let function_name = qualified_name.as_str();

    let paren_token = lexer.next_token()?;
    if !is_punct(&paren_token, "(") {
        return Err("Expected '(' after function name".to_string());
    }

    let params = parse_params(lexer, context)?;
    let mut func_op_array =
        compile_function_body(lexer, context, function_name, name_token.lineno, &params)?;
    // Store param names so the VM can bind arguments
    func_op_array.vars = params
        .0
        .iter()
        .map(|p| crate::engine::vm::var_ref(p))
        .collect();
    func_op_array.variadic_param = params.1;
    func_op_array.ref_params = params.2;

    context
        .function_table
        .store_function(function_name, func_op_array);

    lexer.next_token()
}

/// Compile a closure expression: function($params) use ($captures) { body }
/// Called from expression parser when `function` keyword is encountered in expression position.
/// Returns (Val referencing the closure name, next Token)
pub fn compile_closure(
    lexer: &mut Lexer,
    context: &mut CompileContext,
) -> Result<(Val, Token), String> {
    // The 'function' keyword has been consumed. Next should be '('
    let paren_token = lexer.next_token()?;
    if !is_punct(&paren_token, "(") {
        return Err("Expected '(' after 'function' in closure".to_string());
    }
    compile_closure_inner(lexer, context, paren_token.lineno)
}

/// Inner closure compilation — '(' has been consumed
fn compile_closure_inner(
    lexer: &mut Lexer,
    context: &mut CompileContext,
    lineno: u32,
) -> Result<(Val, Token), String> {
    let params = parse_params(lexer, context)?;

    // After params, we may see: `: returnType`, `use (...)`, or `{`
    let mut next = skip_return_type(lexer)?;

    // Check for `use` clause
    let captures = if next.token_type == TokenType::T_USE {
        let caps = parse_use_clause(lexer)?;
        next = lexer.next_token()?;
        caps
    } else {
        Vec::new()
    };

    if !is_punct(&next, "{") {
        return Err(format!(
            "Expected '{{' in closure, got {:?}",
            next.token_type
        ));
    }

    let closure_name = format!(
        "__closure_{}",
        CLOSURE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut func_context = context.child();
    func_context.set_line(lineno);
    func_context.op_array.function_name = Some(closure_name.clone());
    func_context.op_array.vars = params
        .0
        .iter()
        .map(|p| crate::engine::vm::var_ref(p))
        .collect();
    func_context.op_array.variadic_param = params.1;
    func_context.op_array.ref_params = params.2;
    emit_default_param_prologue(&mut func_context, &params.0, &params.3)?;

    parse_statement_block(lexer, &mut func_context)?;
    let nested = std::mem::take(&mut func_context.function_table);
    let func_op_array = func_context.finalize();
    context.function_table.merge(nested);
    context
        .function_table
        .store_function(&closure_name, func_op_array);

    let val = emit_create_closure(context, &closure_name, &captures);
    let next_token = lexer.next_token()?;
    Ok((val, next_token))
}

/// Emit `CreateClosure` for `function_name` capturing `captures` by value and
/// return the temp holding the Closure object.
pub(crate) fn emit_create_closure(
    context: &mut CompileContext,
    function_name: &str,
    captures: &[String],
) -> Val {
    let mut names = crate::engine::types::PhpArray::new();
    for (i, c) in captures.iter().enumerate() {
        let _ = crate::engine::hash::hash_add_or_update(
            &mut names,
            None,
            i as u64,
            crate::engine::facade::string_val(c),
            0,
        );
    }
    let slot = context.alloc_temp();
    context.emit_opcode(
        crate::engine::vm::Opcode::CreateClosure,
        crate::engine::facade::string_val(function_name),
        Val::new(
            crate::engine::types::PhpValue::Array(Box::new(names)),
            crate::engine::types::PhpType::Array,
        ),
        crate::engine::vm::temp_var_ref(slot),
    );
    crate::engine::vm::temp_var_ref(slot)
}

/// Compile an arrow function `fn(params) => expr`. The `fn` identifier and the
/// opening `(` have been consumed. Variables used in the body that are not
/// parameters are captured by value from the enclosing scope, like PHP.
pub(crate) fn compile_arrow_fn(
    lexer: &mut Lexer,
    context: &mut CompileContext,
    lineno: u32,
) -> Result<(Val, Token), String> {
    let params = parse_params(lexer, context)?;
    let arrow = skip_return_type(lexer)?;
    if arrow.token_type != TokenType::T_DOUBLE_ARROW {
        return Err(format!(
            "Expected '=>' after arrow function parameters, got {:?}",
            arrow.token_type
        ));
    }

    let closure_name = format!(
        "__closure_{}",
        CLOSURE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let mut func_context = context.child();
    func_context.set_line(lineno);
    func_context.op_array.function_name = Some(closure_name.clone());
    func_context.op_array.variadic_param = params.1.clone();
    func_context.op_array.ref_params = params.2.clone();
    emit_default_param_prologue(&mut func_context, &params.0, &params.3)?;

    let body_start = lexer.peek_token()?.offset;
    let (body_val, next_token) = super::expression::parse_expression(lexer, &mut func_context)?;
    func_context.emit_opcode(
        crate::engine::vm::Opcode::Return,
        body_val,
        crate::engine::facade::null_val(),
        crate::engine::facade::null_val(),
    );

    // Free variables of the body become captures.
    let body_src = lexer.source_slice(body_start, next_token.offset);
    let mut captures: Vec<String> = Vec::new();
    let mut scan = Lexer::new(&body_src);
    loop {
        let t = scan.next_token()?;
        if t.token_type == TokenType::T_EOF {
            break;
        }
        if t.token_type == TokenType::T_VARIABLE
            && let Some(v) = t.value.as_ref()
        {
            let name = v.as_str().strip_prefix('$').unwrap_or(v.as_str()).to_string();
            let is_param = params
                .0
                .iter()
                .any(|p| p.strip_prefix('$').unwrap_or(p) == name);
            if name != "this" && !is_param && !captures.contains(&name) {
                captures.push(name);
            }
        }
    }

    func_context.op_array.vars = params
        .0
        .iter()
        .map(|p| crate::engine::vm::var_ref(p))
        .collect();

    let nested = std::mem::take(&mut func_context.function_table);
    let func_op_array = func_context.finalize();
    context.function_table.merge(nested);
    context
        .function_table
        .store_function(&closure_name, func_op_array);
    Ok((emit_create_closure(context, &closure_name, &captures), next_token))
}
