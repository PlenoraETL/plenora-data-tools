//! O-M3: censimento sintattico riproducibile, non certificato di raggiungibilita'.
//! Il parser visita anche closure e condizioni; macro e chiamate opache sono
//! registrate separatamente, mai considerate automaticamente lavoro costante.
#![deny(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

fn instrumented_file(path: &str) -> bool {
    let Some(relative) = path.strip_prefix("crates/plenora-kernels-geo/src/") else {
        return false;
    };
    [
        "coordinate_",
        "grid_text",
        "geodesic_",
        "geodetica",
        "noding",
        "offset",
        "predicates",
        "preserve_shape_measure",
        "reshape",
        "trim_extend",
        "extended_algorithms/polygonize_limited",
        "extended_algorithms/split_limited",
    ]
    .iter()
    .any(|prefix| relative.starts_with(prefix))
}

fn counter(body: &syn::Block) -> Option<String> {
    let syn::Stmt::Expr(syn::Expr::Call(call), _) = body.stmts.first()? else {
        return None;
    };
    let syn::Expr::Path(path) = call.func.as_ref() else {
        return None;
    };
    if path
        .path
        .segments
        .iter()
        .map(|part| part.ident.to_string())
        .collect::<Vec<_>>()
        != ["crate", "noding", "gop028", "cycle"]
        || call.attrs.len() != 1
        || !call.attrs[0].path().is_ident("cfg")
        || !test_only(&call.attrs)
        || call.args.len() != 1
    {
        return None;
    }
    let syn::Expr::Lit(literal) = call.args.first()? else {
        return None;
    };
    let syn::Lit::Str(value) = &literal.lit else {
        return None;
    };
    Some(value.value())
}

fn test_only(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path().is_ident("test")
            || (attr.path().is_ident("cfg")
                && attr
                    .meta
                    .require_list()
                    .is_ok_and(|list| list.tokens.to_string() == "test"))
    })
}

struct Census {
    root: PathBuf,
    file: PathBuf,
    directory: PathBuf,
    function: String,
    constant: bool,
    records: Vec<serde_json::Value>,
    counters: std::collections::BTreeSet<String>,
}

impl Census {
    fn file(&mut self, path: &Path, directory: &Path) {
        let previous_file = std::mem::replace(&mut self.file, path.to_owned());
        let previous_directory = std::mem::replace(&mut self.directory, directory.to_owned());
        let text = std::fs::read_to_string(path).expect("sorgente censito");
        let ast = syn::parse_file(&text).expect("AST Rust valido");
        self.visit_file(&ast);
        self.file = previous_file;
        self.directory = previous_directory;
    }

    fn record(&mut self, kind: &str, span: proc_macro2::Span, body: Option<&syn::Block>) {
        let start = span.start();
        let file = self
            .file
            .strip_prefix(&self.root)
            .expect("nel workspace")
            .to_string_lossy()
            .replace('\\', "/");
        let counter = body.and_then(counter);
        if body.is_some() && !self.constant && instrumented_file(&file) {
            let site = counter
                .as_ref()
                .unwrap_or_else(|| panic!("ciclo privo di contatore: {file}:{}", start.line));
            assert!(
                self.counters.insert(site.clone()),
                "contatore duplicato: {site}"
            );
        }
        self.records.push(serde_json::json!({
            "file": file,
            "function": if self.function.is_empty() { "<module>" } else { &self.function },
            "constant": self.constant,
            "kind": kind,
            "line": start.line,
            "column": start.column,
            "counter": counter,
            "body": body.map(|block| {
                let point = block.brace_token.span.open().end();
                [point.line, point.column]
            }),
        }));
    }
}

