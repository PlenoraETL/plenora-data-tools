//! Regole strutturali: cardinalita' locale, mai costo totale di una chiamata.
use super::*;
use std::collections::BTreeMap;

pub type Proofs = BTreeMap<(usize, usize), serde_json::Value>;

#[derive(Clone, Copy)]
enum Fact {
    Array(u128),
    Slice,
    ValidatedSlice(u128),
}

fn integer(expr: &syn::Expr) -> Option<u128> {
    let syn::Expr::Lit(lit) = expr else {
        return None;
    };
    let syn::Lit::Int(value) = &lit.lit else {
        return None;
    };
    // Anche con overflowing_literals: nessun troncamento in un tipo intero.
    value.base10_parse().ok().filter(|value| *value <= 127)
}

fn name(expr: &syn::Expr) -> Option<String> {
    let syn::Expr::Path(path) = expr else {
        return None;
    };
    path.path.get_ident().map(ToString::to_string)
}

fn typed(ty: &syn::Type) -> Option<Fact> {
    match ty {
        syn::Type::Array(array) => integer(&array.len).map(Fact::Array),
        syn::Type::Reference(reference) => match reference.elem.as_ref() {
            syn::Type::Slice(_) if reference.mutability.is_none() => Some(Fact::Slice),
            syn::Type::Array(array) => integer(&array.len).map(Fact::Array),
            _ => None,
        },
        _ => None,
    }
}

/// R-ARRAY: array Rust letterale o tipo [T; N]; la cardinalita' non puo'
/// cambiare per mutazione degli elementi. Nessuna assunzione su tipi custom.
fn collection(expr: &syn::Expr, facts: &BTreeMap<String, Fact>) -> Option<u128> {
    match expr {
        syn::Expr::Array(array) => Some(array.elems.len() as u128),
        syn::Expr::Repeat(repeat) => integer(&repeat.len),
        syn::Expr::Reference(reference) => collection(&reference.expr, facts),
        syn::Expr::Paren(paren) => collection(&paren.expr, facts),
        syn::Expr::If(node) => {
            let syn::Stmt::Expr(left, None) = node.then_branch.stmts.last()? else {
                return None;
            };
            if !matches!(left, syn::Expr::Array(_) | syn::Expr::Repeat(_)) {
                return None;
            }
            let n = collection(left, &BTreeMap::new())?;
            let (_, right) = node.else_branch.as_ref()?;
            let syn::Expr::Block(right) = right.as_ref() else {
                return None;
            };
            let syn::Stmt::Expr(right, None) = right.block.stmts.last()? else {
                return None;
            };
            if !matches!(right, syn::Expr::Array(_) | syn::Expr::Repeat(_)) {
                return None;
            }
            (collection(right, &BTreeMap::new())? == n).then_some(n)
        }
        syn::Expr::Block(node) if node.label.is_none() && node.block.stmts.len() == 1 => {
            let syn::Stmt::Expr(value, None) = node.block.stmts.first()? else {
                return None;
            };
            collection(value, facts)
        }
        syn::Expr::Path(_) => match facts.get(&name(expr)?)? {
            Fact::Array(n) | Fact::ValidatedSlice(n) => Some(*n),
            Fact::Slice => None,
        },
        _ => None,
    }
}

// Solo &[T] oppure &array[..]: niente autoderef da array o &mut[T]
// dove un trait custom puo' precedere i metodi intrinseci della slice.
fn slice(expr: &syn::Expr, facts: &BTreeMap<String, Fact>) -> Option<u128> {
    if let syn::Expr::Paren(paren) = expr {
        return slice(&paren.expr, facts);
    }
    if let Some(Fact::ValidatedSlice(n)) = name(expr).and_then(|key| facts.get(&key)) {
        return Some(*n);
    }
    let syn::Expr::Reference(reference) = expr else {
        return None;
    };
    if reference.mutability.is_some() {
        return None;
    }
    let syn::Expr::Index(index) = reference.expr.as_ref() else {
        return None;
    };
    if matches!(index.index.as_ref(), syn::Expr::Range(range) if range.start.is_none() && range.end.is_none())
    {
        collection(&index.expr, facts)
    } else {
        None
    }
}

