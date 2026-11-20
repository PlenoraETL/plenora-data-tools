//! O-M3: censimento sintattico riproducibile, non certificato di raggiungibilita'.
//! Il parser visita anche closure e condizioni; macro e chiamate opache sono
//! registrate separatamente, mai considerate automaticamente lavoro costante.
#![deny(clippy::disallowed_methods)]

use std::path::{Path, PathBuf};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};

#[path = "checkpoint620_census/static_proofs.rs"]
mod static_proofs;

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

/// Limite delle entrate nel corpo, non del lavoro dei suoi cicli annidati.
/// Solo estremi letterali: nessuna deduzione da nomi, misure o tipi esterni.
fn literal_range_bound(expression: &syn::Expr) -> Option<u128> {
    let syn::Expr::Range(range) = expression else {
        return None;
    };
    let value = |expression: &syn::Expr| {
        let syn::Expr::Lit(literal) = expression else {
            return None;
        };
        let syn::Lit::Int(integer) = &literal.lit else {
            return None;
        };
        let value = integer.base10_parse::<u128>().ok()?;
        // Senza risoluzione dei tipi, 0..=127 e' rappresentabile in ogni
        // intero Rust. Oltre, un allow(overflowing_literals) potrebbe
        // troncare anche un letterale senza suffisso per inferenza.
        (value <= i8::MAX as u128).then_some(value)
    };
    let start = value(range.start.as_deref()?)?;
    let end = value(range.end.as_deref()?)?;
    if start > end {
        return Some(0);
    }
    let difference = end.checked_sub(start)?;
    match range.limits {
        syn::RangeLimits::HalfOpen(_) => Some(difference),
        syn::RangeLimits::Closed(_) => difference.checked_add(1),
    }
}

// Lotti chiusi: ogni ciclo deve avere una prova locale, anche rigenerando
// il censimento. La chiusura locale NON e' il maggiorante composto di un'API.
const CLOSED_MODULES: &[&str] = &[
    "analyze/helpers.rs",
    "analyze/measures.rs",
    "coordinate_api.rs",
    "coordinate_utm.rs",
    "predicates.rs",
    "geodesic_buffer/point.rs",
    "noding.rs",
    "geodesic_buffer.rs",
    "preserve_shape_measure.rs",
    "geodesic_densify/validation.rs",
    "offset.rs",
    "reshape.rs",
    "trim_extend.rs",
    "extended_algorithms/polygonize_limited.rs",
    "extended_algorithms/split_limited.rs",
    "geodesic_buffer/projected/erosion.rs",
    "geodesic_buffer/projected/surface.rs",
    "geodesic_buffer/projected.rs",
    "predicates/interval_index.rs",
    "noding/faces.rs",
    "predicates/arrangement_indexed.rs",
    "predicates/arrangement_indexed/point_boolean.rs",
    "predicates/arrangement_indexed/general_boolean.rs",
    "predicates/arrangement_indexed/line_boolean.rs",
    "predicates/arrangement_indexed/polygon_boolean.rs",
    "predicates/arrangement_indexed/polygon_completion.rs",
    "coordinate_mgrs.rs",
    "geodesic_buffer/point/union.rs",
    "geodesic_densify.rs",
    "geodesic_densify/geometry.rs",
    "geodesic_surface.rs",
    "geodesic_surface/metric.rs",
    "noding/storage.rs",
    "offset/enclosure.rs",
    "preserve_shape_measure/certificate.rs",
    "preserve_shape_measure/certificate/azimuthal.rs",
    "preserve_shape_measure/certificate/lambert.rs",
    "preserve_shape_measure/certificate/transverse.rs",
];

