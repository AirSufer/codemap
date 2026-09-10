use crate::extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
use codemap_graph::NodeKind;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node as TsNode, Parser};

pub struct GoExtractor;

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
    if !n.kind().contains("string_literal") {
        return None;
    }
    Some(text(n, src).trim_matches(['"', '`']).to_string())
}

/// `func (s *ChatService) Stream(...)` -> ("s", "ChatService")
fn receiver_of(m: TsNode, src: &str) -> Option<(String, String)> {
    let recv = m.child_by_field_name("receiver")?;
    let decl = named_children(recv).into_iter().next()?;
    let ident = decl.child_by_field_name("name")?;
    let mut ty = decl.child_by_field_name("type")?;
    if ty.kind() == "pointer_type" {
        ty = named_children(ty).into_iter().next()?;
    }
    Some((text(ident, src).to_string(), text(ty, src).to_string()))
}

impl LanguageExtractor for GoExtractor {
    fn extensions(&self) -> &'static [&'static str] {
        &["go"]
    }

    fn extract(&self, src: &str) -> FileFacts {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_go::LANGUAGE.into())
            .expect("go grammar");
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
            &imports,
            &HashMap::new(),
            &mut f,
        );
        f // tree dropped here — never retained
    }
}

fn collect_imports(n: TsNode, src: &str, out: &mut HashSet<String>) {
    for ch in named_children(n) {
        if ch.kind() == "import_spec" {
            if let Some(p) = named_children(ch).into_iter().next() {
                if let Some(v) = string_value(p, src) {
                    if let Some(last) = v.rsplit('/').next() {
                        out.insert(last.to_string());
                    }
                }
            }
        }
        collect_imports(ch, src, out);
    }
}

/// `svc := &ChatService{}` / `svc := ChatService{}` inside one function.
fn local_bindings(scope: TsNode, src: &str) -> HashMap<String, String> {
    fn rec(n: TsNode, src: &str, out: &mut HashMap<String, String>) {
        for ch in named_children(n) {
            if ch.kind() == "short_var_declaration" || ch.kind() == "assignment_statement" {
                let (Some(l), Some(r)) = (
                    ch.child_by_field_name("left"),
                    ch.child_by_field_name("right"),
                ) else {
                    rec(ch, src, out);
                    continue;
                };
                let lhs = named_children(l).into_iter().next();
                let mut rhs = named_children(r).into_iter().next();
                if let Some(x) = rhs {
                    if x.kind() == "unary_expression" {
                        rhs = named_children(x).into_iter().next();
                    }
                }
                if let (Some(lv), Some(rv)) = (lhs, rhs) {
                    if rv.kind() == "composite_literal" {
                        if let Some(ty) = rv.child_by_field_name("type") {
                            out.insert(text(lv, src).to_string(), text(ty, src).to_string());
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
    func: Option<&str>,
    imports: &HashSet<String>,
    bindings: &HashMap<String, String>,
    f: &mut FileFacts,
) {
    for ch in named_children(n) {
        match ch.kind() {
            "type_spec" => {
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
            "method_declaration" => {
                let recv = receiver_of(ch, src);
                let owner = recv.as_ref().map(|(_, t)| t.clone());
                if let Some(name) = ch.child_by_field_name("name") {
                    let nm = text(name, src).to_string();
                    f.symbols.push(RawSymbol {
                        name: nm.clone(),
                        parent: owner.clone(),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Function,
                    });
                    let mut inner = local_bindings(ch, src);
                    // the receiver identifier resolves to its own type
                    if let (Some((rid, rty)), Some(_)) = (recv, Some(())) {
                        inner.insert(rid, rty);
                    }
                    walk(ch, src, Some(&nm), imports, &inner, f);
                    continue;
                }
            }
            "function_declaration" => {
                if let Some(name) = ch.child_by_field_name("name") {
                    let nm = text(name, src).to_string();
                    f.symbols.push(RawSymbol {
                        name: nm.clone(),
                        parent: None,
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Function,
                    });
                    let inner = local_bindings(ch, src);
                    walk(ch, src, Some(&nm), imports, &inner, f);
                    continue;
                }
            }
            "call_expression" => {
                if let Some(r) = route_from_call(ch, src) {
                    f.routes.push(r);
                }
                record_call(ch, src, func, imports, bindings, f);
            }
            _ => {}
        }
        walk(ch, src, func, imports, bindings, f);
    }
}

fn record_call(
    call: TsNode,
    src: &str,
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
            resolvable: true,
        }),
        "selector_expression" => {
            let Some(field) = fun.child_by_field_name("field") else {
                return;
            };
            let attr = text(field, src).to_string();
            let recv = fun.child_by_field_name("operand");
            let mut callee = attr.clone();
            let resolvable = match recv {
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
                callee,
                resolvable,
            });
        }
        _ => {}
    }
}

/// `mux.HandleFunc("/path", h)` (no method -> ANY) and `r.Get("/path", h)`.
fn route_from_call(call: TsNode, src: &str) -> Option<RawRoute> {
    let fun = call.child_by_field_name("function")?;
    if fun.kind() != "selector_expression" {
        return None;
    }
    let raw = text(fun.child_by_field_name("field")?, src).to_string();
    let lower = raw.to_lowercase();
    let method = if lower == "handlefunc" || lower == "handle" {
        "ANY".to_string()
    } else if METHODS.contains(&lower.as_str()) {
        lower.to_uppercase()
    } else {
        return None;
    };
    let args = call.child_by_field_name("arguments")?;
    let kids = named_children(args);
    let path = string_value(*kids.first()?, src)?;
    let handler = kids.get(1).map(|h| {
        let t = text(*h, src);
        t.rsplit('.').next().unwrap_or(t).to_string()
    })?;
    Some(RawRoute {
        method,
        path,
        handler,
        line_start: call.start_position().row as u32 + 1,
        line_end: call.end_position().row as u32 + 1,
    })
}