impl<'ast> Visit<'ast> for Census {
    fn visit_item_trait(&mut self, node: &'ast syn::ItemTrait) {
        if !test_only(&node.attrs) {
            visit::visit_item_trait(self, node);
        }
    }
    fn visit_expr_closure(&mut self, node: &'ast syn::ExprClosure) {
        let constant = std::mem::replace(&mut self.constant, false);
        visit::visit_expr_closure(self, node);
        self.constant = constant;
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if !test_only(&node.attrs) {
            visit::visit_item_impl(self, node);
        }
    }
    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, format!("trait::{}", node.sig.ident));
        visit::visit_trait_item_fn(self, node);
        self.function = previous;
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, format!("const::{}", node.ident));
        let constant = std::mem::replace(&mut self.constant, true);
        visit::visit_item_const(self, node);
        self.function = previous;
        self.constant = constant;
    }
    fn visit_item_static(&mut self, node: &'ast syn::ItemStatic) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, format!("static::{}", node.ident));
        let constant = std::mem::replace(&mut self.constant, true);
        visit::visit_item_static(self, node);
        self.function = previous;
        self.constant = constant;
    }
    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        if !test_only(&node.attrs) {
            visit::visit_item_macro(self, node);
        }
    }
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if test_only(&node.attrs) {
            return;
        }
        if let Some((_, items)) = &node.content {
            let previous = self.directory.clone();
            self.directory.push(node.ident.to_string());
            for item in items {
                self.visit_item(item);
            }
            self.directory = previous;
        } else {
            let name = node.ident.to_string();
            let explicit = node.attrs.iter().find_map(|attr| {
                if attr.path().is_ident("path") {
                    if let syn::Meta::NameValue(value) = &attr.meta {
                        if let syn::Expr::Lit(literal) = &value.value {
                            if let syn::Lit::Str(path) = &literal.lit {
                                return Some(path.value());
                            }
                        }
                    }
                }
                None
            });
            let path = explicit.map_or_else(
                || {
                    let path = self.directory.join(format!("{name}.rs"));
                    if path.exists() {
                        path
                    } else {
                        self.directory.join(&name).join("mod.rs")
                    }
                },
                |path| self.directory.join(path),
            );
            let directory = if path.file_name().is_some_and(|name| name == "mod.rs") {
                path.parent().expect("directory modulo").to_owned()
            } else {
                path.with_extension("")
            };
            self.file(&path, &directory);
        }
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, node.sig.ident.to_string());
        let constant = std::mem::replace(&mut self.constant, node.sig.constness.is_some());
        visit::visit_item_fn(self, node);
        self.function = previous;
        self.constant = constant;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, node.sig.ident.to_string());
        let constant = std::mem::replace(&mut self.constant, node.sig.constness.is_some());
        visit::visit_impl_item_fn(self, node);
        self.function = previous;
        self.constant = constant;
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.record("for", node.span(), Some(&node.body));
        visit::visit_expr_for_loop(self, node);
    }
    fn visit_expr_while(&mut self, node: &'ast syn::ExprWhile) {
        self.record("while", node.span(), Some(&node.body));
        visit::visit_expr_while(self, node);
    }
    fn visit_expr_loop(&mut self, node: &'ast syn::ExprLoop) {
        self.record("loop", node.span(), Some(&node.body));
        visit::visit_expr_loop(self, node);
    }
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        if [
            "map",
            "flat_map",
            "flatten",
            "filter",
            "filter_map",
            "find",
            "find_map",
            "position",
            "any",
            "all",
            "fold",
            "try_fold",
            "for_each",
            "try_for_each",
            "collect",
            "sum",
            "count",
            "extend",
            "retain",
            "sort",
            "sort_by",
            "sort_unstable",
            "sort_unstable_by",
            "clone",
            "to_vec",
            "resize",
            "truncate",
            "clear",
        ]
        .contains(&name.as_str())
        {
            self.record(&format!("method:{name}"), node.method.span(), None);
        }
        visit::visit_expr_method_call(self, node);
    }
    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if let syn::Expr::Path(path) = node.func.as_ref() {
            let mut segments = path.path.segments.iter().rev();
            if segments
                .next()
                .is_some_and(|segment| segment.ident == "new")
                && segments
                    .next()
                    .is_some_and(|segment| segment.ident == "Polygon")
            {
                self.record("polygon-constructor", node.span(), None);
            }
        }
        visit::visit_expr_call(self, node);
    }
    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        self.record("macro", node.span(), None);
        self.records.last_mut().unwrap()["tokens"] = node.tokens.to_string().into();
        self.records.last_mut().unwrap()["path"] = node
            .path
            .segments
            .iter()
            .map(|part| part.ident.to_string())
            .collect::<Vec<_>>()
            .join("::")
            .into();
        visit::visit_macro(self, node);
    }
}