/// Percorsi letti fino a `noding::check_interruption`: `DensifyWork`,
/// `geodesic_surface::metric::Work` e `noding::Work`. Non basta il nome step
/// in un modulo arbitrario per considerarlo un checkpoint condiviso.
fn checkpoint_method(file: &str) -> Option<&'static str> {
    let relative = file.strip_prefix("crates/plenora-kernels-geo/src/")?;
    match relative {
        "geodesic_densify.rs" | "geodesic_densify/geometry.rs" | "preserve_shape_measure.rs" => {
            Some("checkpoint")
        }
        "noding/exact.rs"
        | "geodesic_buffer/point/union.rs"
        | "noding/storage.rs"
        | "offset/enclosure.rs" => Some("step"),
        "geodesic_buffer/point.rs"
        | "noding.rs"
        | "geodesic_buffer.rs"
        | "geodesic_densify/validation.rs"
        | "geodesic_surface.rs"
        | "geodesic_surface/metric.rs"
        | "offset.rs"
        | "reshape.rs"
        | "trim_extend.rs"
        | "extended_algorithms/polygonize_limited.rs"
        | "extended_algorithms/split_limited.rs"
        | "geodesic_buffer/projected/erosion.rs"
        | "geodesic_buffer/projected/surface.rs"
        | "geodesic_buffer/projected.rs"
        | "predicates/interval_index.rs"
        | "noding/faces.rs"
        | "predicates/arrangement_indexed.rs"
        | "predicates/arrangement_indexed/point_boolean.rs"
        | "predicates/arrangement_indexed/general_boolean.rs"
        | "predicates/arrangement_indexed/line_boolean.rs"
        | "predicates/arrangement_indexed/polygon_boolean.rs"
        | "predicates/arrangement_indexed/polygon_completion.rs" => Some("metric"),
        _ => None,
    }
}

fn entry_checkpoint(file: &str, body: &syn::Block) -> Option<String> {
    let offset = usize::from(counter(body).is_some());
    let offset = if file == "crates/plenora-kernels-geo/src/coordinate_api.rs"
        && body.stmts.get(offset).is_some_and(|stmt| {
            matches!(stmt, syn::Stmt::Expr(syn::Expr::Call(call), Some(_))
                if call.args.is_empty() && call.attrs.len() == 1 && test_only(&call.attrs)
                && matches!(call.func.as_ref(), syn::Expr::Path(path)
                    if path.path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>()
                        == ["crate", "noding", "gop028", "unit"]))
        }) {
        offset + 1
    } else {
        offset
    };
    let syn::Stmt::Expr(syn::Expr::Try(attempt), Some(_)) = body.stmts.get(offset)? else {
        return None;
    };
    if file == "crates/plenora-kernels-geo/src/geodesic_buffer/point.rs" {
        if let syn::Expr::Call(call) = attempt.expr.as_ref() {
            if let syn::Expr::Path(path) = call.func.as_ref() {
                if path.path.is_ident("checkpoint")
                    && path.attrs.is_empty()
                    && call.args.is_empty()
                    && call.attrs.is_empty()
                    && attempt.attrs.is_empty()
                {
                    return Some("checkpoint()?".into());
                }
            }
        }
    }
    if file == "crates/plenora-kernels-geo/src/coordinate_api.rs" {
        if let syn::Expr::Call(call) = attempt.expr.as_ref() {
            if matches!(call.func.as_ref(), syn::Expr::Path(path) if path.path.is_ident("checkpoint") && path.attrs.is_empty())
                && call.args.len() == 1
                && matches!(call.args.first(), Some(syn::Expr::Path(path)) if path.path.is_ident("work") && path.attrs.is_empty())
                && call.attrs.is_empty()
                && attempt.attrs.is_empty()
            {
                return Some("checkpoint(work)?".into());
            }
        }
    }
    let syn::Expr::MethodCall(call) = attempt.expr.as_ref() else {
        return None;
    };
    let syn::Expr::Path(receiver) = call.receiver.as_ref() else {
        return None;
    };
    if file == "crates/plenora-pipeline/src/esecuzione.rs"
        && receiver.path.is_ident("interruzione")
        && call.method == "verifica"
        && call.args.len() == 1
        && call.turbofish.is_none()
        && receiver.attrs.is_empty()
        && call.attrs.is_empty()
        && attempt.attrs.is_empty()
        && matches!(call.args.first(), Some(syn::Expr::Lit(lit))
            if lit.attrs.is_empty() && matches!(&lit.lit, syn::Lit::Str(value) if value.value() == "esecuzione pipeline"))
    {
        return Some("interruzione.verifica(\"esecuzione pipeline\")?".into());
    }
    let method = checkpoint_method(file)?;
    let allowed = call.method == method
        || (file == "crates/plenora-kernels-geo/src/preserve_shape_measure.rs"
            && call.method == "charge")
        || (method == "metric" && (call.method == "step" || call.method == "checkpoint"));
    (allowed
        && (receiver.path.is_ident("work")
            || (file == "crates/plenora-kernels-geo/src/geodesic_buffer.rs"
                && receiver.path.is_ident("self")))
        && receiver.attrs.is_empty()
        && call.attrs.is_empty()
        && attempt.attrs.is_empty()
        && call.args.is_empty()
        && call.turbofish.is_none())
    .then(|| format!("{}.{}()?", receiver.path.segments[0].ident, call.method))
}

