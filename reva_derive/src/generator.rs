use std::borrow::Cow;
use std::collections::hash_map::{Entry, HashMap};
use std::ops::Deref;
use std::path::Path;
use std::rc::Rc;
use std::{cmp, hash, mem, str};

use crate::config::WhitespaceHandling;
use crate::heritage::{Context, Heritage};
use crate::input::{Source, TemplateInput};
use crate::{CompileError, CRATE};

use parser::node::{
    Call, Comment, CondTest, FilterBlock, If, Include, Let, Lit, Loop, Match, Target, Whitespace,
    Ws,
};
use parser::{Expr, Filter, Node};
use quote::quote;

pub(crate) struct Generator<'a> {
    // The template input state: original struct AST and attributes
    input: &'a TemplateInput<'a>,
    // All contexts, keyed by the package-relative template path
    contexts: &'a HashMap<&'a Rc<Path>, Context<'a>>,
    // The heritage contains references to blocks and their ancestry
    heritage: Option<&'a Heritage<'a>>,
    // Variables accessible directly from the current scope (not redirected to context)
    locals: MapChain<'a, Cow<'a, str>, LocalMeta>,
    // Suffix whitespace from the previous literal. Will be flushed to the
    // output buffer unless suppressed by whitespace suppression on the next
    // non-literal.
    next_ws: Option<&'a str>,
    // Whitespace suppression from the previous non-literal. Will be used to
    // determine whether to flush prefix whitespace from the next literal.
    skip_ws: WhitespaceHandling,
    // If currently in a block, this will contain the name of a potential parent block
    super_block: Option<(&'a str, usize)>,
    // Buffer for writable
    buf_writable: WritableBuffer<'a>,
    // Counter for write! hash named arguments
    named: usize,
}

