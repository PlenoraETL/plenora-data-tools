// Il macro `op!` delle voci del catalogo (`catalog.rs`).
//
// Sta in un file a parte, incluso con `include!` da `catalog.rs` e dai
// doctest di `catalog::_CONTROLLO_OP`, perché la regola sulle chiavi si
// provi anche nel verso negativo: una voce con una chiave ripetuta non
// compila.
//
// Senza chiavi opzionali: tutte e quattro le versioni a 1, vincolo di
// espansione `SumRelative`, nessuna esenzione, `NotFusible`.
//
// Chiavi opzionali, in qualsiasi ordine, ognuna al più una volta:
// `semantic_version`, `config_schema_version`, `contract_analysis_version`,
// `kernel_version` (default 1), `expansion_constraint` (default
// `SumRelative`; un ident di variante oppure `Custom(fattore)` con il
// fattore f64), `expansion_factor_exempt` (default `false`) e `geo_fusion`
// (default `NotFusible`). Una chiave sconosciuta o ripetuta è un errore di
// compilazione (`compile_error!`), non l'ultima che vince.
macro_rules! op {
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident) => {
        op!(@munch
            ($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat)
            ([] [] [] [] [] [] []))
    };
    ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident,
     $($chiavi:tt)+) => {
        op!(@munch
            ($id, $family, $origin, $arity, $exec, $cancel, $shape, $crs, $caps, $det, $mat)
            ([] [] [] [] [] [] [])
            $($chiavi)+ ,)
    };
    // Muncher: una chiave per passo. L'accumulatore ha una casella per
    // chiave (semantic, config_schema, contract_analysis, kernel,
    // expansion_constraint, expansion_factor_exempt, geo_fusion): `[]` finché
    // la chiave non compare, `[valore]` dopo. Ogni regola accetta la sua
    // chiave solo con la casella vuota, quindi una seconda occorrenza non
    // trova regola e cade sull'ultima, che non compila.
    (@munch $base:tt ([] $c:tt $a:tt $k:tt $x:tt $e:tt $g:tt)
        semantic_version = $v:expr, $($resto:tt)*) => {
        op!(@munch $base ([$v] $c $a $k $x $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt [] $a:tt $k:tt $x:tt $e:tt $g:tt)
        config_schema_version = $v:expr, $($resto:tt)*) => {
        op!(@munch $base ($s [$v] $a $k $x $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt [] $k:tt $x:tt $e:tt $g:tt)
        contract_analysis_version = $v:expr, $($resto:tt)*) => {
        op!(@munch $base ($s $c [$v] $k $x $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt $a:tt [] $x:tt $e:tt $g:tt)
        kernel_version = $v:expr, $($resto:tt)*) => {
        op!(@munch $base ($s $c $a [$v] $x $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt $a:tt $k:tt [] $e:tt $g:tt)
        expansion_constraint = Custom($v:expr), $($resto:tt)*) => {
        op!(@munch $base ($s $c $a $k [ExpansionConstraint::Custom($v)] $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt $a:tt $k:tt [] $e:tt $g:tt)
        expansion_constraint = $v:ident, $($resto:tt)*) => {
        op!(@munch $base ($s $c $a $k [ExpansionConstraint::$v] $e $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt $a:tt $k:tt $x:tt [] $g:tt)
        expansion_factor_exempt = $v:expr, $($resto:tt)*) => {
        op!(@munch $base ($s $c $a $k $x [$v] $g) $($resto)*)
    };
    (@munch $base:tt ($s:tt $c:tt $a:tt $k:tt $x:tt $e:tt [])
        geo_fusion = $v:ident, $($resto:tt)*) => {
        op!(@munch $base ($s $c $a $k $x $e [GeoFusion::$v]) $($resto)*)
    };
    // Chiavi finite: i default riempiono le caselle vuote.
    (@munch $base:tt ($s:tt $c:tt $a:tt $k:tt $x:tt $e:tt $g:tt)) => {
        op!(@build $base (
            op!(@o $s 1),
            op!(@o $c 1),
            op!(@o $a 1),
            op!(@o $k 1),
            op!(@o $x ExpansionConstraint::SumRelative),
            op!(@o $e false),
            op!(@o $g GeoFusion::NotFusible)
        ))
    };
    // Nessuna regola sopra: la chiave è ripetuta (casella già piena) o
    // sconosciuta.
    (@munch $base:tt $acc:tt $chiave:ident $($resto:tt)*) => {
        compile_error!(concat!(
            "op!: chiave `",
            stringify!($chiave),
            "` ripetuta o sconosciuta"
        ))
    };
    (@o [] $default:expr) => {
        $default
    };
    (@o [$v:expr] $default:expr) => {
        $v
    };
    (@build ($id:literal, $family:ident, $origin:ident, $arity:ident, $exec:ident,
     $cancel:ident, $shape:expr, $crs:expr, $caps:expr, $det:ident, $mat:ident)
     ($semantic:expr, $config_schema:expr, $contract_analysis:expr, $kernel:expr,
      $constraint:expr, $exempt:expr, $fusion:expr)) => {
        OperationDescriptor {
            id: $id,
            family: Family::$family,
            origin: Origin::$origin,
            arity: Arity::$arity,
            execution_class: ExecutionClass::$exec,
            cancellation_behavior: CancellationBehavior::$cancel,
            geo_fusion: $fusion,
            result_shape: $shape,
            crs_requirement: $crs,
            required_capabilities: $caps,
            determinism: DeterminismPolicy::$det,
            expansion_constraint: $constraint,
            expansion_factor_exempt: $exempt,
            maturity: Maturity::$mat,
            semantic_version: $semantic,
            config_schema_version: $config_schema,
            contract_analysis_version: $contract_analysis,
            kernel_version: $kernel,
        }
    };
}
