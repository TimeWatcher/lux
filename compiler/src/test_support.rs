use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn temp_root(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "lux_test_{name}_{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ))
}

pub fn test_std_package_root() -> PathBuf {
    let root = temp_root("std_packages");
    fs::create_dir_all(&root).expect("create test package root");
    fs::write(
        root.join("lux.package.toml"),
        r#"
name = "lux-std-test"

[[package]]
id = "@lux/std"
version = "0.1.0"
path = "lux/std"

[[package]]
id = "@lux/reactive"
version = "0.1.0"
path = "lux/reactive"

[[package]]
id = "@lux/gmod"
version = "0.1.0"
path = "lux/gmod"

[[package]]
id = "@lux/ui"
version = "0.1.0"
path = "lux/ui"
depends = ["@lux/reactive >=0.1 <0.2"]

[[package]]
id = "@lux/compile/macro"
version = "0.1.0"
path = "lux/compile/macro"

[[package]]
id = "@lux/compile/host"
version = "0.1.0"
path = "lux/compile/host"

[[package]]
id = "@lux/macros"
version = "0.1.0"
path = "lux/macros"
depends = ["@lux/compile/macro >=0.1 <0.2"]

[[package]]
id = "@lux/gmod/macros"
version = "0.1.0"
path = "lux/gmod/macros"
depends = ["@lux/compile/macro >=0.1 <0.2"]
"#,
    )
    .expect("write test package set manifest");

    write_package(
        &root,
        "lux/std",
        "src",
        r#"
export const arr = {}
export const dict = {}
export const pool = {}
"#,
    );
    write_package(
        &root,
        "lux/reactive",
        "src",
        r#"
export fn signal(value) = { value = value }
export fn effect(callback) = callback()
"#,
    );
    write_package(
        &root,
        "lux/gmod",
        "src",
        r#"
export fn valid(value) = value ~= nil
export const hookx = {}
export const netx = {}
"#,
    );
    write_package(
        &root,
        "lux/ui",
        "src",
        r#"
import { signal } from "@lux/reactive"

export fn node(kind, props, children) =
  { kind = kind, props = props, children = children }

export fn mount(factory, render) =
  render(signal(factory()))

export fn Column(props, children) = node("Column", props, children)
export fn Row(props, children) = node("Row", props, children)
export fn Label(props, children) = node("Label", props, children)
export fn Button(props, children) = node("Button", props, children)
"#,
    );
    write_package(
        &root,
        "lux/compile/macro",
        "compiletime",
        r#"
import * as ast from "@lux/compile/ast"

export fn fail(ctx, code, message, span) {
  ctx.error(code, message, span)
  return nil
}

export fn expectArgCount(ctx, call, count, usage) {
  if call.argc == count {
    return true
  }
  return fail(ctx, "MACRO003", usage, call.span)
}

export fn expectArgCount2(ctx, call, first, second, usage) {
  if call.argc == first or call.argc == second {
    return true
  }
  return fail(ctx, "MACRO003", usage, call.span)
}

export fn requireStatement(ctx, call, usage) {
  if call.position == "statement" {
    return true
  }
  return fail(ctx, "MACRO004", usage, call.span)
}

export fn requireStatementOrExpression(ctx, call, usage) {
  if call.position == "statement" or call.position == "expression" {
    return true
  }
  return fail(ctx, "MACRO004", usage, call.span)
}

export fn path(names, span) {
  return pathFrom(names, 2, ast.ident(names[1], span), span)
}

fn pathFrom(names, index, expr, span) {
  if index > #names {
    return expr
  }
  return pathFrom(names, index + 1, ast.member(expr, names[index], span), span)
}

export fn callPath(names, args, span) = ast.call(path(names, span), args, span)
export fn exprStmt(expr, span) = ast.exprStmt(expr, span)
export fn callStmt(names, args, span) = exprStmt(callPath(names, args, span), span)
export fn localOne(name, value, span) = ast.localDecl({ name }, { value }, span)

export fn localExpr(ctx, prefix, value, span) {
  local name = ctx.gensym(prefix)
  return {
    name = name,
    ref = ast.ident(name, span),
    stmt = localOne(name, value, span)
  }
}

export fn localMany(ctx, specs, span) {
  return localManyFrom(ctx, specs, span, 1, {}, {}, {})
}

fn localManyFrom(ctx, specs, span, index, names, refs, stmts) {
  if index > #specs {
    return { names = names, refs = refs, stmts = stmts }
  }
  local item = localExpr(ctx, specs[index].prefix, specs[index].value, span)
  names[index] = item.name
  refs[index] = item.ref
  stmts[index] = item.stmt
  return localManyFrom(ctx, specs, span, index + 1, names, refs, stmts)
}

