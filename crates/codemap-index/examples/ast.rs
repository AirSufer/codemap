use tree_sitter::Parser;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (lang_name, path) = (&args[1], &args[2]);
    let lang: tree_sitter::Language = match lang_name.as_str() {
        "ts" => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        "go" => tree_sitter_go::LANGUAGE.into(),
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        _ => tree_sitter_python::LANGUAGE.into(),
    };
    let src = std::fs::read_to_string(path).unwrap();
    let mut p = Parser::new();
    p.set_language(&lang).unwrap();
    let tree = p.parse(&src, None).unwrap();
    fn walk(n: tree_sitter::Node, src: &str, d: usize) {
        if n.is_named() {
            let t = &src[n.byte_range()];
            let t = t.lines().next().unwrap_or("");
            let t: String = t.chars().take(46).collect();
            let field = n.parent().and_then(|_| None::<&str>).unwrap_or("");
            println!("{}{}{}  |{}", "  ".repeat(d), n.kind(), field, t);
        }
        let mut c = n.walk();
        for ch in n.children(&mut c) {
            walk(ch, src, d + 1);
        }
    }
    walk(tree.root_node(), &src, 0);
}