#[test]
fn checkpoint620_census() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_owned();
    let mut census = Census {
        root: root.clone(),
        file: PathBuf::new(),
        directory: PathBuf::new(),
        function: String::new(),
        constant: false,
        records: Vec::new(),
        counters: std::collections::BTreeSet::new(),
    };
    for name in ["plenora-kernels-geo", "plenora-pipeline"] {
        let directory = root.join("crates").join(name).join("src");
        census.file(&directory.join("lib.rs"), &directory);
    }
    let actual = serde_json::to_string_pretty(&census.records).unwrap() + "\n";
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/checkpoint620-cycles.json");
    if std::env::var("PLENORA_RIGENERA_CICLI").as_deref() == Ok("1") {
        std::fs::write(&fixture, &actual).unwrap();
    }
    assert_eq!(
        actual,
        std::fs::read_to_string(fixture).expect("registro dei cicli")
    );
}

/// Una sonda tardiva, estranea o attiva in produzione non copre il ciclo.
#[test]
fn checkpoint620_counter_requires_first_test_statement() {
    for source in [
        "{}",
        "{ if skip { continue; } #[cfg(test)] crate::noding::gop028::cycle(\"a\"); }",
        "{ crate::noding::gop028::cycle(\"a\"); }",
        "{ #[cfg(test)] another::cycle(\"a\"); }",
        "{ #[cfg(test)] #[cfg(any())] crate::noding::gop028::cycle(\"a\"); }",
        "{ #[test] crate::noding::gop028::cycle(\"a\"); }",
    ] {
        assert_eq!(counter(&syn::parse_str(source).unwrap()), None);
    }
    let body = syn::parse_str(
        "{ #[cfg(test)] crate::noding::gop028::cycle(\"a\"); if skip { continue; } }",
    )
    .unwrap();
    assert_eq!(counter(&body).as_deref(), Some("a"));
}

#[test]
fn checkpoint620_ast_finds_nested_and_conditional_loops() {
    let mut census = Census {
        root: PathBuf::from("root"),
        file: PathBuf::from("root/fixture.rs"),
        directory: PathBuf::from("root"),
        function: String::new(),
        constant: false,
        records: Vec::new(),
        counters: std::collections::BTreeSet::new(),
    };
    let ast = syn::parse_file(
        r#"
        fn sample() {
            let text = "for while loop";
            // loop {}
            for x in (0..10).map(|x| { while test(x) { step(); } x }) {
                loop { if stop() { break; } }
            }
            opaque! { for x in hidden { step(); } }
        }
        #[cfg(test)] mod tests { fn ignored() { loop {} } }
    "#,
    )
    .unwrap();
    census.visit_file(&ast);
    let kinds: Vec<_> = census
        .records
        .iter()
        .map(|record| record["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["for", "method:map", "while", "loop", "macro"]);
}

#[test]
fn checkpoint620_ast_keeps_module_macros_and_default_methods() {
    let mut census = Census {
        root: PathBuf::from("root"),
        file: PathBuf::from("root/fixture.rs"),
        directory: PathBuf::from("root"),
        function: String::new(),
        constant: false,
        records: Vec::new(),
        counters: std::collections::BTreeSet::new(),
    };
    let ast = syn::parse_file(
        r"
        macro_rules! generate { () => { fn generated() { loop {} } } }
        generate!();
        trait Example { fn default_method() { loop { break; } } }
        const VALUE: u8 = { let mut x = 0; while x < 2 { x += 1; } x };
        static STATIC: u8 = { loop { break 1; } };
        static CALLBACK: fn() = || { loop { break; } };
        #[cfg(test)] trait OnlyTest { fn omitted() { loop {} } }
    ",
    )
    .unwrap();
    census.visit_file(&ast);
    let kinds: Vec<_> = census
        .records
        .iter()
        .map(|record| record["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["macro", "macro", "loop", "while", "loop", "loop"]);
    assert_eq!(census.records[2]["function"], "trait::default_method");
    assert_eq!(census.records[3]["constant"], true);
    assert_eq!(census.records[4]["constant"], true);
    assert_eq!(census.records[5]["constant"], false);
    assert_eq!(census.records[1]["path"], "generate");
}