fn array_bound(expression: &syn::Expr) -> Option<usize> {
    let syn::Expr::Array(array) = expression else {
        return None;
    };
    // Un attributo cfg su un elemento puo' solo ridurre la cardinalita'.
    // Le espressioni degli elementi sono visitate separatamente dal censimento.
    Some(array.elems.len())
}

fn locally_covered(record: &serde_json::Value) -> bool {
    !record["entry_checkpoint"].is_null()
        || !record["literal_iteration_bound"].is_null()
        || !record["array_iteration_bound"].is_null()
        || !record["registered_iteration_bound"].is_null()
}

fn verify_closed_modules(records: &[serde_json::Value]) {
    for record in records.iter().filter(|record| !record["body"].is_null()) {
        let file = record["file"].as_str().unwrap();
        let relative = file.strip_prefix("crates/plenora-kernels-geo/src/");
        if relative.is_some_and(|relative| CLOSED_MODULES.contains(&relative))
            || file == "crates/plenora-pipeline/src/esecuzione.rs"
        {
            assert!(
                locally_covered(record),
                "ciclo scoperto nel lotto chiuso: {file}:{}",
                record["line"]
            );
        }
    }
}

fn uncovered_cycles(records: &[serde_json::Value]) -> Vec<serde_json::Value> {
    records
        .iter()
        .filter(|record| !record["body"].is_null() && !locally_covered(record))
        .map(|record| {
            serde_json::json!({
                "file": record["file"],
                "function": record["function"],
                "source": record["source"],
            })
        })
        .collect()
}

fn verify_open_cycles(records: &[serde_json::Value], pending: &[serde_json::Value]) {
    // Multinsieme con sorgente esatto, non un nome/numero di riga o un hash.
    // Anche duplicare un ciclo aperto richiede una nuova voce esplicita.
    let key = |record: &serde_json::Value| {
        serde_json::json!([record["file"], record["function"], record["source"]]).to_string()
    };
    let mut actual: Vec<_> = uncovered_cycles(records).iter().map(key).collect();
    let mut expected: Vec<_> = pending.iter().map(key).collect();
    actual.sort();
    expected.sort();
    assert!(actual == expected, "debito cicli modificato: registrare la prova o aggiornare esplicitamente il debito; PLENORA_RIGENERA_CICLI non lo modifica");
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
            "entry_checkpoint": body.and_then(|body| entry_checkpoint(&file, body)),
            // Conserva anche condizioni, iteratore e corpo: togliere uno step
            // senza cambiare le posizioni deve comunque invalidare il registro.
            "source": body.map(|_| span.source_text().expect("sorgente del ciclo AST").replace("\r\n", "\n")),
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
        let constant = std::mem::replace(&mut self.constant, false);
        visit::visit_trait_item_fn(self, node);
        self.function = previous;
        self.constant = constant;
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
        // Una const fn puo' essere chiamata anche a runtime. Solo gli
        // inizializzatori const/static sono esclusi dall'esecuzione runtime.
        let constant = std::mem::replace(&mut self.constant, false);
        visit::visit_item_fn(self, node);
        self.function = previous;
        self.constant = constant;
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        let previous = std::mem::replace(&mut self.function, node.sig.ident.to_string());
        let constant = std::mem::replace(&mut self.constant, false);
        visit::visit_impl_item_fn(self, node);
        self.function = previous;
        self.constant = constant;
    }

    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        self.record("for", node.span(), Some(&node.body));
        self.records.last_mut().unwrap()["literal_iteration_bound"] =
            literal_range_bound(&node.expr)
                .map(|bound| bound.to_string())
                .into();
        self.records.last_mut().unwrap()["array_iteration_bound"] = array_bound(&node.expr)
            .map(|bound| bound.to_string())
            .into();
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
            "inverse",
            "direct",
            "par_iter",
            "into_par_iter",
            "to_wkb",
            "to_wkt",
            "sqrt",
            "pow",
            "modpow",
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
    static_proofs::register(&root, &mut census.records);
    let actual = serde_json::to_string_pretty(&census.records).unwrap() + "\n";
    verify_closed_modules(&census.records);
    let pending: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("fixtures/checkpoint620-open-cycles.json"))
            .expect("debito esplicito dei cicli, non rigenerato dal censimento");
    verify_open_cycles(&census.records, &pending);
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

