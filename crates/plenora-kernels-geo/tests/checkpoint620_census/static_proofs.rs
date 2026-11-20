//! Prove locali revisionate: dipendenze AST e sorgente del ciclo vincolati.
//! Il registro non viene mai aggiornato da `PLENORA_RIGENERA_CICLI`.
use super::*;

fn tokens(source: &str) -> String {
    source
        .parse::<proc_macro2::TokenStream>()
        .unwrap()
        .to_string()
}

#[derive(Default)]
struct Items(Vec<(&'static str, String)>);

impl Items {
    fn function(&mut self, signature: &syn::Signature, body: &syn::Block) {
        self.0.push((
            "function",
            tokens(&format!(
                "{} {}",
                signature.span().source_text().unwrap(),
                body.span().source_text().unwrap()
            )),
        ));
    }
}

impl<'ast> Visit<'ast> for Items {
    fn visit_item_mod(&mut self, node: &'ast syn::ItemMod) {
        if !test_only(&node.attrs) {
            visit::visit_item_mod(self, node);
        }
    }
    fn visit_item_impl(&mut self, node: &'ast syn::ItemImpl) {
        if !test_only(&node.attrs) {
            visit::visit_item_impl(self, node);
        }
    }
    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        self.function(&node.sig, &node.block);
        visit::visit_item_fn(self, node);
    }
    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        if test_only(&node.attrs) {
            return;
        }
        self.function(&node.sig, &node.block);
        visit::visit_impl_item_fn(self, node);
    }
    fn visit_item_const(&mut self, node: &'ast syn::ItemConst) {
        if test_only(&node.attrs) {
            return;
        }
        self.0.push((
            "const",
            tokens(&format!(
                "const {}: {} = {};",
                node.ident,
                node.ty.span().source_text().unwrap(),
                node.expr.span().source_text().unwrap()
            )),
        ));
        visit::visit_item_const(self, node);
    }
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        if !test_only(&node.attrs) {
            self.0
                .push(("use", tokens(&node.span().source_text().unwrap())));
        }
    }
    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if !test_only(&node.attrs) && matches!(node.fields, syn::Fields::Named(_)) {
            self.0.push((
                "struct",
                tokens(&format!(
                    "struct {} {}",
                    node.ident,
                    node.fields.span().source_text().unwrap()
                )),
            ));
        }
    }
    fn visit_item_macro(&mut self, node: &'ast syn::ItemMacro) {
        if test_only(&node.attrs) {
            return;
        }
        self.0.push(("macro", node.mac.tokens.to_string()));
        visit::visit_item_macro(self, node);
    }
}

fn guards_match(source: &str, proof: &serde_json::Value) -> bool {
    let mut items = Items::default();
    items.visit_file(&syn::parse_file(source).unwrap());
    proof["guards"].as_array().unwrap().iter().all(|guard| {
        let expected = tokens(guard["source"].as_str().unwrap());
        items
            .0
            .iter()
            .filter(|(kind, source)| {
                *kind == guard["kind"].as_str().unwrap() && *source == expected
            })
            .count()
            == 1
    })
}

pub fn register(root: &Path, records: &mut [serde_json::Value]) {
    let proofs: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../fixtures/checkpoint620-static-proofs.json")).unwrap();
    for proof in proofs {
        let file = proof["file"].as_str().unwrap();
        for guard in proof["guards"].as_array().unwrap() {
            let dependency = guard["file"].as_str().unwrap_or(file);
            assert!(
                guards_match(
                    &std::fs::read_to_string(root.join(dependency)).unwrap(),
                    &serde_json::json!({"guards": [guard]})
                ),
                "dipendenze della prova statica modificate: {dependency}"
            );
        }
        for cycle in proof["cycles"].as_array().unwrap() {
            let matching: Vec<_> = records
                .iter_mut()
                .filter(|record| {
                    record["file"] == proof["file"]
                        && record["counter"] == cycle["counter"]
                        && record["source"] == cycle["source"]
                })
                .collect();
            assert_eq!(
                matching.len(),
                1,
                "ciclo della prova statica modificato: {file}"
            );
            let record = matching.into_iter().next().unwrap();
            assert!(
                record["registered_iteration_bound"].is_null(),
                "prova duplicata"
            );
            record["registered_iteration_bound"] = cycle["iterations"].clone();
            record["registered_iteration_reason"] = proof["reason"].clone();
        }
    }
}

#[test]
fn static_proofs_reject_changed_dependencies_and_comment_copies() {
    let proofs: Vec<serde_json::Value> =
        serde_json::from_str(include_str!("../fixtures/checkpoint620-static-proofs.json")).unwrap();
    for proof in proofs {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        // Ogni dipendenza e obbligatoria: una copia nel commento non e un item AST.
        for guard in proof["guards"].as_array().unwrap() {
            let dependency = guard["file"]
                .as_str()
                .unwrap_or_else(|| proof["file"].as_str().unwrap());
            let source = std::fs::read_to_string(root.join(dependency)).unwrap();
            let proof = serde_json::json!({"guards": [guard]});
            assert!(guards_match(&source, &proof));
            let source = source.replace("\r\n", "\n");
            let text = guard["source"].as_str().unwrap();
            assert!(source.contains(text));
            let replacement = match guard["kind"].as_str().unwrap() {
                "function" => text.replacen("fn ", "fn changed_", 1),
                "const" => text.replacen("const ", "const CHANGED_", 1),
                "macro" | "use" => String::new(),
                "struct" => text.replacen("struct ", "struct Changed", 1),
                _ => unreachable!(),
            };
            let changed = source.replacen(text, &format!("{replacement} /* {text} */"), 1);
            // I corpi macro non sono Rust autonomo: rimuoverli lascia una macro
            // sintatticamente valida ma diversa; gli item funzione spariscono.
            assert!(!guards_match(&changed, &proof));
        }
    }
}
