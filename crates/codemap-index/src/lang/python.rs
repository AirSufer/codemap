use crate::extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
use codemap_graph::NodeKind;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node as TsNode, Parser};

pub struct PythonExtractor;

fn text<'a>(n: TsNode, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

fn child_name(n: TsNode, src: &str) -> Option<String> {
    n.child_by_field_name("name")
        .map(|c| text(c, src).to_string())
}

impl LanguageExtractor for PythonExtractor {
    fn extensions(&self) -> &'static [&'static str] {
        &["py"]
    }

    fn extract(&self, src: &str) -> FileFacts {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("python grammar");
        let Some(tree) = p.parse(src, None) else {
            return FileFacts::default();
        };
        let mut f = FileFacts::default();
        let mut imports: HashSet<String> = HashSet::new();
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
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        if matches!(ch.kind(), "import_from_statement" | "import_statement") {
            let mut cc = ch.walk();
            for d in ch.children(&mut cc) {
                if d.kind() == "dotted_name" {
                    if let Some(last) = d.child(d.child_count().saturating_sub(1)) {
                        out.insert(text(last, src).to_string());
                    }
                } else if d.kind() == "aliased_import" {
                    if let Some(a) = d.child_by_field_name("alias") {
                        out.insert(text(a, src).to_string());
                    }
                }
            }
        }
        collect_imports(ch, src, out);
    }
}

/// Maps local variable name -> class name for `x = ClassName()` assignments
/// inside one function body. Deliberately shallow: no reassignment tracking,
/// no branches, no attribute targets. Recovers part of the `<local var>.x()`
/// gap, which the spike measured at 17,136 of 23,848 unresolvable Python calls.
fn local_bindings(func: TsNode, src: &str) -> HashMap<String, String> {
    fn rec(n: TsNode, src: &str, out: &mut HashMap<String, String>) {
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            if ch.kind() == "assignment" {
                if let (Some(l), Some(r)) = (
                    ch.child_by_field_name("left"),
                    ch.child_by_field_name("right"),
                ) {
                    if l.kind() == "identifier" && r.kind() == "call" {
                        if let Some(f) = r.child_by_field_name("function") {
                            if f.kind() == "identifier" {
                                let cls = text(f, src);
                                if cls.chars().next().is_some_and(|c| c.is_uppercase()) {
                                    out.insert(text(l, src).to_string(), cls.to_string());
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
    rec(func, src, &mut out);
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
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        match ch.kind() {
            "class_definition" => {
                if let Some(name) = child_name(ch, src) {
                    f.containers.push(RawSymbol {
                        name: name.clone(),
                        parent: class.map(String::from),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Class,
                    });
                    walk(ch, src, Some(&name), None, imports, &HashMap::new(), f);
                    continue;
                }
            }
            "function_definition" => {
                if let Some(name) = child_name(ch, src) {
                    f.symbols.push(RawSymbol {
                        name: name.clone(),
                        parent: class.map(String::from),
                        line_start: ch.start_position().row as u32 + 1,
                        line_end: ch.end_position().row as u32 + 1,
                        kind: NodeKind::Function,
                    });
                    let inner = local_bindings(ch, src);
                    walk(ch, src, class, Some(&name), imports, &inner, f);
                    continue;
                }
            }
            "decorated_definition" => {
                if let Some(r) = route_from_decorated(ch, src) {
                    f.routes.push(r);
                }
                walk(ch, src, class, func, imports, bindings, f);
                continue;
            }
            "call" => {
                if let Some(fun) = ch.child_by_field_name("function") {
                    let owner = func.unwrap_or("<module>").to_string();
                    match fun.kind() {
                        "identifier" => f.calls.push(RawCall {
                            from_symbol: owner,
                            callee: text(fun, src).to_string(),
                            resolvable: true,
                        }),
                        "attribute" => {
                            let attr = fun
                                .child_by_field_name("attribute")
                                .map(|a| text(a, src).to_string())
                                .unwrap_or_default();
                            let recv = fun.child_by_field_name("object");
                            let mut callee = attr.clone();
                            let resolvable = match recv {
                                Some(r) if r.kind() == "identifier" => {
                                    let t = text(r, src);
                                    if let Some(cls) = bindings.get(t) {
                                        callee = format!("{cls}.{attr}");
                                        true
                                    } else if t == "self" || t == "cls" {
                                        // Qualify with the enclosing class so the
                                        // callee does not collide with every other
                                        // method of the same bare name in the repo.
                                        if let Some(c) = class {
                                            callee = format!("{c}.{attr}");
                                        }
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
            }
            _ => {}
        }
        walk(ch, src, class, func, imports, bindings, f);
    }
}

/// `@router.post("/x")` above a `def handler(...)`.
fn route_from_decorated(n: TsNode, src: &str) -> Option<RawRoute> {
    const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "websocket", "head"];
    let mut c = n.walk();
    let mut found: Option<(String, String)> = None;
    for ch in n.children(&mut c) {
        if ch.kind() != "decorator" {
            continue;
        }
        let Some(call) = ch.child(1) else { continue };
        if call.kind() != "call" {
            continue;
        }
        let Some(fun) = call.child_by_field_name("function") else {
            continue;
        };
        if fun.kind() != "attribute" {
            continue;
        }
        let Some(attr) = fun.child_by_field_name("attribute") else {
            continue;
        };
        let m = text(attr, src).to_lowercase();
        if !METHODS.contains(&m.as_str()) {
            continue;
        }
        let Some(args) = call.child_by_field_name("arguments") else {
            continue;
        };
        let mut ac = args.walk();
        let Some(path) = args
            .children(&mut ac)
            .find(|a| a.kind() == "string")
            .map(|a| text(a, src).trim_matches(['"', '\'']).to_string())
        else {
            continue;
        };
        found = Some((m.to_uppercase(), path));
    }
    let (method, path) = found?;
    let mut c2 = n.walk();
    let def = n
        .children(&mut c2)
        .find(|c| c.kind() == "function_definition")?;
    Some(RawRoute {
        method,
        path,
        handler: child_name(def, src)?,
        line_start: def.start_position().row as u32 + 1,
        line_end: def.end_position().row as u32 + 1,
    })
}