/// R-ITER: solo adattatori standard senza callback e che non aumentano
/// la cardinalita'; chunks/windows richiedono un passo letterale positivo.
/// cycle, chain, `flat_map`, iteratori custom e metodi sconosciuti sono aperti.
fn iterations(expr: &syn::Expr, facts: &BTreeMap<String, Fact>) -> Option<u128> {
    if let Some(n) = collection(expr, facts).or_else(|| literal_range_bound(expr)) {
        return Some(n);
    }
    match expr {
        syn::Expr::Paren(paren) => iterations(&paren.expr, facts),
        syn::Expr::MethodCall(call) if call.attrs.is_empty() && call.turbofish.is_none() => {
            let method = call.method.to_string();
            match method.as_str() {
                "iter" if call.args.is_empty() => slice(&call.receiver, facts),
                "enumerate" | "rev" | "copied"
                    if call.args.is_empty() && iterator_receiver(&call.receiver) =>
                {
                    iterations(&call.receiver, facts)
                }
                "chunks" | "chunks_exact" | "windows" if call.args.len() == 1 => {
                    let size = integer(call.args.first()?)?;
                    if size == 0 {
                        return None;
                    }
                    let n = slice(&call.receiver, facts)?;
                    Some(match method.as_str() {
                        "chunks" => n / size + u128::from(n % size != 0),
                        "chunks_exact" => n / size,
                        _ => {
                            if n < size {
                                0
                            } else {
                                n - size + 1
                            }
                        }
                    })
                }
                _ => None,
            }
        }
        // R-LEN-RANGE: estremo superiore dalla stessa collezione limitata.
        syn::Expr::Range(range) if matches!(range.limits, syn::RangeLimits::HalfOpen(_)) => {
            let start = integer(range.start.as_deref()?)?;
            let end_expr = range.end.as_deref()?;
            let syn::Expr::MethodCall(end) = end_expr else {
                return None;
            };
            if end.method != "len" || !end.args.is_empty() || end.turbofish.is_some() {
                return None;
            }
            Some(slice(&end.receiver, facts)?.saturating_sub(start))
        }
        _ => None,
    }
}

fn rule_name(expr: &syn::Expr, facts: &BTreeMap<String, Fact>) -> &'static str {
    if literal_range_bound(expr).is_some() {
        return "R-LITERAL-RANGE";
    }
    match expr {
        syn::Expr::Array(_) | syn::Expr::Repeat(_) => "R-ARRAY-LITERAL",
        syn::Expr::Paren(paren) => rule_name(&paren.expr, facts),
        syn::Expr::Reference(reference) => rule_name(&reference.expr, facts),
        syn::Expr::Range(_) => "R-LENGTH-RANGE",
        syn::Expr::MethodCall(_) => "R-FINITE-ITERATOR",
        syn::Expr::Path(_)
            if matches!(
                name(expr).and_then(|key| facts.get(&key)),
                Some(Fact::ValidatedSlice(_))
            ) =>
        {
            "R-DOMINATING-LENGTH-GUARD"
        }
        _ => "R-FIXED-ARRAY",
    }
}

fn rule_reason(rule: &str) -> &'static str {
    match rule {
        "R-LITERAL-RANGE" => "Estremi interi letterali rappresentabili senza troncamento; differenza controllata, incluso l estremo chiuso.",
        "R-ARRAY-LITERAL" => "Numero di elementi AST dell array, oppure ripetizione con lunghezza letterale; cfg puo solo ridurlo.",
        "R-FIXED-ARRAY" => "Tipo array o inizializzatore array nello scope dominante; binding non oscurato. I rami condizionali producono array per valore di uguale lunghezza.",
        "R-FINITE-ITERATOR" => "Iteratore intrinseco di una shared slice limitata e adattatori standard non espandenti senza callback; chunks/windows con passo positivo letterale.",
        "R-LENGTH-RANGE" => "Range semiaperto con inizio letterale e len intrinseca della stessa shared slice con cardinalita gia provata.",
        "R-DOMINATING-LENGTH-GUARD" => "Slice condivisa non riassegnabile; la guardia immediatamente precedente rifiuta len oltre il limite letterale con return incondizionato.",
        _ => panic!("regola strutturale senza motivazione"),
    }
}

// Un array ha IntoIterator, ma non e' esso stesso un Iterator: un metodo
// omonimo fornito da un trait custom non eredita le garanzie degli adattatori.
fn iterator_receiver(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::MethodCall(_) | syn::Expr::Range(_) => true,
        syn::Expr::Paren(paren) => iterator_receiver(&paren.expr),
        _ => false,
    }
}

#[derive(Default)]
struct Bindings(BTreeMap<String, usize>, bool);
impl<'ast> Visit<'ast> for Bindings {
    fn visit_pat(&mut self, node: &'ast syn::Pat) {
        if matches!(node, syn::Pat::Macro(_)) {
            self.1 = true;
        }
        visit::visit_pat(self, node);
    }
    fn visit_item(&mut self, node: &'ast syn::Item) {
        let ident = match node {
            syn::Item::Const(item) => Some(&item.ident),
            syn::Item::Static(item) => Some(&item.ident),
            syn::Item::Fn(item) => Some(&item.sig.ident),
            _ => {
                self.1 = true;
                None
            }
        };
        if let Some(ident) = ident {
            *self.0.entry(ident.to_string()).or_default() += 1;
        }
    }

