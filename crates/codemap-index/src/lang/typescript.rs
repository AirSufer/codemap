use crate::extractor::{FileFacts, LanguageExtractor, RawCall, RawRoute, RawSymbol};
use codemap_graph::NodeKind;
use std::collections::{HashMap, HashSet};
use tree_sitter::{Node as TsNode, Parser};

pub struct TypeScriptExtractor;

const METHODS: [&str; 7] = ["get", "post", "put", "patch", "delete", "head", "all"];

fn text<'a>(n: TsNode, src: &'a str) -> &'a str {
    &src[n.byte_range()]
}

fn name_of(n: TsNode, src: &str) -> Option<String> {
    n.child_by_field_name("name")
        .map(|c| text(c, src).to_string())
}

/// `"/chat/stream"` -> `/chat/stream`. The `string` node keeps its quotes;
/// its `string_fragment` child holds the content.
fn string_value(n: TsNode, src: &str) -> Option<String> {
    if n.kind() != "string" {
        return None;
    }
    let frag = {
        let mut c = n.walk();
        let found = n
            .children(&mut c)
            .find(|ch| ch.kind() == "string_fragment")
            .map(|ch| text(ch, src).to_string());
        found
    };
    frag.or_else(|| Some(text(n, src).trim_matches(['"', '\'', '`']).to_string()))
}

impl LanguageExtractor for TypeScriptExtractor {
    fn extensions(&self) -> &'static [&'static str] {
        &["ts", "tsx", "js", "jsx", "mjs"]
    }

    fn extract(&self, src: &str) -> FileFacts {
        let mut p = Parser::new();
        p.set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("typescript grammar");
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
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        if ch.kind() == "import_statement" {
            let mut cc = ch.walk();
            for d in ch.children(&mut cc) {
                if d.kind() == "import_clause" {
                    collect_import_names(d, src, out);
                }
            }
        }
        collect_imports(ch, src, out);
    }
}

fn collect_import_names(n: TsNode, src: &str, out: &mut HashSet<String>) {
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        match ch.kind() {
            "identifier" => {
                out.insert(text(ch, src).to_string());
            }
            "named_imports" | "import_specifier" | "namespace_import" => {
                collect_import_names(ch, src, out)
            }
            _ => {}
        }
    }
}

/// `const x = new ClassName()` inside one function body -> {x: ClassName}.
/// Mirrors the Python local-binding heuristic (spec section 7).
fn local_bindings(scope: TsNode, src: &str) -> HashMap<String, String> {
    fn rec(n: TsNode, src: &str, out: &mut HashMap<String, String>) {
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            if ch.kind() == "variable_declarator" {
                if let (Some(l), Some(r)) = (
                    ch.child_by_field_name("name"),
                    ch.child_by_field_name("value"),
                ) {
                    if l.kind() == "identifier" && r.kind() == "new_expression" {
                        if let Some(ctor) = r.child_by_field_name("constructor") {
                            if ctor.kind() == "identifier" {
                                out.insert(text(l, src).to_string(), text(ctor, src).to_string());
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
    let mut c = n.walk();
    for ch in n.children(&mut c) {
        match ch.kind() {
            "class_declaration" | "abstract_class_declaration" => {
                if let Some(name) = name_of(ch, src) {
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
            "method_definition" | "function_declaration" | "function_expression" => {
                if let Some(name) = name_of(ch, src) {
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
            resolvable: true,
        }),
        "member_expression" => {
            let Some(prop) = fun.child_by_field_name("property") else {
                return;
            };
            let attr = text(prop, src).to_string();
            let recv = fun.child_by_field_name("object");
            let mut callee = attr.clone();
            let resolvable = match recv {
                Some(r) if r.kind() == "this" => {
                    if let Some(c) = class {
                        callee = format!("{c}.{attr}");
                    }
                    true
                }
                Some(r) if r.kind() == "identifier" => {
                    let t = text(r, src);
                    if let Some(cls) = bindings.get(t) {
                        callee = format!("{cls}.{attr}");
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

/// `app.post("/chat/stream", handler)` — Express, Koa, Fastify and Hono all
/// share this shape.
fn route_from_call(call: TsNode, src: &str) -> Option<RawRoute> {
    let fun = call.child_by_field_name("function")?;
    if fun.kind() != "member_expression" {
        return None;
    }
    let m = text(fun.child_by_field_name("property")?, src).to_lowercase();
    if !METHODS.contains(&m.as_str()) {
        return None;
    }
    let args = call.child_by_field_name("arguments")?;
    let kids: Vec<TsNode> = {
        let mut c = args.walk();
        let v: Vec<TsNode> = args.children(&mut c).filter(|k| k.is_named()).collect();
        v
    };
    let path = string_value(*kids.first()?, src)?;
    // Second argument is the handler: a named function, or a reference to one.
    let handler = kids.get(1).and_then(|h| match h.kind() {
        "function_expression" | "function_declaration" => name_of(*h, src),
        "identifier" => Some(text(*h, src).to_string()),
        "arrow_function" => Some(format!("<anonymous>@{}", h.start_position().row + 1)),
        _ => None,
    })?;
    Some(RawRoute {
        method: m.to_uppercase(),
        path,
        handler,
        line_start: call.start_position().row as u32 + 1,
        line_end: call.end_position().row as u32 + 1,
    })
}