/// Il registro protegge il codice del ciclo, non solo la sua posizione.
#[test]
fn checkpoint620_ast_keeps_runtime_const_functions_and_loop_source() {
    let scan = |source: &str| {
        let mut census = Census {
            root: PathBuf::from("root"),
            file: PathBuf::from("root/fixture.rs"),
            directory: PathBuf::from("root"),
            function: String::new(),
            constant: false,
            records: Vec::new(),
            counters: std::collections::BTreeSet::new(),
        };
        census.visit_file(&syn::parse_file(source).unwrap());
        census.records
    };
    let before = scan("const fn f() { for x in 0..4 { step(); } }");
    let after = scan("const fn f() { for x in 0..4 { skip(); } }");
    assert_eq!(before[0]["constant"], false);
    assert_eq!(before[0]["line"], after[0]["line"]);
    assert_eq!(before[0]["column"], after[0]["column"]);
    assert_eq!(before[0]["body"], after[0]["body"]);
    assert_eq!(before[0]["source"], "for x in 0..4 { step(); }");
    assert_ne!(before, after);
    let implementation = scan("impl T { const fn f() { loop { break; } } }");
    assert_eq!(implementation[0]["constant"], false);
    let nested_trait = scan(
        "static F: fn() = { trait T { fn f() { loop { break; } } } struct S; impl T for S {} S::f };",
    );
    assert_eq!(nested_trait[0]["constant"], false);
    assert_eq!(
        scan("fn f() { for x in 0..4 {\r\n step();\r\n} }"),
        scan("fn f() { for x in 0..4 {\n step();\n} }")
    );
}

#[test]
fn checkpoint620_literal_bounds_are_structural() {
    for (source, expected) in [
        ("0..40", Some(40)),
        ("1..=40", Some(40)),
        ("4..4", Some(0)),
        ("4..=4", Some(1)),
        ("5..4", Some(0)),
        ("5..=4", Some(0)),
        ("0..0x10", Some(16)),
        ("0..limit", None),
        ("0..compute()", None),
        ("0..", None),
        ("..40", None),
        ("items.iter()", None),
        ("256u8..255u8", None),
        ("256..255", None),
        ("0..=340282366920938463463374607431768211455u128", None),
    ] {
        assert_eq!(
            literal_range_bound(&syn::parse_str(source).unwrap()),
            expected
        );
    }
}

#[test]
fn checkpoint620_entry_requires_unconditional_shared_call() {
    for module in CLOSED_MODULES {
        let file = format!("crates/plenora-kernels-geo/src/{module}");
        if checkpoint_method(&file) != Some("metric") {
            continue;
        }
        for source in [
            "{ work.step()?; }",
            "{ work.checkpoint()?; }",
            "{ #[cfg(test)] crate::noding::gop028::cycle(\"a\"); work.step()?; }",
        ] {
            let body = syn::parse_str(source).unwrap();
            assert!(entry_checkpoint(&file, &body).is_some());
            assert!(entry_checkpoint("unknown.rs", &body).is_none());
        }
        for source in [
            "{ if skip { continue; } work.step()?; }",
            "{ #[cfg(test)] work.step()?; }",
            "{ #[cfg(feature = \"optional\")] work.step()?; }",
            "{ if condition { work.step()?; } }",
            "{ let _ = work.step(); }",
            "{ work.step(); }",
            "{ other.step()?; }",
            "{ work.step(1)?; }",
            "{ work.step::<Other>()?; }",
            "{ loop {} work.step()?; }",
        ] {
            assert!(entry_checkpoint(&file, &syn::parse_str(source).unwrap()).is_none());
        }
    }
}