    fn visit_pat_ident(&mut self, node: &'ast syn::PatIdent) {
        *self.0.entry(node.ident.to_string()).or_default() += 1;
        visit::visit_pat_ident(self, node);
    }
}

struct Recognizer {
    facts: BTreeMap<String, Fact>,
    bindings: Bindings,
    proofs: Proofs,
}

impl Recognizer {
    fn bind(&mut self, pat: &syn::Pat, fact: Option<Fact>) {
        if self.bindings.1 {
            return;
        }
        if let syn::Pat::Ident(pat) = pat {
            let key = pat.ident.to_string();
            if pat.by_ref.is_none()
                && pat.subpat.is_none()
                && self.bindings.0.get(&key) == Some(&1)
                && !matches!(fact, Some(Fact::Slice))
            {
                if let Some(fact) = fact {
                    self.facts.insert(key, fact);
                }
            } else if pat.by_ref.is_none()
                && pat.subpat.is_none()
                && pat.mutability.is_none()
                && self.bindings.0.get(&key) == Some(&1)
                && matches!(fact, Some(Fact::Slice))
            {
                self.facts.insert(key, fact.unwrap());
            }
        }
    }

    /// R-LEN-GUARD: `if slice.len() > K { return ... }` domina il ciclo
    /// immediatamente successivo nello stesso blocco. Solo parametri slice
    /// non riassegnabili: il compilatore garantisce len intrinseco e stabile.
    fn guard(&mut self, stmt: &syn::Stmt) {
        let syn::Stmt::Expr(syn::Expr::If(node), _) = stmt else {
            return;
        };
        if !node.attrs.is_empty() || node.else_branch.is_some() {
            return;
        }
        let syn::Expr::Binary(test) = node.cond.as_ref() else {
            return;
        };
        if !matches!(test.op, syn::BinOp::Gt(_)) {
            return;
        }
        let syn::Expr::MethodCall(call) = test.left.as_ref() else {
            return;
        };
        if call.method != "len" || !call.args.is_empty() || call.turbofish.is_some() {
            return;
        }
        let Some(key) = name(&call.receiver) else {
            return;
        };
        if !matches!(self.facts.get(&key), Some(Fact::Slice)) {
            return;
        }
        let Some(n) = integer(&test.right) else {
            return;
        };
        if matches!(node.then_branch.stmts.as_slice(), [syn::Stmt::Expr(syn::Expr::Return(ret), _)] if ret.attrs.is_empty())
        {
            self.facts.insert(key, Fact::ValidatedSlice(n));
        }
    }
}