export fn doStmt(statements, span) = ast.doStmt(statements, span)
export fn doBlock(statements, span) = doStmt(statements, span)
export fn doExpr(statements, tail, span) = ast.doExpr(statements, tail, span)
export fn exprWithSetup(statements, value, span) = ast.expr(doExpr(statements, value, span))
export fn lowerIntoTarget(statements, value, span) = exprWithSetup(statements, value, span)
export fn realmBlock(realm, statements, span) = ast.realmBlock(realm, statements, span)
export fn sharedBlock(statements, span) = realmBlock("shared", statements, span)
export fn clientBlock(statements, span) = realmBlock("client", statements, span)
export fn serverBlock(statements, span) = realmBlock("server", statements, span)
export fn ifStmt(cond, thenStmts, elseStmts, span) = ast.ifStmt(cond, thenStmts, elseStmts, span)
export fn guard(name, statements, span) = ifStmt(ast.ident(name, span), statements, nil, span)
export fn stmts(statements) = ast.stmts(statements)
export fn expr(expr) = ast.expr(expr)
export fn block(statements, tail, span) = ast.block(statements, tail, span)
export fn func(params, body, span) = ast.func(params, body, span)
export fn call(callee, args, span) = ast.call(callee, args, span)
export fn assign(targets, values, span) = ast.assign(targets, values, span)
export fn assignOne(target, value, span) = assign({ target }, { value }, span)
export fn ident(name, span) = ast.ident(name, span)
export fn data(value) = ast.data(value)
export fn number(value, span) = ast.number(value, span)
export fn string(value, span) = ast.string(value, span)
export fn index(base, value, span) = ast.index(base, value, span)
export fn returnOne(value, span) = ast.returnStmt({ value }, span)
export fn returnMany(values, span) = ast.returnStmt(values, span)
export fn iifeBlock(block, span) = call(func({}, block, span), {}, span)
export fn table(fields, span) = ast.table(fields, span)
export fn field(value, span) = ast.arrayField(value, span)
export fn named(name, value, span) = ast.namedField(name, value, span)
export fn keyed(key, value, span) = ast.keyedField(key, value, span)
export fn fnDecl(name, params, body, span) = ast.fnDecl(name, params, body, span)
export fn exportRuntime(realm, stmt, span) = ast.exportRuntime(realm, stmt, span)
"#,
    );
    write_package(
        &root,
        "lux/compile/host",
        "compiletime",
        r#"
import * as ir from "@lux/compile/ir"

export fn tailTableParts(expr) = ir.tailTableParts(expr)
export fn importRuntimeName(ctx, imported, preferredLocal) =
  ctx.importRuntime(imported, preferredLocal)
export fn importRuntimeIdent(ctx, imported, preferredLocal, origin) =
  ir.ident(importRuntimeName(ctx, imported, preferredLocal), origin)
export fn callRuntime(ctx, imported, preferredLocal, args, origin) =
  ir.call(importRuntimeIdent(ctx, imported, preferredLocal, origin), args, origin)
"#,
    );
    write_package(
        &root,
        "lux/macros",
        "compiletime",
        r#"
import * as m from "@lux/compile/macro"

export macro fn dbg(ctx, call) {
  if not m.expectArgCount(ctx, call, 1, "`dbg` expects exactly one expression argument") {
    return nil
  }
  local value = m.localExpr(ctx, "dbg", call.args[1], call.span)
  return m.exprWithSetup({
    value.stmt,
    m.callStmt({ "print" }, { m.string(ctx.label(call.span), call.span), value.ref }, call.span)
  }, value.ref, call.span)
}
"#,
    );
    write_package(
        &root,
        "lux/gmod/macros",
        "compiletime",
        r#"
import * as m from "@lux/compile/macro"

fn hookAddStmt(event, id, callback, span) =
  m.callStmt({ "hook", "Add" }, { event, id, callback }, span)

fn netAddStringStmt(name, span) =
  m.callStmt({ "util", "AddNetworkString" }, { name }, span)

fn netReceiveStmt(name, callback, span) =
  m.callStmt({ "net", "Receive" }, { name, callback }, span)

export macro fn defineHook(ctx, call) {
  if not m.expectArgCount2(ctx, call, 2, 3, "`defineHook` expects (event, callback) or (event, id, callback)") {
    return nil
  }
  if not m.requireStatementOrExpression(ctx, call, "`defineHook` can only be used as a statement or expression") {
    return nil
  }

  local eventLocal = m.localExpr(ctx, "hook_event", call.args[1], call.span)
  local idName = ctx.gensym("hook_id")
  local idValue = m.ident(idName, call.span)

  if call.position == "expression" {
    if call.argc == 2 {
      local callbackLocal = m.localExpr(ctx, "hook_callback", call.args[2], call.span)
      return m.exprWithSetup({
        eventLocal.stmt,
        callbackLocal.stmt,
        m.localOne(idName, m.string(ctx.gensymString("hook"), call.span), call.span),
        hookAddStmt(eventLocal.ref, idValue, callbackLocal.ref, call.span)
      }, idValue, call.span)
    }
    local callbackLocal = m.localExpr(ctx, "hook_callback", call.args[3], call.span)
    return m.exprWithSetup({
      eventLocal.stmt,
      m.localOne(idName, call.args[2], call.span),
      callbackLocal.stmt,
      hookAddStmt(eventLocal.ref, idValue, callbackLocal.ref, call.span)
    }, idValue, call.span)
  }

  if call.argc == 2 {
    local callbackLocal = m.localExpr(ctx, "hook_callback", call.args[2], call.span)
    return m.stmts({
      m.doStmt({
        eventLocal.stmt,
        callbackLocal.stmt,
        m.localOne(idName, m.string(ctx.gensymString("hook"), call.span), call.span),
        hookAddStmt(eventLocal.ref, idValue, callbackLocal.ref, call.span)
      }, call.span)
    })
  }

  local callbackLocal = m.localExpr(ctx, "hook_callback", call.args[3], call.span)
  return m.stmts({
    m.doStmt({
      eventLocal.stmt,
      m.localOne(idName, call.args[2], call.span),
      callbackLocal.stmt,
      hookAddStmt(eventLocal.ref, idValue, callbackLocal.ref, call.span)
    }, call.span)
  })
}

export macro fn defineNetString(ctx, call) {
  if not m.expectArgCount(ctx, call, 1, "`defineNetString` expects (name)") {
    return nil
  }
  if not m.requireStatement(ctx, call, "`defineNetString` can only be used as a statement") {
    return nil
  }
  local nameLocal = m.localExpr(ctx, "net_name", call.args[1], call.span)
  return m.stmts({
    m.serverBlock({
      nameLocal.stmt,
      netAddStringStmt(nameLocal.ref, call.span)
    }, call.span)
  })
}

export macro fn defineNetReceiver(ctx, call) {
  if not m.expectArgCount(ctx, call, 2, "`defineNetReceiver` expects (name, callback)") {
    return nil
  }
  local locals = m.localMany(ctx, {
    { prefix = "net_name", value = call.args[1] },
    { prefix = "net_callback", value = call.args[2] }
  }, call.span)
  return m.stmts({
    m.sharedBlock({
      locals.stmts[1],
      locals.stmts[2],
      m.serverBlock({
        netAddStringStmt(locals.refs[1], call.span)
      }, call.span),
      netReceiveStmt(locals.refs[1], locals.refs[2], call.span)
    }, call.span)
  })
}

export macro fn defineSharedNetReceiver(ctx, call) =
  defineNetReceiver(ctx, call)

export macro fn defineServerNetReceiver(ctx, call) {
  if not m.expectArgCount(ctx, call, 2, "`defineServerNetReceiver` expects (name, callback)") {
    return nil
  }
  local locals = m.localMany(ctx, {
    { prefix = "net_name", value = call.args[1] },
    { prefix = "net_callback", value = call.args[2] }
  }, call.span)
  return m.stmts({
    m.serverBlock({
      locals.stmts[1],
      locals.stmts[2],
      netAddStringStmt(locals.refs[1], call.span),
      netReceiveStmt(locals.refs[1], locals.refs[2], call.span)
    }, call.span)
  })
}

export macro fn defineClientNetReceiver(ctx, call) {
  if not m.expectArgCount(ctx, call, 2, "`defineClientNetReceiver` expects (name, callback)") {
    return nil
  }
  local locals = m.localMany(ctx, {
    { prefix = "net_name", value = call.args[1] },
    { prefix = "net_callback", value = call.args[2] }
  }, call.span)
  return m.stmts({
    m.clientBlock({
      locals.stmts[1],
      locals.stmts[2],
      netReceiveStmt(locals.refs[1], locals.refs[2], call.span)
    }, call.span)
  })
}
"#,
    );
    write_package(
        &root,
        "lux/ui",
        "host",
        r#"
import * as host from "@lux/compile/host"
import * as ir from "@lux/compile/ir"

export host package {
  target = "lux/ui",
  runtime = "lux/ui"
}

fn isNodeComponent(name) {
  local nodeComponents = {
    Column = true,
    Row = true,
    Label = true,
    Button = true,
    Text = true,
    Spacer = true,
    Panel = true
  }
  return nodeComponents[name] == true
}

export host expr fn foldNode(ctx, call) {
  if not isNodeComponent(call.imported) {
    return nil
  }
  local parts = host.tailTableParts(call.expr)
  if parts == nil {
    return nil
  }
  return host.callRuntime(ctx, "node", "__lux_ui_node", {
    ir.string(call.imported, call.expr),
    parts.props,
    parts.children
  }, call.expr)
}
"#,
    );

    root
}

fn write_package(root: &PathBuf, package: &str, phase: &str, source: &str) {
    let path = root.join(package).join(phase);
    fs::create_dir_all(&path).expect("create test package");
    fs::write(path.join("module.lux"), source).expect("write test package");
}
