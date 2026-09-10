use crate::extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
use codemap_graph::NodeKind;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node as TsNode, Parser};

pub struct RustExtractor;

const METHODS: [&str; 6] = ["get", "post", "put", "patch", "delete", "head"];

fn text<'a>(n: TsNode, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

fn named_children<'a>(n: TsNode<'a>) -> Vec<TsNode<'a>> {
    let mut c = n.walk();
    let v: Vec<TsNode> = n.children(&mut c).filter(|k| k.is_named()).collect();
    v
}

fn string_value(n: TsNode, src: &str) -> Option<String> {
    if n.kind() != "string_literal" {
        return None;
    }
    Some(text(n, src).trim_matches(['"', '#']).to_string())
}

impl LanguageExtractor for RustExtractor {
    fn extensions(&self) -> &'static [&'static str] {
        &["rs"]
    }

    fn extract(&self, src: &str) -> FileFacts {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("rust grammar");
        let Some(tree) = p.parse(src, None) else {
            return FileFacts::default();
        };
        let mut f = FileFacts::default();
        let mut imports = HashSet::new();
        collect_imports(tree.root_node(), src, &mut imports);
        f.imports = imports.iter().cloned().collect();
        walk(
            tree.root_node(),
            src,
            None,
            None,
            &imports,
            &HashMap::new(),
            &mut f,
        );
        f // tree dropped here — never retained
    }
}

fn collect_imports(n: TsNode, src: &str, out: &mut HashSet<String>) {
    for ch in named_children(n) {
        if ch.kind() == "use_declaration" {
            collect_idents(ch, src, out);
        }
        collect_imports(ch, src, out);
    }
}

fn collect_idents(n: TsNode, src: &str, out: &mut HashSet<String>) {
    for ch in named_children(n) {
        if matches!(ch.kind(), "identifier" | "type_identifier") {
            out.insert(text(ch, src).to_string());
        }
        collect_idents(ch, src, out);
    }
}

/// `let x = Type::new(...)` inside one function body -> {x: Type}. Parity with
/// the Python/TypeScript/Go local-binding heuristics.
fn local_bindings(scope: TsNode, src: &str) -> HashMap<String, String> {
    fn rec(n: TsNode, src: &str, out: &mut HashMap<String, String>) {
        for ch in named_children(n) {
            if ch.kind() == "let_declaration" {
                if let (Some(pat), Some(val)) = (
                    ch.child_by_field_name("pattern"),
                    ch.child_by_field_name("value"),
                ) {
                    if pat.kind() == "identifier" && val.kind() == "call_expression" {
                        if let Some(f) = val.child_by_field_name("function") {
                            if f.kind() == "scoped_identifier" {
                                if let Some(ty) = named_children(f).first() {
                                    out.insert(
                                        text(pat, src).to_string(),
                                        text(*ty, src).to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
            rec(ch, src, out);
        }
    }
    let mut out = HashMap::new();
    rec(scope, src, &mut out);
    out
}

fn walk(
    n: TsNode,
    src: &str,
    class: Option<&str>,
    func: Option<&str>,
    imports: &HashSet<String>,
    bindings: &HashMap<String, String>,
    f: &mut FileFacts,
) {
    for ch in named_children(n) {
        match ch.kind() {
            "struct_item" | "enum_item" | "trait_item" => {
                if let Some(name) = ch.child_by_field_name("name") {
                    f.containers.push(RawSymbol {
                        name: text(name, src).to_string(),
                        parent: None,
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Class,
                    });
                }
            }
            // `impl ChatService { ... }` — the impl type owns every method inside.
            "impl_item" => {
                let owner = ch
                    .child_by_field_name("type")
                    .map(|t| text(t, src).to_string());
                walk(ch, src, owner.as_deref(), func, imports, bindings, f);
                continue;
            }
            "function_item" => {
                if let Some(name) = ch.child_by_field_name("name") {
                    let nm = text(name, src).to_string();
                    f.symbols.push(RawSymbol {
                        name: nm.clone(),
                        parent: class.map(String::from),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Function,
                    });
                    let inner = local_bindings(ch, src);
                    walk(ch, src, class, Some(&nm), imports, &inner, f);
                    continue;
                }
            }
            "call_expression" => {
                if let Some(r) = route_from_call(ch, src) {
                    f.routes.push(r);
                }
                record_call(ch, src, class, func, imports, bindings, f);
            }
            _ => {}
        }
        walk(ch, src, class, func, imports, bindings, f);
    }
}

fn record_call(
    call: TsNode,
    src: &str,
    class: Option<&str>,
    func: Option<&str>,
    imports: &HashSet<String>,
    bindings: &HashMap<String, String>,
    f: &mut FileFacts,
) {
    let Some(fun) = call.child_by_field_name("function") else {
        return;
    };
    let owner = func.unwrap_or("<module>").to_string();
    match fun.kind() {
        "identifier" => f.calls.push(RawCall {
            from_symbol: owner,
            callee: text(fun, src).to_string(),
            line: call.start_position().row as u32 + 1,
            col: call.start_position().column as u32 + 1,
            resolvable: true,
        }),
        // `self.finish()` and `x.method()` both parse as field_expression.
        "field_expression" => {
            let Some(field) = fun.child_by_field_name("field") else {
                return;
            };
            let attr = text(field, src).to_string();
            let recv = fun.child_by_field_name("value");
            let mut callee = attr.clone();
            let resolvable = match recv {
                Some(r) if r.kind() == "self" => {
                    if let Some(c) = class {
                        callee = format!("{c}.{attr}");
                    }
                    true
                }
                Some(r) if r.kind() == "identifier" => {
                    let t = text(r, src);
                    if let Some(ty) = bindings.get(t) {
                        callee = format!("{ty}.{attr}");
                        true
                    } else {
                        imports.contains(t)
                    }
                }
                _ => false,
            };
            f.calls.push(RawCall {
                from_symbol: owner,
                line: call.start_position().row as u32 + 1,
                col: call.start_position().column as u32 + 1,
                callee,
                resolvable,
            });
        }
        // `String::new()` / `Router::new()`
        "scoped_identifier" => {
            let kids = named_children(fun);
            if let (Some(ty), Some(m)) = (kids.first(), kids.get(1)) {
                f.calls.push(RawCall {
                    from_symbol: owner,
                    callee: format!("{}.{}", text(*ty, src), text(*m, src)),
                    line: call.start_position().row as u32 + 1,
                    col: call.start_position().column as u32 + 1,
                    resolvable: true,
                });
            }
        }
        _ => {}
    }
}

/// axum: `.route("/path", post(handler))`
fn route_from_call(call: TsNode, src: &str) -> Option<RawRoute> {
    let fun = call.child_by_field_name("function")?;
    if fun.kind() != "field_expression" {
        return None;
    }
    if text(fun.child_by_field_name("field")?, src) != "route" {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let kids = named_children(args);
    let path = string_value(*kids.first()?, src)?;
    let verb = kids.get(1)?;
    if verb.kind() != "call_expression" {
        return None;
    }
    let vf = verb.child_by_field_name("function")?;
    let m = text(vf, src).rsplit("::").next()?.to_lowercase();
    if !METHODS.contains(&m.as_str()) {
        return None;
    }
    let handler = named_children(verb.child_by_field_name("arguments")?)
        .first()
        .map(|h| text(*h, src).to_string())?;
    Some(RawRoute {
        method: m.to_uppercase(),
        path,
        handler,
        line_start: call.start_position().row as u32 + 1,
        line_end: call.end_position().row as u32 + 1,
    })
}