impl<'a> Generator<'a> {
    pub(crate) fn new<'n>(
        input: &'n TemplateInput<'_>,
        contexts: &'n HashMap<&'n Rc<Path>, Context<'n>>,
        heritage: Option<&'n Heritage<'_>>,
        locals: MapChain<'n, Cow<'n, str>, LocalMeta>,
    ) -> Generator<'n> {
        Generator {
            input,
            contexts,
            heritage,
            locals,
            next_ws: None,
            skip_ws: WhitespaceHandling::Preserve,
            super_block: None,
            buf_writable: WritableBuffer {
                discard: input.block.is_some(),
                ..Default::default()
            },
            named: 0,
        }
    }

    // Takes a Context and generates the relevant implementations.
    pub(crate) fn build(mut self, ctx: &Context<'a>) -> Result<String, CompileError> {
        let mut buf = Buffer::new(0);

        self.impl_template(ctx, &mut buf)?;
        self.impl_display(&mut buf)?;

        #[cfg(feature = "with-actix-web")]
        self.impl_actix_web_responder(&mut buf)?;
        #[cfg(feature = "with-axum")]
        self.impl_axum_into_response(&mut buf)?;
        #[cfg(feature = "with-rocket")]
        self.impl_rocket_responder(&mut buf)?;
        #[cfg(feature = "with-warp")]
        self.impl_warp_reply(&mut buf)?;

        Ok(buf.buf)
    }

    // Implement `Template` for the given context struct.
    fn impl_template(&mut self, ctx: &Context<'a>, buf: &mut Buffer) -> Result<(), CompileError> {
        self.write_header(buf, &format!("{CRATE}::Template"), None)?;
        buf.write("fn render_into(&self, writer: &mut (impl ::std::fmt::Write + ?Sized)) -> ");
        buf.write(CRATE);
        buf.writeln("::Result<()> {")?;

        // Make sure the compiler understands that the generated code depends on the template files.
        for path in self.contexts.keys() {
            // Skip the fake path of templates defined in rust source.
            let path_is_valid = match self.input.source {
                Source::Path(_) => true,
                Source::Source(_) => **path != self.input.path,
            };
            if path_is_valid {
                let canonical_path = path.canonicalize().unwrap();
                let include_path = canonical_path.to_str().unwrap();
                buf.writeln(
                    &quote! {
                        include_bytes!(#include_path);
                    }
                    .to_string(),
                )?;
            }
        }

        let size_hint = if let Some(heritage) = self.heritage {
            self.handle(heritage.root, heritage.root.nodes, buf, AstLevel::Top)
        } else {
            self.handle(ctx, ctx.nodes, buf, AstLevel::Top)
        }?;

        self.flush_ws(Ws(None, None));
        buf.write(CRATE);
        buf.writeln("::Result::Ok(())")?;
        buf.writeln("}")?;

        buf.writeln("const EXTENSION: ::std::option::Option<&'static ::std::primitive::str> = ")?;
        buf.writeln(&format!("{:?}", self.input.extension()))?;
        buf.writeln(";")?;

        buf.writeln("const SIZE_HINT: ::std::primitive::usize = ")?;
        buf.writeln(&format!("{size_hint}"))?;
        buf.writeln(";")?;

        buf.writeln("const MIME_TYPE: &'static ::std::primitive::str = ")?;
        buf.writeln(&format!("{:?}", &self.input.mime_type))?;
        buf.writeln(";")?;

        buf.writeln("}")?;
        Ok(())
    }

    // Implement `Display` for the given context struct.
    fn impl_display(&mut self, buf: &mut Buffer) -> Result<(), CompileError> {
        self.write_header(buf, "::std::fmt::Display", None)?;
        buf.writeln("#[inline]")?;
        buf.writeln("fn fmt(&self, f: &mut ::std::fmt::Formatter) -> ::std::fmt::Result {")?;
        buf.write(CRATE);
        buf.writeln("::Template::render_into(self, f).map_err(|_| ::std::fmt::Error {})")?;
        buf.writeln("}")?;
        buf.writeln("}")
    }

    // Implement Actix-web's `Responder`.
    #[cfg(feature = "with-actix-web")]
    fn impl_actix_web_responder(&mut self, buf: &mut Buffer) -> Result<(), CompileError> {
        self.write_header(buf, "::reva_actix::actix_web::Responder", None)?;
        buf.writeln("type Body = ::reva_actix::actix_web::body::BoxBody;")?;
        buf.writeln("#[inline]")?;
        buf.writeln(
            "fn respond_to(self, _req: &::reva_actix::actix_web::HttpRequest) \
             -> ::reva_actix::actix_web::HttpResponse<Self::Body> {",
        )?;
        buf.writeln("::reva_actix::into_response(&self)")?;
        buf.writeln("}")?;
        buf.writeln("}")
    }

    // Implement Axum's `IntoResponse`.
    #[cfg(feature = "with-axum")]
    fn impl_axum_into_response(&mut self, buf: &mut Buffer) -> Result<(), CompileError> {
        self.write_header(buf, "::reva_axum::axum_core::response::IntoResponse", None)?;
        buf.writeln("#[inline]")?;
        buf.writeln(
            "fn into_response(self)\
             -> ::reva_axum::axum_core::response::Response {",
        )?;
        buf.writeln("::reva_axum::into_response(&self)")?;
        buf.writeln("}")?;
        buf.writeln("}")
    }

    // Implement Rocket's `Responder`.
    #[cfg(feature = "with-rocket")]
    fn impl_rocket_responder(&mut self, buf: &mut Buffer) -> Result<(), CompileError> {
        let lifetime1 = syn::Lifetime::new("'reva1", proc_macro2::Span::call_site());
        let param1 = syn::GenericParam::Lifetime(syn::LifetimeParam::new(lifetime1));

        self.write_header(
            buf,
            "::reva_rocket::rocket::response::Responder<'reva1, 'static>",
            Some(vec![param1]),
        )?;
        buf.writeln("#[inline]")?;
        buf.writeln(
            "fn respond_to(self, _: &'reva1 ::reva_rocket::rocket::request::Request<'_>) \
             -> ::reva_rocket::rocket::response::Result<'static> {",
        )?;
        buf.writeln("::reva_rocket::respond(&self)")?;
        buf.writeln("}")?;
        buf.writeln("}")?;
        Ok(())
    }

    #[cfg(feature = "with-warp")]
    fn impl_warp_reply(&mut self, buf: &mut Buffer) -> Result<(), CompileError> {
        self.write_header(buf, "::reva_warp::warp::reply::Reply", None)?;
        buf.writeln("#[inline]")?;
        buf.writeln("fn into_response(self) -> ::reva_warp::warp::reply::Response {")?;
        buf.writeln("::reva_warp::into_response(&self)")?;
        buf.writeln("}")?;
        buf.writeln("}")
    }

    // Writes header for the `impl` for `TraitFromPathName` or `Template`
    // for the given context struct.
    fn write_header(
        &mut self,
        buf: &mut Buffer,
        target: &str,
        params: Option<Vec<syn::GenericParam>>,
    ) -> Result<(), CompileError> {
        let mut generics;
        let (impl_generics, orig_ty_generics, where_clause) = if let Some(params) = params {
            generics = self.input.ast.generics.clone();
            for param in params {
                generics.params.push(param);
            }

            let (_, orig_ty_generics, _) = self.input.ast.generics.split_for_impl();
            let (impl_generics, _, where_clause) = generics.split_for_impl();
            (impl_generics, orig_ty_generics, where_clause)
        } else {
            self.input.ast.generics.split_for_impl()
        };

        buf.writeln(&format!(
            "{} {} for {}{} {{",
            quote!(impl #impl_generics),
            target,
            self.input.ast.ident,
            quote!(#orig_ty_generics #where_clause),
        ))
    }

    /* Helper methods for handling node types */

    fn handle(
        &mut self,
        ctx: &Context<'a>,
        nodes: &'a [Node<'_>],
        buf: &mut Buffer,
        level: AstLevel,
    ) -> Result<usize, CompileError> {
        let mut size_hint = 0;
        for n in nodes {
            match *n {
                Node::Lit(ref lit) => {
                    self.visit_lit(lit);
                }
                Node::Comment(ref comment) => {
                    self.write_comment(comment);
                }
                Node::Expr(ws, ref val) => {
                    self.write_expr(ws, val);
                }
                Node::Let(ref l) => {
                    self.write_let(buf, l)?;
                }
                Node::If(ref i) => {
                    size_hint += self.write_if(ctx, buf, i)?;
                }
                Node::Match(ref m) => {
                    size_hint += self.write_match(ctx, buf, m)?;
                }
                Node::Loop(ref loop_block) => {
                    size_hint += self.write_loop(ctx, buf, loop_block)?;
                }
                Node::BlockDef(ref b) => {
                    size_hint += self.write_block(ctx, buf, Some(b.name), Ws(b.ws1.0, b.ws2.1))?;
                }
                Node::Include(ref i) => {
                    size_hint += self.handle_include(ctx, buf, i)?;
                }
                Node::Call(ref call) => {
                    size_hint += self.write_call(ctx, buf, call)?;
                }
                Node::FilterBlock(ref filter) => {
                    size_hint += self.write_filter_block(ctx, buf, filter)?;
                }
                Node::Macro(ref m) => {
                    if level != AstLevel::Top {
                        return Err("macro blocks only allowed at the top level".into());
                    }
                    self.flush_ws(m.ws1);
                    self.prepare_ws(m.ws2);
                }
                Node::Raw(ref raw) => {
                    self.handle_ws(raw.ws1);
                    self.visit_lit(&raw.lit);
                    self.handle_ws(raw.ws2);
                }
                Node::Import(ref i) => {
                    if level != AstLevel::Top {
                        return Err("import blocks only allowed at the top level".into());
                    }
                    self.handle_ws(i.ws);
                }
                Node::Extends(_) => {
                    if level != AstLevel::Top {
                        return Err("extend blocks only allowed at the top level".into());
                    }
                    // No whitespace handling: child template top-level is not used,
                    // except for the blocks defined in it.
                }
                Node::Break(ws) => {
                    self.handle_ws(ws);
                    self.write_buf_writable(buf)?;
                    buf.writeln("break;")?;
                }
                Node::Continue(ws) => {
                    self.handle_ws(ws);
                    self.write_buf_writable(buf)?;
                    buf.writeln("continue;")?;
                }
            }
        }

        if AstLevel::Top == level {
            // Handle any pending whitespace.
            if self.next_ws.is_some() {
                self.flush_ws(Ws(Some(self.skip_ws.into()), None));
            }

            size_hint += self.write_buf_writable(buf)?;
        }
        Ok(size_hint)
    }

    fn write_if(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        i: &'a If<'_>,
    ) -> Result<usize, CompileError> {
        let mut flushed = 0;
        let mut arm_sizes = Vec::new();
        let mut has_else = false;
        for (i, cond) in i.branches.iter().enumerate() {
            self.handle_ws(cond.ws);
            flushed += self.write_buf_writable(buf)?;
            if i > 0 {
                self.locals.pop();
            }

            self.locals.push();
            let mut arm_size = 0;
            if let Some(CondTest { target, expr }) = &cond.cond {
                if i == 0 {
                    buf.write("if ");
                } else {
                    buf.dedent()?;
                    buf.write("} else if ");
                }

                if let Some(target) = target {
                    let mut expr_buf = Buffer::new(0);
                    buf.write("let ");
                    // If this is a chain condition, then we need to declare the variable after the
                    // left expression has been handled but before the right expression is handled
                    // but this one should have access to the let-bound variable.
                    match expr {
                        Expr::BinOp(op, ref left, ref right) if *op == "||" || *op == "&&" => {
                            self.visit_expr(&mut expr_buf, left)?;
                            self.visit_target(buf, true, true, target);
                            expr_buf.write(&format!(" {op} "));
                            self.visit_expr(&mut expr_buf, right)?;
                        }
                        _ => {
                            self.visit_expr(&mut expr_buf, expr)?;
                            self.visit_target(buf, true, true, target);
                        }
                    }
                    buf.write(" = &");
                    buf.write(&expr_buf.buf);
                } else {
                    // The following syntax `*(&(...) as &bool)` is used to
                    // trigger Rust's automatic dereferencing, to coerce
                    // e.g. `&&&&&bool` to `bool`. First `&(...) as &bool`
                    // coerces e.g. `&&&bool` to `&bool`. Then `*(&bool)`
                    // finally dereferences it to `bool`.
                    buf.write("*(&(");
                    let expr_code = self.visit_expr_root(expr)?;
                    buf.write(&expr_code);
                    buf.write(") as &bool)");
                }
            } else {
                buf.dedent()?;
                buf.write("} else");
                has_else = true;
            }

            buf.writeln(" {")?;

            arm_size += self.handle(ctx, &cond.nodes, buf, AstLevel::Nested)?;
            arm_sizes.push(arm_size);
        }
        self.handle_ws(i.ws);
        flushed += self.write_buf_writable(buf)?;
        buf.writeln("}")?;

        self.locals.pop();

        if !has_else {
            arm_sizes.push(0);
        }
        Ok(flushed + median(&mut arm_sizes))
    }

    #[allow(clippy::too_many_arguments)]
    fn write_match(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        m: &'a Match<'a>,
    ) -> Result<usize, CompileError> {
        let Match {
            ws1,
            ref expr,
            ref arms,
            ws2,
        } = *m;

        self.flush_ws(ws1);
        let flushed = self.write_buf_writable(buf)?;
        let mut arm_sizes = Vec::new();

        let expr_code = self.visit_expr_root(expr)?;
        buf.writeln(&format!("match &{expr_code} {{"))?;

        let mut arm_size = 0;
        for (i, arm) in arms.iter().enumerate() {
            self.handle_ws(arm.ws);

            if i > 0 {
                arm_sizes.push(arm_size + self.write_buf_writable(buf)?);

                buf.writeln("}")?;
                self.locals.pop();
            }

            self.locals.push();
            self.visit_target(buf, true, true, &arm.target);
            buf.writeln(" => {")?;

            arm_size = self.handle(ctx, &arm.nodes, buf, AstLevel::Nested)?;
        }

        self.handle_ws(ws2);
        arm_sizes.push(arm_size + self.write_buf_writable(buf)?);
        buf.writeln("}")?;
        self.locals.pop();

        buf.writeln("}")?;

        Ok(flushed + median(&mut arm_sizes))
    }

    #[allow(clippy::too_many_arguments)]
    fn write_loop(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        loop_block: &'a Loop<'_>,
    ) -> Result<usize, CompileError> {
        self.handle_ws(loop_block.ws1);
        self.locals.push();

        let expr_code = self.visit_expr_root(&loop_block.iter)?;

        let has_else_nodes = !loop_block.else_nodes.is_empty();

        let flushed = self.write_buf_writable(buf)?;
        buf.writeln("{")?;
        if has_else_nodes {
            buf.writeln("let mut _did_loop = false;")?;
        }
        match loop_block.iter {
            Expr::Range(_, _, _) => buf.writeln(&format!("let _iter = {expr_code};")),
            Expr::Array(..) => buf.writeln(&format!("let _iter = {expr_code}.iter();")),
            // If `iter` is a call then we assume it's something that returns
            // an iterator. If not then the user can explicitly add the needed
            // call without issues.
            Expr::Call(..) | Expr::Index(..) => {
                buf.writeln(&format!("let _iter = ({expr_code}).into_iter();"))
            }
            // If accessing `self` then it most likely needs to be
            // borrowed, to prevent an attempt of moving.
            _ if expr_code.starts_with("self.") => {
                buf.writeln(&format!("let _iter = (&{expr_code}).into_iter();"))
            }
            // If accessing a field then it most likely needs to be
            // borrowed, to prevent an attempt of moving.
            Expr::Attr(..) => buf.writeln(&format!("let _iter = (&{expr_code}).into_iter();")),
            // Otherwise, we borrow `iter` assuming that it implements `IntoIterator`.
            _ => buf.writeln(&format!("let _iter = ({expr_code}).into_iter();")),
        }?;
        if let Some(cond) = &loop_block.cond {
            self.locals.push();
            buf.write("let _iter = _iter.filter(|");
            self.visit_target(buf, true, true, &loop_block.var);
            buf.write("| -> bool {");
            self.visit_expr(buf, cond)?;
            buf.writeln("});")?;
            self.locals.pop();
        }

        self.locals.push();
        buf.write("for (");
        self.visit_target(buf, true, true, &loop_block.var);
        buf.write(", _loop_item) in ");
        buf.write(CRATE);
        buf.writeln("::helpers::TemplateLoop::new(_iter) {")?;

        if has_else_nodes {
            buf.writeln("_did_loop = true;")?;
        }
        let mut size_hint1 = self.handle(ctx, &loop_block.body, buf, AstLevel::Nested)?;
        self.handle_ws(loop_block.ws2);
        size_hint1 += self.write_buf_writable(buf)?;
        self.locals.pop();
        buf.writeln("}")?;

        let mut size_hint2;
        if has_else_nodes {
            buf.writeln("if !_did_loop {")?;
            self.locals.push();
            size_hint2 = self.handle(ctx, &loop_block.else_nodes, buf, AstLevel::Nested)?;
            self.handle_ws(loop_block.ws3);
            size_hint2 += self.write_buf_writable(buf)?;
            self.locals.pop();
            buf.writeln("}")?;
        } else {
            self.handle_ws(loop_block.ws3);
            size_hint2 = self.write_buf_writable(buf)?;
        }

        buf.writeln("}")?;
        self.locals.pop();

        Ok(flushed + ((size_hint1 * 3) + size_hint2) / 2)
    }

    fn write_call(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        call: &'a Call<'_>,
    ) -> Result<usize, CompileError> {
        let Call {
            ws,
            scope,
            name,
            ref args,
        } = *call;
        if name == "super" {
            return self.write_block(ctx, buf, None, ws);
        }

        let (def, own_ctx) = match scope {
            Some(s) => {
                let path = ctx.imports.get(s).ok_or_else(|| {
                    CompileError::from(format!("no import found for scope {s:?}"))
                })?;
                let mctx = self
                    .contexts
                    .get(path)
                    .ok_or_else(|| CompileError::from(format!("context for {path:?} not found")))?;
                let def = mctx.macros.get(name).ok_or_else(|| {
                    CompileError::from(format!("macro {name:?} not found in scope {s:?}"))
                })?;
                (def, mctx)
            }
            None => {
                let def = ctx
                    .macros
                    .get(name)
                    .ok_or_else(|| CompileError::from(format!("macro {name:?} not found")))?;
                (def, ctx)
            }
        };

        self.flush_ws(ws); // Cannot handle_ws() here: whitespace from macro definition comes first
        self.locals.push();
        self.write_buf_writable(buf)?;
        buf.writeln("{")?;
        self.prepare_ws(def.ws1);

        let mut names = Buffer::new(0);
        let mut values = Buffer::new(0);
        let mut is_first_variable = true;
        if args.len() != def.args.len() {
            return Err(CompileError::from(format!(
                "macro {name:?} expected {} argument{}, found {}",
                def.args.len(),
                if def.args.len() != 1 { "s" } else { "" },
                args.len()
            )));
        }
        let mut named_arguments = HashMap::new();
        // Since named arguments can only be passed last, we only need to check if the last argument
        // is a named one.
        if let Some(Expr::NamedArgument(_, _)) = args.last() {
            // First we check that all named arguments actually exist in the called item.
            for arg in args.iter().rev() {
                let Expr::NamedArgument(arg_name, _) = arg else {
                    break;
                };
                if !def.args.iter().any(|arg| arg == arg_name) {
                    return Err(CompileError::from(format!(
                        "no argument named `{arg_name}` in macro {name:?}"
                    )));
                }
                named_arguments.insert(Cow::Borrowed(arg_name), arg);
            }
        }

        // Handling both named and unnamed arguments requires to be careful of the named arguments
        // order. To do so, we iterate through the macro defined arguments and then check if we have
        // a named argument with this name:
        //
        // * If there is one, we add it and move to the next argument.
        // * If there isn't one, then we pick the next argument (we can do it without checking
        //   anything since named arguments are always last).
        let mut allow_positional = true;
        for (index, arg) in def.args.iter().enumerate() {
            let expr = match named_arguments.get(&Cow::Borrowed(arg)) {
                Some(expr) => {
                    allow_positional = false;
                    expr
                }
                None => {
                    if !allow_positional {
                        // If there is already at least one named argument, then it's not allowed
                        // to use unnamed ones at this point anymore.
                        return Err(CompileError::from(format!(
                            "cannot have unnamed argument (`{arg}`) after named argument in macro \
                             {name:?}"
                        )));
                    }
                    &args[index]
                }
            };
            match expr {
                // If `expr` is already a form of variable then
                // don't reintroduce a new variable. This is
                // to avoid moving non-copyable values.
                &Expr::Var(name) if name != "self" => {
                    let var = self.locals.resolve_or_self(name);
                    self.locals
                        .insert(Cow::Borrowed(arg), LocalMeta::with_ref(var));
                }
                Expr::Attr(obj, attr) => {
                    let mut attr_buf = Buffer::new(0);
                    self.visit_attr(&mut attr_buf, obj, attr)?;

                    let var = self.locals.resolve(&attr_buf.buf).unwrap_or(attr_buf.buf);
                    self.locals
                        .insert(Cow::Borrowed(arg), LocalMeta::with_ref(var));
                }
                // Everything else still needs to become variables,
                // to avoid having the same logic be executed
                // multiple times, e.g. in the case of macro
                // parameters being used multiple times.
                _ => {
                    if is_first_variable {
                        is_first_variable = false
                    } else {
                        names.write(", ");
                        values.write(", ");
                    }
                    names.write(arg);

                    values.write("(");
                    values.write(&self.visit_expr_root(expr)?);
                    values.write(")");
                    self.locals.insert_with_default(Cow::Borrowed(arg));
                }
            }
        }

        debug_assert_eq!(names.buf.is_empty(), values.buf.is_empty());
        if !names.buf.is_empty() {
            buf.writeln(&format!("let ({}) = ({});", names.buf, values.buf))?;
        }

        let mut size_hint = self.handle(own_ctx, &def.nodes, buf, AstLevel::Nested)?;

        self.flush_ws(def.ws2);
        size_hint += self.write_buf_writable(buf)?;
        buf.writeln("}")?;
        self.locals.pop();
        self.prepare_ws(ws);
        Ok(size_hint)
    }

    fn write_filter_block(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        filter: &'a FilterBlock<'_>,
    ) -> Result<usize, CompileError> {
        self.flush_ws(filter.ws1);
        let mut var_name = String::new();
        for id in 0.. {
            var_name = format!("__filter_block{id}");
            if self.locals.get(&Cow::Borrowed(var_name.as_str())).is_none() {
                // No variable with this name exists, we're in the clear!
                break;
            }
        }
        let current_buf = mem::take(&mut self.buf_writable.buf);

        self.prepare_ws(filter.ws1);
        let mut size_hint = self.handle(ctx, &filter.nodes, buf, AstLevel::Nested)?;
        self.flush_ws(filter.ws2);

        let WriteParts {
            size_hint: write_size_hint,
            buffers,
        } = self.prepare_format(buf.indent + 1)?;
        size_hint += match buffers {
            None => return Ok(0),
            Some(WritePartsBuffers { format, expr: None }) => {
                buf.writeln(&format!("let {var_name} = {:#?};", &format.buf))?;
                write_size_hint
            }
            Some(WritePartsBuffers {
                format,
                expr: Some(expr),
            }) => {
                buf.writeln(&format!(
                    "let {var_name} = format!({:#?}, {});",
                    &format.buf,
                    expr.buf.trim(),
                ))?;
                write_size_hint
            }
        };

        self.buf_writable.buf = current_buf;

        let mut filter_buf = Buffer::new(buf.indent);
        let Filter {
            name: filter_name,
            arguments,
        } = &filter.filters;
        let mut arguments = arguments.clone();

        insert_first_filter_argument(&mut arguments, var_name.clone());

        let wrap = self.visit_filter(&mut filter_buf, filter_name, &arguments)?;

        self.buf_writable
            .push(Writable::Generated(filter_buf.buf, wrap));
        self.prepare_ws(filter.ws2);

        // We don't forget to add the created variable into the list of variables in the scope.
        self.locals
            .insert(Cow::Owned(var_name), LocalMeta::initialized());

        Ok(size_hint)
    }

    fn handle_include(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        i: &'a Include<'_>,
    ) -> Result<usize, CompileError> {
        self.flush_ws(i.ws);
        self.write_buf_writable(buf)?;
        let path = self
            .input
            .config
            .find_template(i.path, Some(&self.input.path))?;

        // Make sure the compiler understands that the generated code depends on the template file.
        {
            let canonical_path = path.canonicalize().unwrap();
            let include_path = canonical_path.to_str().unwrap();
            buf.writeln(
                &quote! {
                    include_bytes!(#include_path);
                }
                .to_string(),
            )?;
        }

        // We clone the context of the child in order to preserve their macros and imports.
        // But also add all the imports and macros from this template that don't override the
        // child's ones to preserve this template's context.
        let child_ctx = &mut self.contexts[&path].clone();
        for (name, mac) in &ctx.macros {
            child_ctx.macros.entry(name).or_insert(mac);
        }
        for (name, import) in &ctx.imports {
            child_ctx
                .imports
                .entry(name)
                .or_insert_with(|| import.clone());
        }

        // Create a new generator for the child, and call it like in `impl_template` as if it were
        // a full template, while preserving the context.
        let heritage = if !child_ctx.blocks.is_empty() || child_ctx.extends.is_some() {
            Some(Heritage::new(child_ctx, self.contexts))
        } else {
            None
        };

        let handle_ctx = match &heritage {
            Some(heritage) => heritage.root,
            None => child_ctx,
        };
        let locals = MapChain::with_parent(&self.locals);
        let mut child = Self::new(self.input, self.contexts, heritage.as_ref(), locals);
        child.buf_writable.discard = self.buf_writable.discard;
        let mut size_hint = child.handle(handle_ctx, handle_ctx.nodes, buf, AstLevel::Top)?;
        size_hint += child.write_buf_writable(buf)?;
        self.prepare_ws(i.ws);

        Ok(size_hint)
    }

    fn is_shadowing_variable(&self, var: &Target<'a>) -> Result<bool, CompileError> {
        match var {
            Target::Name(name) => {
                let name = normalize_identifier(name);
                match self.locals.get(&Cow::Borrowed(name)) {
                    // declares a new variable
                    None => Ok(false),
                    // an initialized variable gets shadowed
                    Some(meta) if meta.initialized => Ok(true),
                    // initializes a variable that was introduced in a LetDecl before
                    _ => Ok(false),
                }
            }
            Target::Tuple(_, targets) => {
                for target in targets {
                    match self.is_shadowing_variable(target) {
                        Ok(false) => continue,
                        outcome => return outcome,
                    }
                }
                Ok(false)
            }
            Target::Struct(_, named_targets) => {
                for (_, target) in named_targets {
                    match self.is_shadowing_variable(target) {
                        Ok(false) => continue,
                        outcome => return outcome,
                    }
                }
                Ok(false)
            }
            _ => Err("literals are not allowed on the left-hand side of an assignment".into()),
        }
    }

    fn write_let(&mut self, buf: &mut Buffer, l: &'a Let<'_>) -> Result<(), CompileError> {
        self.handle_ws(l.ws);

        let Some(val) = &l.val else {
            self.write_buf_writable(buf)?;
            buf.write("let ");
            self.visit_target(buf, false, true, &l.var);
            return buf.writeln(";");
        };

        let mut expr_buf = Buffer::new(0);
        self.visit_expr(&mut expr_buf, val)?;

        let shadowed = self.is_shadowing_variable(&l.var)?;
        if shadowed {
            // Need to flush the buffer if the variable is being shadowed,
            // to ensure the old variable is used.
            self.write_buf_writable(buf)?;
        }
        if shadowed
            || !matches!(l.var, Target::Name(_))
            || matches!(&l.var, Target::Name(name) if self.locals.get(&Cow::Borrowed(name)).is_none())
        {
            buf.write("let ");
        }

        self.visit_target(buf, true, true, &l.var);
        buf.writeln(&format!(" = {};", &expr_buf.buf))
    }

    // If `name` is `Some`, this is a call to a block definition, and we have to find
    // the first block for that name from the ancestry chain. If name is `None`, this
    // is from a `super()` call, and we can get the name from `self.super_block`.
    fn write_block(
        &mut self,
        ctx: &Context<'a>,
        buf: &mut Buffer,
        name: Option<&'a str>,
        outer: Ws,
    ) -> Result<usize, CompileError> {
        // Flush preceding whitespace according to the outer WS spec
        self.flush_ws(outer);

        let cur = match (name, self.super_block) {
            // The top-level context contains a block definition
            (Some(cur_name), None) => (cur_name, 0),
            // A block definition contains a block definition of the same name
            (Some(cur_name), Some((prev_name, _))) if cur_name == prev_name => {
                return Err(format!("cannot define recursive blocks ({cur_name})").into());
            }
            // A block definition contains a definition of another block
            (Some(cur_name), Some((_, _))) => (cur_name, 0),
            // `super()` was called inside a block
            (None, Some((prev_name, gen))) => (prev_name, gen + 1),
            // `super()` is called from outside a block
            (None, None) => return Err("cannot call 'super()' outside block".into()),
        };

        self.write_buf_writable(buf)?;

        let block_fragment_write = self.input.block == name && self.buf_writable.discard;
        // Allow writing to the buffer if we're in the block fragment
        if block_fragment_write {
            self.buf_writable.discard = false;
        }
        let prev_buf_discard = mem::replace(&mut buf.discard, self.buf_writable.discard);

        // Get the block definition from the heritage chain
        let heritage = self
            .heritage
            .ok_or_else(|| CompileError::from("no block ancestors available"))?;
        let (child_ctx, def) = *heritage.blocks[cur.0].get(cur.1).ok_or_else(|| {
            CompileError::from(match name {
                None => format!("no super() block found for block '{}'", cur.0),
                Some(name) => format!("no block found for name '{name}'"),
            })
        })?;

        // We clone the context of the child in order to preserve their macros and imports.
        // But also add all the imports and macros from this template that don't override the
        // child's ones to preserve this template's context.
        let mut child_ctx = child_ctx.clone();
        for (name, mac) in &ctx.macros {
            child_ctx.macros.entry(name).or_insert(mac);
        }
        for (name, import) in &ctx.imports {
            child_ctx
                .imports
                .entry(name)
                .or_insert_with(|| import.clone());
        }

        let mut child = Self::new(
            self.input,
            self.contexts,
            Some(heritage),
            // Variables are NOT inherited from the parent scope.
            MapChain::default(),
        );
        child.buf_writable = mem::take(&mut self.buf_writable);

        // Handle inner whitespace suppression spec and process block nodes
        child.prepare_ws(def.ws1);

        child.super_block = Some(cur);
        let size_hint = child.handle(&child_ctx, &def.nodes, buf, AstLevel::Block)?;

        if !child.locals.is_current_empty() {
            // Need to flush the buffer before popping the variable stack
            child.write_buf_writable(buf)?;
        }

        child.flush_ws(def.ws2);
        self.buf_writable = child.buf_writable;

        // Restore original block context and set whitespace suppression for
        // succeeding whitespace according to the outer WS spec
        self.prepare_ws(outer);

        // Restore the original buffer discarding state
        if block_fragment_write {
            self.buf_writable.discard = true;
        }
        buf.discard = prev_buf_discard;

        Ok(size_hint)
    }

    fn write_expr(&mut self, ws: Ws, s: &'a Expr<'a>) {
        self.handle_ws(ws);
        self.buf_writable.push(Writable::Expr(s));
    }

    // Write expression buffer and empty
    fn write_buf_writable(&mut self, buf: &mut Buffer) -> Result<usize, CompileError> {
        let WriteParts { size_hint, buffers } = self.prepare_format(buf.indent)?;
        match buffers {
            None => Ok(size_hint),
            Some(WritePartsBuffers { format, expr: None }) => {
                buf.writeln(&format!("writer.write_str({:#?})?;", &format.buf))?;
                Ok(size_hint)
            }
            Some(WritePartsBuffers {
                format,
                expr: Some(expr),
            }) => {
                buf.writeln("::std::write!(")?;
                buf.indent();
                buf.writeln("writer,")?;
                buf.writeln(&format!("{:#?},", &format.buf))?;
                buf.writeln(expr.buf.trim())?;
                buf.dedent()?;
                buf.writeln(")?;")?;
                Ok(size_hint)
            }
        }
    }

    /// This is the common code to generate an expression. It is used for filter blocks and for
    /// expressions more generally. It stores the size it represents and the buffers. Take a look
    /// at `WriteParts` for more details.
    fn prepare_format(&mut self, indent: u8) -> Result<WriteParts, CompileError> {
        if self.buf_writable.is_empty() {
            return Ok(WriteParts {
                size_hint: 0,
                buffers: None,
            });
        }

        if self
            .buf_writable
            .iter()
            .all(|w| matches!(w, Writable::Lit(_)))
        {
            let mut buf_lit = Buffer::new(0);
            for s in mem::take(&mut self.buf_writable.buf) {
                if let Writable::Lit(s) = s {
                    buf_lit.write(s);
                };
            }
            return Ok(WriteParts {
                size_hint: buf_lit.buf.len(),
                buffers: Some(WritePartsBuffers {
                    format: buf_lit,
                    expr: None,
                }),
            });
        }

        let mut expr_cache = HashMap::with_capacity(self.buf_writable.len());

        let mut size_hint = 0;
        let mut buf_format = Buffer::new(0);
        let mut buf_expr = Buffer::new(indent + 1);

        for s in mem::take(&mut self.buf_writable.buf) {
            match s {
                Writable::Lit(s) => {
                    buf_format.write(&s.replace('{', "{{").replace('}', "}}"));
                    size_hint += s.len();
                }
                Writable::Expr(s) => {
                    let mut expr_buf = Buffer::new(0);
                    let wrapped = self.visit_expr(&mut expr_buf, s)?;
                    let cacheable = is_cacheable(s);
                    size_hint += self.named_expression(
                        &mut buf_expr,
                        &mut buf_format,
                        expr_buf.buf,
                        wrapped,
                        cacheable,
                        &mut expr_cache,
                    )?;
                }
                Writable::Generated(s, wrapped) => {
                    size_hint += self.named_expression(
                        &mut buf_expr,
                        &mut buf_format,
                        s,
                        wrapped,
                        false,
                        &mut expr_cache,
                    )?;
                }
            }
        }
        Ok(WriteParts {
            size_hint,
            buffers: Some(WritePartsBuffers {
                format: buf_format,
                expr: Some(buf_expr),
            }),
        })
    }

    fn named_expression(
        &mut self,
        buf_expr: &mut Buffer,
        buf_format: &mut Buffer,
        expr: String,
        wrapped: DisplayWrap,
        cacheable: bool,
        expr_cache: &mut HashMap<String, usize>,
    ) -> Result<usize, CompileError> {
        let expression = match wrapped {
            DisplayWrap::Wrapped => expr,
            DisplayWrap::Unwrapped => format!(
                "{CRATE}::MarkupDisplay::new_unsafe(&({}), {})",
                expr, self.input.escaper
            ),
        };
        let id = match expr_cache.entry(expression) {
            Entry::Occupied(e) if cacheable => *e.get(),
            entry => {
                let id = self.named;
                self.named += 1;

                buf_expr.write(&format!("expr{id} = "));
                buf_expr.write("&");
                buf_expr.write(entry.key());
                buf_expr.writeln(",")?;

                if let Entry::Vacant(e) = entry {
                    e.insert(id);
                }

                id
            }
        };

        buf_format.write(&format!("{{expr{id}}}"));
        Ok(3)
    }

    fn visit_lit(&mut self, lit: &'a Lit<'_>) {
        assert!(self.next_ws.is_none());
        let Lit { lws, val, rws } = *lit;
        if !lws.is_empty() {
            match self.skip_ws {
                WhitespaceHandling::Suppress => {}
                _ if val.is_empty() => {
                    assert!(rws.is_empty());
                    self.next_ws = Some(lws);
                }
                WhitespaceHandling::Preserve => self.buf_writable.push(Writable::Lit(lws)),
                WhitespaceHandling::Minimize => {
                    self.buf_writable
                        .push(Writable::Lit(match lws.contains('\n') {
                            true => "\n",
                            false => " ",
                        }));
                }
            }
        }

        if !val.is_empty() {
            self.skip_ws = WhitespaceHandling::Preserve;
            self.buf_writable.push(Writable::Lit(val));
        }

        if !rws.is_empty() {
            self.next_ws = Some(rws);
        }
    }

    fn write_comment(&mut self, comment: &'a Comment<'_>) {
        self.handle_ws(comment.ws);
    }

    /* Visitor methods for expression types */

    fn visit_expr_root(&mut self, expr: &Expr<'_>) -> Result<String, CompileError> {
        let mut buf = Buffer::new(0);
        self.visit_expr(&mut buf, expr)?;
        Ok(buf.buf)
    }

    fn visit_expr(
        &mut self,
        buf: &mut Buffer,
        expr: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        Ok(match *expr {
            Expr::BoolLit(s) => self.visit_bool_lit(buf, s),
            Expr::NumLit(s) => self.visit_num_lit(buf, s),
            Expr::StrLit(s) => self.visit_str_lit(buf, s),
            Expr::CharLit(s) => self.visit_char_lit(buf, s),
            Expr::Var(s) => self.visit_var(buf, s),
            Expr::Path(ref path) => self.visit_path(buf, path),
            Expr::Array(ref elements) => self.visit_array(buf, elements)?,
            Expr::Attr(ref obj, name) => self.visit_attr(buf, obj, name)?,
            Expr::Index(ref obj, ref key) => self.visit_index(buf, obj, key)?,
            Expr::Filter(Filter {
                name,
                ref arguments,
            }) => self.visit_filter(buf, name, arguments)?,
            Expr::Unary(op, ref inner) => self.visit_unary(buf, op, inner)?,
            Expr::BinOp(op, ref left, ref right) => self.visit_binop(buf, op, left, right)?,
            Expr::Range(op, ref left, ref right) => {
                self.visit_range(buf, op, left.as_deref(), right.as_deref())?
            }
            Expr::Group(ref inner) => self.visit_group(buf, inner)?,
            Expr::Call(ref obj, ref args) => self.visit_call(buf, obj, args)?,
            Expr::RustMacro(ref path, args) => self.visit_rust_macro(buf, path, args),
            Expr::Try(ref expr) => self.visit_try(buf, expr)?,
            Expr::Tuple(ref exprs) => self.visit_tuple(buf, exprs)?,
            Expr::NamedArgument(_, ref expr) => self.visit_named_argument(buf, expr)?,
            Expr::Generated(ref s) => self.visit_generated(buf, s),
        })
    }

    fn visit_try(
        &mut self,
        buf: &mut Buffer,
        expr: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        buf.write("::core::result::Result::map_err(");
        self.visit_expr(buf, expr)?;
        buf.write(", |err| ");
        buf.write(CRATE);
        buf.write("::shared::Error::Custom(::core::convert::Into::into(err)))?");
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_rust_macro(&mut self, buf: &mut Buffer, path: &[&str], args: &str) -> DisplayWrap {
        self.visit_path(buf, path);
        buf.write("!(");
        buf.write(args);
        buf.write(")");

        DisplayWrap::Unwrapped
    }

    fn visit_filter(
        &mut self,
        buf: &mut Buffer,
        name: &str,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        match name {
            "as_ref" => return self._visit_as_ref_filter(buf, args),
            "deref" => return self._visit_deref_filter(buf, args),
            "escape" | "e" => return self._visit_escape_filter(buf, args),
            "fmt" => return self._visit_fmt_filter(buf, args),
            "format" => return self._visit_format_filter(buf, args),
            "join" => return self._visit_join_filter(buf, args),
            "safe" => return self._visit_safe_filter(buf, args),
            _ => {}
        }

        if crate::BUILT_IN_FILTERS.contains(&name) {
            buf.write(&format!("{CRATE}::filters::{name}("));
        } else {
            buf.write(&format!("filters::{name}("));
        }
        self._visit_args(buf, args)?;
        buf.write(")?");
        Ok(DisplayWrap::Unwrapped)
    }

    fn _visit_as_ref_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        let arg = match args {
            [arg] => arg,
            _ => return Err("unexpected argument(s) in `as_ref` filter".into()),
        };
        buf.write("&");
        self.visit_expr(buf, arg)?;
        Ok(DisplayWrap::Unwrapped)
    }

    fn _visit_deref_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        let arg = match args {
            [arg] => arg,
            _ => return Err("unexpected argument(s) in `deref` filter".into()),
        };
        buf.write("*");
        self.visit_expr(buf, arg)?;
        Ok(DisplayWrap::Unwrapped)
    }

    fn _visit_safe_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        if args.len() != 1 {
            return Err("unexpected argument(s) in `safe` filter".into());
        }
        buf.write(CRATE);
        buf.write("::filters::safe(");
        buf.write(self.input.escaper);
        buf.write(", ");
        self._visit_args(buf, args)?;
        buf.write(")?");
        Ok(DisplayWrap::Wrapped)
    }

    fn _visit_escape_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        if args.len() > 2 {
            return Err("only two arguments allowed to escape filter".into());
        }
        let opt_escaper = match args.get(1) {
            Some(Expr::StrLit(name)) => Some(*name),
            Some(_) => return Err("invalid escaper type for escape filter".into()),
            None => None,
        };
        let escaper = match opt_escaper {
            Some(name) => self
                .input
                .config
                .escapers
                .iter()
                .find_map(|(escapers, escaper)| escapers.contains(name).then_some(escaper))
                .ok_or_else(|| CompileError::from("invalid escaper for escape filter"))?,
            None => self.input.escaper,
        };
        buf.write(CRATE);
        buf.write("::filters::escape(");
        buf.write(escaper);
        buf.write(", ");
        self._visit_args(buf, &args[..1])?;
        buf.write(")?");
        Ok(DisplayWrap::Wrapped)
    }

    fn _visit_format_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        let fmt = match args {
            [Expr::StrLit(fmt), ..] => fmt,
            _ => return Err(r#"use filter format like `"a={} b={}"|format(a, b)`"#.into()),
        };
        buf.write("::std::format!(");
        self.visit_str_lit(buf, fmt);
        if args.len() > 1 {
            buf.write(", ");
            self._visit_args(buf, &args[1..])?;
        }
        buf.write(")");
        Ok(DisplayWrap::Unwrapped)
    }

    fn _visit_fmt_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        let fmt = match args {
            [_, Expr::StrLit(fmt)] => fmt,
            _ => return Err(r#"use filter fmt like `value|fmt("{:?}")`"#.into()),
        };
        buf.write("::std::format!(");
        self.visit_str_lit(buf, fmt);
        buf.write(", ");
        self._visit_args(buf, &args[..1])?;
        buf.write(")");
        Ok(DisplayWrap::Unwrapped)
    }

    // Force type coercion on first argument to `join` filter (see #39).
    fn _visit_join_filter(
        &mut self,
        buf: &mut Buffer,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        buf.write(CRATE);
        buf.write("::filters::join((&");
        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                buf.write(", &");
            }
            self.visit_expr(buf, arg)?;
            if i == 0 {
                buf.write(").into_iter()");
            }
        }
        buf.write(")?");
        Ok(DisplayWrap::Unwrapped)
    }

    fn _visit_args(&mut self, buf: &mut Buffer, args: &[Expr<'_>]) -> Result<(), CompileError> {
        if args.is_empty() {
            return Ok(());
        }

        for (i, arg) in args.iter().enumerate() {
            if i > 0 {
                buf.write(", ");
            }

            let borrow = !is_copyable(arg);
            if borrow {
                buf.write("&(");
            }

            match arg {
                Expr::Call(left, _) if !matches!(left.as_ref(), Expr::Path(_)) => {
                    buf.writeln("{")?;
                    self.visit_expr(buf, arg)?;
                    buf.writeln("}")?;
                }
                _ => {
                    self.visit_expr(buf, arg)?;
                }
            }

            if borrow {
                buf.write(")");
            }
        }
        Ok(())
    }

    fn visit_attr(
        &mut self,
        buf: &mut Buffer,
        obj: &Expr<'_>,
        attr: &str,
    ) -> Result<DisplayWrap, CompileError> {
        if let Expr::Var(name) = *obj {
            if name == "loop" {
                if attr == "index" {
                    buf.write("(_loop_item.index + 1)");
                    return Ok(DisplayWrap::Unwrapped);
                } else if attr == "index0" {
                    buf.write("_loop_item.index");
                    return Ok(DisplayWrap::Unwrapped);
                } else if attr == "first" {
                    buf.write("_loop_item.first");
                    return Ok(DisplayWrap::Unwrapped);
                } else if attr == "last" {
                    buf.write("_loop_item.last");
                    return Ok(DisplayWrap::Unwrapped);
                } else {
                    return Err("unknown loop variable".into());
                }
            }
        }
        self.visit_expr(buf, obj)?;
        buf.write(&format!(".{}", normalize_identifier(attr)));
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_index(
        &mut self,
        buf: &mut Buffer,
        obj: &Expr<'_>,
        key: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        buf.write("&");
        self.visit_expr(buf, obj)?;
        buf.write("[");
        self.visit_expr(buf, key)?;
        buf.write("]");
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_call(
        &mut self,
        buf: &mut Buffer,
        left: &Expr<'_>,
        args: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        match left {
            Expr::Attr(left, method) if **left == Expr::Var("loop") => match *method {
                "cycle" => match args {
                    [arg] => {
                        if matches!(arg, Expr::Array(arr) if arr.is_empty()) {
                            return Err("loop.cycle(…) cannot use an empty array".into());
                        }
                        buf.write("({");
                        buf.write("let _cycle = &(");
                        self.visit_expr(buf, arg)?;
                        buf.writeln(");")?;
                        buf.writeln("let _len = _cycle.len();")?;
                        buf.writeln("if _len == 0 {")?;
                        buf.write("return ::core::result::Result::Err(");
                        buf.write(CRATE);
                        buf.writeln("::Error::Fmt(::core::fmt::Error));")?;
                        buf.writeln("}")?;
                        buf.writeln("_cycle[_loop_item.index % _len]")?;
                        buf.writeln("})")?;
                    }
                    _ => return Err("loop.cycle(…) expects exactly one argument".into()),
                },
                s => return Err(format!("unknown loop method: {s:?}").into()),
            },
            left => {
                match left {
                    Expr::Var(name) => match self.locals.resolve(name) {
                        Some(resolved) => buf.write(&resolved),
                        None => buf.write(&format!("(&self.{})", normalize_identifier(name))),
                    },
                    left => {
                        self.visit_expr(buf, left)?;
                    }
                }

                buf.write("(");
                self._visit_args(buf, args)?;
                buf.write(")");
            }
        }
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_unary(
        &mut self,
        buf: &mut Buffer,
        op: &str,
        inner: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        buf.write(op);
        self.visit_expr(buf, inner)?;
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_range(
        &mut self,
        buf: &mut Buffer,
        op: &str,
        left: Option<&Expr<'_>>,
        right: Option<&Expr<'_>>,
    ) -> Result<DisplayWrap, CompileError> {
        if let Some(left) = left {
            self.visit_expr(buf, left)?;
        }
        buf.write(op);
        if let Some(right) = right {
            self.visit_expr(buf, right)?;
        }
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_binop(
        &mut self,
        buf: &mut Buffer,
        op: &str,
        left: &Expr<'_>,
        right: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        self.visit_expr(buf, left)?;
        buf.write(&format!(" {op} "));
        self.visit_expr(buf, right)?;
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_group(
        &mut self,
        buf: &mut Buffer,
        inner: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        buf.write("(");
        self.visit_expr(buf, inner)?;
        buf.write(")");
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_tuple(
        &mut self,
        buf: &mut Buffer,
        exprs: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        buf.write("(");
        for (index, expr) in exprs.iter().enumerate() {
            if index > 0 {
                buf.write(" ");
            }
            self.visit_expr(buf, expr)?;
            buf.write(",");
        }
        buf.write(")");
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_named_argument(
        &mut self,
        buf: &mut Buffer,
        expr: &Expr<'_>,
    ) -> Result<DisplayWrap, CompileError> {
        self.visit_expr(buf, expr)?;
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_array(
        &mut self,
        buf: &mut Buffer,
        elements: &[Expr<'_>],
    ) -> Result<DisplayWrap, CompileError> {
        buf.write("[");
        for (i, el) in elements.iter().enumerate() {
            if i > 0 {
                buf.write(", ");
            }
            self.visit_expr(buf, el)?;
        }
        buf.write("]");
        Ok(DisplayWrap::Unwrapped)
    }

    fn visit_path(&mut self, buf: &mut Buffer, path: &[&str]) -> DisplayWrap {
        for (i, part) in path.iter().enumerate() {
            if i > 0 {
                buf.write("::");
            }
            buf.write(part);
        }
        DisplayWrap::Unwrapped
    }

    fn visit_var(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        if s == "self" {
            buf.write(s);
            return DisplayWrap::Unwrapped;
        }

        buf.write(normalize_identifier(&self.locals.resolve_or_self(s)));
        DisplayWrap::Unwrapped
    }

    fn visit_generated(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        buf.write(s);
        DisplayWrap::Unwrapped
    }

    fn visit_bool_lit(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        buf.write(s);
        DisplayWrap::Unwrapped
    }

    fn visit_str_lit(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        buf.write(&format!("\"{s}\""));
        DisplayWrap::Unwrapped
    }

    fn visit_char_lit(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        buf.write(&format!("'{s}'"));
        DisplayWrap::Unwrapped
    }

    fn visit_num_lit(&mut self, buf: &mut Buffer, s: &str) -> DisplayWrap {
        buf.write(s);
        DisplayWrap::Unwrapped
    }

    fn visit_target(
        &mut self,
        buf: &mut Buffer,
        initialized: bool,
        first_level: bool,
        target: &Target<'a>,
    ) {
        match target {
            Target::Name("_") => {
                buf.write("_");
            }
            Target::Name(name) => {
                let name = normalize_identifier(name);
                match initialized {
                    true => self
                        .locals
                        .insert(Cow::Borrowed(name), LocalMeta::initialized()),
                    false => self.locals.insert_with_default(Cow::Borrowed(name)),
                }
                buf.write(name);
            }
            Target::OrChain(targets) => match targets.first() {
                None => buf.write("_"),
                Some(first_target) => {
                    self.visit_target(buf, initialized, first_level, first_target);
                    for target in &targets[1..] {
                        buf.write(" | ");
                        self.visit_target(buf, initialized, first_level, target);
                    }
                }
            },
            Target::Tuple(path, targets) => {
                buf.write(&path.join("::"));
                buf.write("(");
                for target in targets {
                    self.visit_target(buf, initialized, false, target);
                    buf.write(",");
                }
                buf.write(")");
            }
            Target::Struct(path, targets) => {
                buf.write(&path.join("::"));
                buf.write(" { ");
                for (name, target) in targets {
                    buf.write(normalize_identifier(name));
                    buf.write(": ");
                    self.visit_target(buf, initialized, false, target);
                    buf.write(",");
                }
                buf.write(" }");
            }
            Target::Path(path) => {
                self.visit_path(buf, path);
            }
            Target::StrLit(s) => {
                if first_level {
                    buf.write("&");
                }
                self.visit_str_lit(buf, s);
            }
            Target::NumLit(s) => {
                if first_level {
                    buf.write("&");
                }
                self.visit_num_lit(buf, s);
            }
            Target::CharLit(s) => {
                if first_level {
                    buf.write("&");
                }
                self.visit_char_lit(buf, s);
            }
            Target::BoolLit(s) => {
                if first_level {
                    buf.write("&");
                }
                buf.write(s);
            }
        }
    }

    /* Helper methods for dealing with whitespace nodes */

    // Combines `flush_ws()` and `prepare_ws()` to handle both trailing whitespace from the
    // preceding literal and leading whitespace from the succeeding literal.
    fn handle_ws(&mut self, ws: Ws) {
        self.flush_ws(ws);
        self.prepare_ws(ws);
    }

    fn should_trim_ws(&self, ws: Option<Whitespace>) -> WhitespaceHandling {
        match ws {
            Some(Whitespace::Suppress) => WhitespaceHandling::Suppress,
            Some(Whitespace::Preserve) => WhitespaceHandling::Preserve,
            Some(Whitespace::Minimize) => WhitespaceHandling::Minimize,
            None => self.input.config.whitespace,
        }
    }

    // If the previous literal left some trailing whitespace in `next_ws` and the
    // prefix whitespace suppressor from the given argument, flush that whitespace.
    // In either case, `next_ws` is reset to `None` (no trailing whitespace).
    fn flush_ws(&mut self, ws: Ws) {
        if self.next_ws.is_none() {
            return;
        }

        // If `whitespace` is set to `suppress`, we keep the whitespace characters only if there is
        // a `+` character.
        match self.should_trim_ws(ws.0) {
            WhitespaceHandling::Preserve => {
                let val = self.next_ws.unwrap();
                if !val.is_empty() {
                    self.buf_writable.push(Writable::Lit(val));
                }
            }
            WhitespaceHandling::Minimize => {
                let val = self.next_ws.unwrap();
                if !val.is_empty() {
                    self.buf_writable
                        .push(Writable::Lit(match val.contains('\n') {
                            true => "\n",
                            false => " ",
                        }));
                }
            }
            WhitespaceHandling::Suppress => {}
        }
        self.next_ws = None;
    }

    // Sets `skip_ws` to match the suffix whitespace suppressor from the given
    // argument, to determine whether to suppress leading whitespace from the
    // next literal.
    fn prepare_ws(&mut self, ws: Ws) {
        self.skip_ws = self.should_trim_ws(ws.1);
    }
}

struct Buffer {
    // The buffer to generate the code into
    buf: String,
    // The current level of indentation (in spaces)
    indent: u8,
    // Whether the output buffer is currently at the start of a line
    start: bool,
    discard: bool,
}

impl Buffer {
    fn new(indent: u8) -> Self {
        Self {
            buf: String::new(),
            indent,
            start: true,
            discard: false,
        }
    }

    fn writeln(&mut self, s: &str) -> Result<(), CompileError> {
        if self.discard {
            return Ok(());
        }
        if s == "}" {
            self.dedent()?;
        }
        if !s.is_empty() {
            self.write(s);
        }
        self.buf.push('\n');
        if s.ends_with('{') {
            self.indent();
        }
        self.start = true;
        Ok(())
    }

    fn write(&mut self, s: &str) {
        if self.discard {
            return;
        }
        if self.start {
            for _ in 0..(self.indent * 4) {
                self.buf.push(' ');
            }
            self.start = false;
        }

        self.buf.push_str(s);
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) -> Result<(), CompileError> {
        if self.indent == 0 {
            return Err("dedent() called while indentation == 0".into());
        }
        self.indent -= 1;
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct LocalMeta {
    refs: Option<String>,
    initialized: bool,
}

impl LocalMeta {
    fn initialized() -> Self {
        Self {
            refs: None,
            initialized: true,
        }
    }

    fn with_ref(refs: String) -> Self {
        Self {
            refs: Some(refs),
            initialized: true,
        }
    }
}

// type SetChain<'a, T> = MapChain<'a, T, ()>;

#[derive(Debug, Clone)]
pub(crate) struct MapChain<'a, K, V>
where
    K: cmp::Eq + hash::Hash,
{
    parent: Option<&'a MapChain<'a, K, V>>,
    scopes: Vec<HashMap<K, V>>,
}

impl<'a, K: 'a, V: 'a> MapChain<'a, K, V>
where
    K: cmp::Eq + hash::Hash,
{
    fn with_parent<'p>(parent: &'p MapChain<'_, K, V>) -> MapChain<'p, K, V> {
        MapChain {
            parent: Some(parent),
            scopes: vec![HashMap::new()],
        }
    }

    /// Iterates the scopes in reverse and returns `Some(LocalMeta)`
    /// from the first scope where `key` exists.
    fn get(&self, key: &K) -> Option<&V> {
        let mut scopes = self.scopes.iter().rev();
        scopes
            .find_map(|set| set.get(key))
            .or_else(|| self.parent.and_then(|set| set.get(key)))
    }

    fn is_current_empty(&self) -> bool {
        self.scopes.last().unwrap().is_empty()
    }

    fn insert(&mut self, key: K, val: V) {
        self.scopes.last_mut().unwrap().insert(key, val);

        // Note that if `insert` returns `Some` then it implies
        // an identifier is reused. For e.g. `{% macro f(a, a) %}`
        // and `{% let (a, a) = ... %}` then this results in a
        // generated template, which when compiled fails with the
        // compile error "identifier `a` used more than once".
    }

    fn insert_with_default(&mut self, key: K)
    where
        V: Default,
    {
        self.insert(key, V::default());
    }

    fn push(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop().unwrap();
        assert!(!self.scopes.is_empty());
    }
}

impl MapChain<'_, Cow<'_, str>, LocalMeta> {
    fn resolve(&self, name: &str) -> Option<String> {
        let name = normalize_identifier(name);
        self.get(&Cow::Borrowed(name)).map(|meta| match &meta.refs {
            Some(expr) => expr.clone(),
            None => name.to_string(),
        })
    }

    fn resolve_or_self(&self, name: &str) -> String {
        let name = normalize_identifier(name);
        self.resolve(name).unwrap_or_else(|| format!("self.{name}"))
    }
}

impl<K: Eq + hash::Hash, V> Default for MapChain<'_, K, V> {
    fn default() -> Self {
        Self {
            parent: None,
            scopes: vec![HashMap::new()],
        }
    }
}

/// Returns `true` if enough assumptions can be made,
/// to determine that `self` is copyable.
fn is_copyable(expr: &Expr<'_>) -> bool {
    is_copyable_within_op(expr, false)
}

fn is_copyable_within_op(expr: &Expr<'_>, within_op: bool) -> bool {
    match expr {
        Expr::BoolLit(_) | Expr::NumLit(_) | Expr::StrLit(_) | Expr::CharLit(_) => true,
        Expr::Unary(.., expr) => is_copyable_within_op(expr, true),
        Expr::BinOp(_, lhs, rhs) => {
            is_copyable_within_op(lhs, true) && is_copyable_within_op(rhs, true)
        }
        Expr::Range(..) => true,
        // The result of a call likely doesn't need to be borrowed,
        // as in that case the call is more likely to return a
        // reference in the first place then.
        Expr::Call(..) | Expr::Path(..) => true,
        // If the `expr` is within a `Unary` or `BinOp` then
        // an assumption can be made that the operand is copy.
        // If not, then the value is moved and adding `.clone()`
        // will solve that issue. However, if the operand is
        // implicitly borrowed, then it's likely not even possible
        // to get the template to compile.
        _ => within_op && is_attr_self(expr),
    }
}

/// Returns `true` if this is an `Attr` where the `obj` is `"self"`.
pub(crate) fn is_attr_self(expr: &Expr<'_>) -> bool {
    match expr {
        Expr::Attr(obj, _) if matches!(obj.as_ref(), Expr::Var("self")) => true,
        Expr::Attr(obj, _) if matches!(obj.as_ref(), Expr::Attr(..)) => is_attr_self(obj),
        _ => false,
    }
}

/// Returns `true` if the outcome of this expression may be used multiple times in the same
/// `write!()` call, without evaluating the expression again, i.e. the expression should be
/// side-effect free.
pub(crate) fn is_cacheable(expr: &Expr<'_>) -> bool {
    match expr {
        // Literals are the definition of pure:
        Expr::BoolLit(_) => true,
        Expr::NumLit(_) => true,
        Expr::StrLit(_) => true,
        Expr::CharLit(_) => true,
        // fmt::Display should have no effects:
        Expr::Var(_) => true,
        Expr::Path(_) => true,
        // Check recursively:
        Expr::Array(args) => args.iter().all(is_cacheable),
        Expr::Attr(lhs, _) => is_cacheable(lhs),
        Expr::Index(lhs, rhs) => is_cacheable(lhs) && is_cacheable(rhs),
        Expr::Filter(Filter { arguments, .. }) => arguments.iter().all(is_cacheable),
        Expr::Unary(_, arg) => is_cacheable(arg),
        Expr::BinOp(_, lhs, rhs) => is_cacheable(lhs) && is_cacheable(rhs),
        Expr::Range(_, lhs, rhs) => {
            lhs.as_ref().map_or(true, |v| is_cacheable(v))
                && rhs.as_ref().map_or(true, |v| is_cacheable(v))
        }
        Expr::Group(arg) => is_cacheable(arg),
        Expr::Tuple(args) => args.iter().all(is_cacheable),
        Expr::NamedArgument(_, expr) => is_cacheable(expr),
        // We have too little information to tell if the expression is pure:
        Expr::Call(_, _) => false,
        Expr::RustMacro(_, _) => false,
        Expr::Try(_) => false,
        Expr::Generated(_) => true,
    }
}

fn median(sizes: &mut [usize]) -> usize {
    sizes.sort_unstable();
    if sizes.len() % 2 == 1 {
        sizes[sizes.len() / 2]
    } else {
        (sizes[sizes.len() / 2 - 1] + sizes[sizes.len() / 2]) / 2
    }
}

/// In `FilterBlock`, we have a recursive `Expr::Filter` entry, where the more you go "down",
/// the sooner you are called in the Rust code. Example:
///
/// ```text
/// {% filter a|b|c %}bla{% endfilter %}
/// ```
///
/// Will be translated as:
///
/// ```text
/// FilterBlock {
///    filters: Filter {
///        name: "c",
///        arguments: vec![
///            Filter {
///                name: "b",
///                arguments: vec![
///                    Filter {
///                        name: "a",
///                        arguments: vec![],
///                    }.
///                ],
///            }
///        ],
///    },
///    // ...
/// }
/// ```
///
/// So in here, we want to insert the variable containing the content of the filter block inside
/// the call to `"a"`. To do so, we recursively go through all `Filter` and finally insert our
/// variable as the first argument to the `"a"` call.
fn insert_first_filter_argument(args: &mut Vec<Expr<'_>>, var_name: String) {
    if let Some(Expr::Filter(Filter { arguments, .. })) = args.first_mut() {
        insert_first_filter_argument(arguments, var_name);
    } else {
        args.insert(0, Expr::Generated(var_name));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AstLevel {
    Top,
    Block,
    Nested,
}

#[derive(Clone, Copy, Debug)]
enum DisplayWrap {
    Wrapped,
    Unwrapped,
}

#[derive(Default, Debug)]
struct WritableBuffer<'a> {
    buf: Vec<Writable<'a>>,
    discard: bool,
}

impl<'a> WritableBuffer<'a> {
    fn push(&mut self, writable: Writable<'a>) {
        if !self.discard {
            self.buf.push(writable);
        }
    }
}

impl<'a> Deref for WritableBuffer<'a> {
    type Target = [Writable<'a>];

    fn deref(&self) -> &Self::Target {
        &self.buf[..]
    }
}

#[derive(Debug)]
enum Writable<'a> {
    Lit(&'a str),
    Expr(&'a Expr<'a>),
    Generated(String, DisplayWrap),
}

struct WriteParts {
    size_hint: usize,
    buffers: Option<WritePartsBuffers>,
}

/// If "expr" is `None`, it means we can generate code like this:
///
/// ```ignore
/// let var = format;
/// ```
///
/// Otherwise we need to format "expr" using "format":
///
/// ```ignore
/// let var = format!(format, expr);
/// ```
struct WritePartsBuffers {
    format: Buffer,
    expr: Option<Buffer>,
}

// Identifiers to be replaced with raw identifiers, so as to avoid
// collisions between template syntax and Rust's syntax. In particular
// [Rust keywords](https://doc.rust-lang.org/reference/keywords.html)
// should be replaced, since they're not reserved words in Reva
// syntax but have a high probability of causing problems in the
// generated code.
//
// This list excludes the Rust keywords *self*, *Self*, and *super*
// because they are not allowed to be raw identifiers, and *loop*
// because it's used something like a keyword in the template
// language.
fn normalize_identifier(ident: &str) -> &str {
    // This table works for as long as the replacement string is the original string
    // prepended with "r#". The strings get right-padded to the same length with b'_'.
    // While the code does not need it, please keep the list sorted when adding new
    // keywords.

    // FIXME: Replace with `[core:ascii::Char; MAX_REPL_LEN]` once
    //        <https://github.com/rust-lang/rust/issues/110998> is stable.

    const MAX_KW_LEN: usize = 8;
    const MAX_REPL_LEN: usize = MAX_KW_LEN + 2;

    const KW0: &[[u8; MAX_REPL_LEN]] = &[];
    const KW1: &[[u8; MAX_REPL_LEN]] = &[];
    const KW2: &[[u8; MAX_REPL_LEN]] = &[
        *b"r#as______",
        *b"r#do______",
        *b"r#fn______",
        *b"r#if______",
        *b"r#in______",
    ];
    const KW3: &[[u8; MAX_REPL_LEN]] = &[
        *b"r#box_____",
        *b"r#dyn_____",
        *b"r#for_____",
        *b"r#let_____",
        *b"r#mod_____",
        *b"r#mut_____",
        *b"r#pub_____",
        *b"r#ref_____",
        *b"r#try_____",
        *b"r#use_____",
    ];
    const KW4: &[[u8; MAX_REPL_LEN]] = &[
        *b"r#else____",
        *b"r#enum____",
        *b"r#impl____",
        *b"r#move____",
        *b"r#priv____",
        *b"r#true____",
        *b"r#type____",
    ];
    const KW5: &[[u8; MAX_REPL_LEN]] = &[
        *b"r#async___",
        *b"r#await___",
        *b"r#break___",
        *b"r#const___",
        *b"r#crate___",
        *b"r#false___",
        *b"r#final___",
        *b"r#macro___",
        *b"r#match___",
        *b"r#trait___",
        *b"r#where___",
        *b"r#while___",
        *b"r#yield___",
    ];
    const KW6: &[[u8; MAX_REPL_LEN]] = &[
        *b"r#become__",
        *b"r#extern__",
        *b"r#return__",
        *b"r#static__",
        *b"r#struct__",
        *b"r#typeof__",
        *b"r#unsafe__",
    ];
    const KW7: &[[u8; MAX_REPL_LEN]] = &[*b"r#unsized_", *b"r#virtual_"];
    const KW8: &[[u8; MAX_REPL_LEN]] = &[*b"r#abstract", *b"r#continue", *b"r#override"];

    const KWS: &[&[[u8; MAX_REPL_LEN]]] = &[KW0, KW1, KW2, KW3, KW4, KW5, KW6, KW7, KW8];

    // Ensure that all strings are ASCII, because we use `from_utf8_unchecked()` further down.
    const _: () = {
        let mut i = 0;
        while i < KWS.len() {
            let mut j = 0;
            while KWS[i].len() < j {
                let mut k = 0;
                while KWS[i][j].len() < k {
                    assert!(KWS[i][j][k].is_ascii());
                    k += 1;
                }
                j += 1;
            }
            i += 1;
        }
    };

    if ident.len() > MAX_KW_LEN {
        return ident;
    }
    let kws = KWS[ident.len()];

    let mut padded_ident = [b'_'; MAX_KW_LEN];
    padded_ident[..ident.len()].copy_from_slice(ident.as_bytes());

    // Since the individual buckets are quite short, a linear search is faster than a binary search.
    let replacement = match kws
        .iter()
        .find(|probe| padded_ident == <[u8; MAX_KW_LEN]>::try_from(&probe[2..]).unwrap())
    {
        Some(replacement) => replacement,
        None => return ident,
    };

    // SAFETY: We know that the input byte slice is pure-ASCII.
    unsafe { std::str::from_utf8_unchecked(&replacement[..ident.len() + 2]) }
}