#[test]
fn checkpoint620_new_cycles_cannot_hide_in_regeneration() {
    let pending = serde_json::json!({"file": "open.rs", "function": "f", "source": "loop {}"});
    let record = serde_json::json!({"file": "open.rs", "function": "f", "source": "loop {}", "body": [1, 1]});
    verify_open_cycles(
        std::slice::from_ref(&record),
        std::slice::from_ref(&pending),
    );
    // Copia identica, ciclo nuovo e corpo modificato: tutti debiti nuovi.
    for records in [
        vec![record.clone(), record.clone()],
        vec![{
            let mut changed = record;
            changed["source"] = "for x in input {}".into();
            changed
        }],
    ] {
        let failure = std::panic::catch_unwind(|| {
            verify_open_cycles(&records, std::slice::from_ref(&pending));
        })
        .expect_err("il gate deve rifiutare il nuovo debito");
        assert_eq!(failure.downcast_ref::<&str>().copied(), Some("debito cicli modificato: registrare la prova o aggiornare esplicitamente il debito; PLENORA_RIGENERA_CICLI non lo modifica"));
    }
    let closed = serde_json::json!({
        "file": "crates/plenora-kernels-geo/src/geodesic_surface.rs",
        "body": [1, 1], "line": 1,
    });
    // Il debito non puo' riaprire un modulo chiuso.
    let failure = std::panic::catch_unwind(|| verify_closed_modules(&[closed]))
        .expect_err("il gate deve rifiutare la riapertura");
    assert_eq!(
        failure.downcast_ref::<String>().map(String::as_str),
        Some(
            "ciclo scoperto nel lotto chiuso: crates/plenora-kernels-geo/src/geodesic_surface.rs:1"
        )
    );
    for (expression, expected) in [
        ("[]", Some(0)),
        ("[a, b, c]", Some(3)),
        ("[a; n]", None),
        ("items", None),
    ] {
        assert_eq!(array_bound(&syn::parse_str(expression).unwrap()), expected);
    }
}

#[test]
fn checkpoint620_callbacks_receivers_and_pipeline_require_propagation() {
    for (file, call) in [
        (
            "crates/plenora-kernels-geo/src/coordinate_api.rs",
            "checkpoint(work)",
        ),
        (
            "crates/plenora-kernels-geo/src/noding/exact.rs",
            "work.step()",
        ),
        (
            "crates/plenora-kernels-geo/src/geodesic_buffer/point.rs",
            "checkpoint()",
        ),
        (
            "crates/plenora-kernels-geo/src/geodesic_buffer.rs",
            "self.checkpoint()",
        ),
        (
            "crates/plenora-kernels-geo/src/preserve_shape_measure.rs",
            "work.charge()",
        ),
        (
            "crates/plenora-kernels-geo/src/preserve_shape_measure.rs",
            "work.checkpoint()",
        ),
        (
            "crates/plenora-pipeline/src/esecuzione.rs",
            "interruzione.verifica(\"esecuzione pipeline\")",
        ),
    ] {
        let body = syn::parse_str(&format!("{{ {call}?; }}")).unwrap();
        assert!(entry_checkpoint(file, &body).is_some());
        assert!(entry_checkpoint("unknown.rs", &body).is_none());
        for source in [
            format!("{{ if skip {{ continue; }} {call}?; }}"),
            format!("{{ if condition {{ {call}?; }} }}"),
            format!("{{ #[cfg(test)] {call}?; }}"),
            format!("{{ #[cfg(feature = \"optional\")] {call}?; }}"),
            format!("{{ let _ = {call}; }}"),
            format!("{{ {call}; }}"),
            format!("{{ loop {{}} {call}?; }}"),
        ] {
            assert!(entry_checkpoint(file, &syn::parse_str(&source).unwrap()).is_none());
        }
    }
}

#[test]
fn checkpoint620_coordinate_test_unit_is_not_production_work() {
    let file = "crates/plenora-kernels-geo/src/coordinate_api.rs";
    let body = syn::parse_str("{ #[cfg(test)] crate::noding::gop028::unit(); checkpoint(work)?; }")
        .unwrap();
    assert!(entry_checkpoint(file, &body).is_some());
    for source in [
        "{ crate::noding::gop028::unit(); checkpoint(work)?; }",
        "{ #[cfg(feature = \"optional\")] crate::noding::gop028::unit(); checkpoint(work)?; }",
        "{ #[cfg(test)] other::unit(); checkpoint(work)?; }",
        "{ #[cfg(test)] crate::noding::gop028::unit(); checkpoint(other)?; }",
        "{ #[cfg(test)] crate::noding::gop028::unit(); checkpoint(work); }",
    ] {
        assert!(entry_checkpoint(file, &syn::parse_str(source).unwrap()).is_none());
    }
}