impl<'ast> Visit<'ast> for Recognizer {
    fn visit_item(&mut self, _: &'ast syn::Item) {}
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_block(&mut self, block: &'ast syn::Block) {
        let previous = self.facts.clone();
        for stmt in &block.stmts {
            self.visit_stmt(stmt);
            for fact in self.facts.values_mut() {
                if matches!(fact, Fact::ValidatedSlice(_)) {
                    *fact = Fact::Slice;
                }
            }
            if let syn::Stmt::Local(local) = stmt {
                if local.attrs.is_empty() {
                    match &local.pat {
                        syn::Pat::Type(pat) => self.bind(&pat.pat, typed(&pat.ty)),
                        pat => {
                            let fact = local.init.as_ref().and_then(|init| {
                                collection(
                                    &init.expr,
                                    &self
                                        .facts
                                        .iter()
                                        .filter(|(_, fact)| matches!(fact, Fact::Array(_)))
                                        .map(|(key, fact)| (key.clone(), *fact))
                                        .collect(),
                                )
                                .map(Fact::Array)
                            });
                            self.bind(pat, fact);
                        }
                    }
                }
            }
            // Una macro puo' introdurre un binding invisibile a syn.
            if matches!(stmt, syn::Stmt::Macro(_)) {
                self.facts.clear();
            }
            self.guard(stmt);
        }
        self.facts = previous;
    }
    fn visit_expr_for_loop(&mut self, node: &'ast syn::ExprForLoop) {
        if let Some(bound) = iterations(&node.expr, &self.facts) {
            let start = node.span().start();
            let rule = rule_name(&node.expr, &self.facts);
            self.proofs.insert(
                (start.line, start.column),
                serde_json::json!({
                    "rule": rule,
                    "iterations": bound.to_string(),
                    "reason": rule_reason(rule)
                }),
            );
        }
        visit::visit_expr_for_loop(self, node);
    }
}

pub fn function(signature: &syn::Signature, block: &syn::Block) -> Proofs {
    let mut bindings = Bindings::default();
    bindings.visit_signature(signature);
    bindings.visit_block(block);
    let mut scanner = Recognizer {
        facts: BTreeMap::new(),
        bindings,
        proofs: Proofs::new(),
    };
    for arg in &signature.inputs {
        if let syn::FnArg::Typed(arg) = arg {
            if arg.attrs.is_empty() {
                scanner.bind(&arg.pat, typed(&arg.ty));
            }
        }
    }
    scanner.visit_block(block);
    scanner.proofs
}

#[test]
fn structural_rules_accept_only_proven_cardinality() {
    for (source, expected) in [
        (
            "fn f(a: [u8; 4]) { for x in (&a[..]).iter().copied().enumerate() {} }",
            Some(4),
        ),
        (
            "fn f(a: &[u8; 4]) { for x in (&a[..]).windows(2) {} }",
            Some(3),
        ),
        (
            "fn f() { let a = [0; 4]; for x in (&a[..]).chunks(3) {} }",
            Some(2),
        ),
        (
            "fn f() { let a = [0; 4]; for x in 1..(&a[..]).len() {} }",
            Some(3),
        ),
        (
            "fn f(a: &[u8]) { if a.len() > 4 { return; } for x in a {} }",
            Some(4),
        ),
        ("fn f(a: &[u8]) { for x in a {} }", None),
        (
            "fn f(n: ::core::primitive::u8) { for x in 0..n {} }",
            None,
        ),
        ("fn f(mut n: u8) { for x in 0..n {} }", None),
        ("fn f(a: [u8; 4]) { for x in a.iter() {} }", None),
        ("fn f(a: [u8; 4]) { for x in a.rev() {} }", None),
        (
            "fn f(a: [u8; 4]) { let bind!(a) = other; for x in a {} }",
            None,
        ),
        (
            "fn f(a: [u8; 4]) { { const a: [u8; 100] = [0;100]; for x in a {} } }",
            None,
        ),
        (
            "fn f() { let a = if flag { [1,2] } else { [3,4] }; for x in a {} }",
            Some(2),
        ),
        (
            "fn f(a: Other) { if a.len() > 4 { return; } for x in a.iter() {} }",
            None,
        ),
        (
            "fn f(a: &[u8]) { if flag { if a.len() > 4 { return; } } for x in a {} }",
            None,
        ),
        (
            "fn f(a: &[u8]) { for x in a {} if a.len() > 4 { return; } }",
            None,
        ),
        (
            "fn f(mut a: &[u8]) { if a.len() > 4 { return; } a = other; for x in a {} }",
            None,
        ),
        (
            "fn f(a: &[u8]) { #[cfg(test)] if a.len() > 4 { return; } for x in a {} }",
            None,
        ),
        ("fn f(a: [u8; 4]) { let a = other; for x in a {} }", None),
        ("fn f(a: [u8; 4]) { shadow!(); for x in a {} }", None),
        ("fn f(a: [u8; 4]) { for x in a.iter().cycle() {} }", None),
        (
            "fn f(a: [u8; 4]) { for x in a.iter().flat_map(expand) {} }",
            None,
        ),
        ("fn f(a: [u8; 4]) { for x in a.windows(0) {} }", None),
        ("fn f(a: [u8; N]) { for x in a {} }", None),
        ("fn f(flag: bool) { let a = 'b: { if flag { if flag { break 'b &[0;100][..]; } &[0;4] } else { &[0;4] } }; for x in a {} }", None),
        ("fn f(a: &mut [u8]) { if a.len() > 4 { return; } for x in a.iter() {} }", None),
        ("fn f(a: &[u8]) { if a.len() > 4 { return; } for x in a.iter().enumerate() {} }", Some(4)),
        ("fn f(a: &[u8]) { if a.len() > 4 { return; } touch(); for x in a {} }", None),
        ("fn f(a: &[u8]) { if a.len() > 4 { return; } for x in 0..a.len() {} }", Some(4)),
    ] {
        let node: syn::ItemFn = syn::parse_str(source).unwrap();
        let proofs = function(&node.sig, &node.block);
        assert_eq!(
            proofs.values().next().map(|p| p["iterations"]
                .as_str()
                .unwrap()
                .parse::<u128>()
                .unwrap()),
            expected,
            "{source}"
        );
    }
}
