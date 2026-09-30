//! Catalogo delle operazioni.
//!
//! Un [`OperationDescriptor`] per ogni operazione tabellare e geografica,
//! con id canonico, famiglia, arietà, forma del risultato, requisito CRS,
//! vincolo di espansione e versioni.
//!
//! Il catalogo è dichiarativo: nessun campo esegue qualcosa. Oggi lo
//! consultano il runner (`plenora-pipeline`: id, arietà, provenance delle
//! diagnostiche per riga, vincolo ed esenzione del fattore di espansione),
//! l'analisi dei contratti geo (`crs_requirement`) e il generatore di
//! `docs/operazioni.md`, che ne riporta i campi nella tabella «dal
//! catalogo» di ogni operazione. I campi che nessun codice di questo
//! repository consulta lo dicono nel proprio rustdoc: vengono da
//! `plenora-data-tools@190c493`, dove li usavano engine e planner, e restano
//! come dichiarazione.

/// Famiglia dell'operazione: il prefisso del suo `id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// Operazione tabellare, `id` `table.*`: lavora su colonne Arrow senza
    /// geometrie.
    Table,
    /// Operazione geografica, `id` `geo.*`: legge o produce una colonna
    /// geometria (`GeoArrow` WKB) con un CRS.
    Geo,
}

/// Provenienza dell'operazione.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    /// Compatibile con l'operazione omonima di Manipola, lo strumento Python
    /// di riferimento.
    ManipolaCompat,
    /// Estensione propria, senza corrispondente in Manipola.
    Extension,
}

/// Numero e ruolo delle tabelle d'ingresso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arity {
    /// Un ingresso.
    Unary,
    /// Due ingressi in ordine fisso: sinistra, destra.
    BinaryOrdered,
    /// Due o più ingressi equivalenti (`table.concat`,
    /// `table.concat_by_name`). Il runner ne esegue al più due.
    NAry,
}

/// Classe di esecuzione: quanto dell'ingresso serve prima di produrre
/// l'uscita.
///
/// Dichiarazione del progetto d'origine per il planner a segmenti: il
/// runner di questo repository esegue sempre su tabelle intere e non la
/// consulta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecutionClass {
    /// Riga per riga, senza stato fra un batch e l'altro: si può eseguire
    /// batch per batch.
    Streaming,
    /// Serve l'intero ingresso (ordinamenti, aggregazioni, tessellazioni).
    Blocking,
    /// Servono per intero entrambi gli ingressi (join, overlay, operazioni
    /// insiemistiche).
    BinaryBlocking,
}

/// Dove l'operazione può osservare una richiesta di annullamento.
///
/// Dichiarazione del progetto d'origine: in questo repository non c'è un
/// meccanismo di cancellazione e nessun codice la consulta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CancellationBehavior {
    /// Fra un batch e l'altro (kernel streaming).
    Cooperative,
    /// Solo prima e dopo l'operazione, non durante (kernel bloccanti).
    BoundaryOnly,
    /// Nessun punto di cancellazione: il kernel gira fino in fondo
    /// (`geo.make_valid`, `geo.polygonize`, `geo.split`, `geo.reproject`).
    NonInterruptible,
}

/// Fondibilità di un'operazione geo con le vicine, per decodificare la
/// geometria una volta sola lungo una catena di passi 1:1.
///
/// Dichiarazione del progetto d'origine: il runner di questo repository non
/// esegue le geo e non fonde passi. È una proprietà fisica, non semantica:
/// il fingerprint del catalogo del progetto d'origine non la comprendeva.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeoFusion {
    /// Non fondibile: si esegue da sola. Tutte le tabellari e le geo che non
    /// sono né trasformazioni 1:1 sul posto né misure terminali.
    NotFusible,
    /// Trasformazione 1:1 della geometria sulla stessa colonna (affini,
    /// buffer, semplificazioni, involucri, `reproject`, `make_valid`):
    /// fondibile con le vicine dello stesso genere.
    TransformInPlace,
    /// Misura che consuma la geometria e produce un valore non geometrico
    /// (`area`, `length`, `perimeter`, `vertex_count`, `to_wkt`): chiude una
    /// catena fusa a monte.
    TerminalMeasure,
}

impl GeoFusion {
    /// Nome stabile `snake_case` della variante, indipendente dal `Debug`
    /// di Rust.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotFusible => "not_fusible",
            Self::TransformInPlace => "transform_in_place",
            Self::TerminalMeasure => "terminal_measure",
        }
    }
}

/// Forma del risultato di un'operazione geo rispetto alle righe d'ingresso.
///
/// Le tabellari non la dichiarano (`result_shape` è `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ResultShape {
    /// Una riga d'uscita per riga d'ingresso, nello stesso ordine; per le
    /// binarie, una per riga del primo ingresso, e la destra non aggiunge
    /// righe (`clip`, le booleane allineate, `count_points_in_polygons`,
    /// `within`).
    OneToOne,
    /// Ogni riga d'ingresso produce zero, una o più righe (esplosioni,
    /// tagli, triangolazioni); per le binarie, gli abbinamenti fra i lati
    /// (`sjoin`, `nearest`, `overlay`).
    OneToMany,
    /// Più righe d'ingresso si fondono in una: al più una riga per riga
    /// d'ingresso (`collect`, una per gruppo), o sempre una sola, anche da
    /// una tabella vuota (`dissolve`, costruzione di linee e poligoni).
    ManyToOne,
    /// Il risultato dipende dall'intero ingresso senza corrispondenza fra
    /// righe. Nessuna operazione del catalogo la dichiara oggi.
    Collective,
    /// Dall'intero ingresso a un numero di righe che non si lega alle righe
    /// d'ingresso, anche più di quelle (griglia generata, problemi di
    /// copertura, tratti condivisi, fusione di linee, poligonizzazione);
    /// queste operazioni sono esenti dal fattore di espansione.
    WholeToMany,
    /// Produttore 1:1: costruisce la geometria da colonne non geometriche
    /// della stessa riga (`geo.from_coords`, `geo.from_wkt`).
    FromCoords,
    /// Una riga di diagnostica per riga d'ingresso
    /// (`geo.geometry_diagnostics`).
    Diagnostic,
}

/// Se l'indice di riga della sorgente resta valido attraverso l'operazione.
///
/// `Preserved` significa che ogni configurazione valida mantiene numero e
/// ordine delle righe (del primo ingresso, per le binarie). Ogni altra
/// operazione è `Unavailable`: senza una traccia di lineage non si può
/// ricostruire l'indice originale, e il runner dichiara gli indici della
/// diagnostica per riga di un passo a valle come righe dell'ingresso del
/// passo (`step_input_row_zero_based`), non della sorgente. È questa
/// classificazione, non [`OperationDescriptor::emits_row_diagnostics`], che
/// decide la base: un `Preserved` sbagliato farebbe leggere come righe della
/// sorgente indici che non lo sono.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRowProvenance {
    /// Numero e ordine delle righe invariati: l'indice di riga d'uscita è
    /// quello della sorgente.
    Preserved,
    /// Almeno una configurazione può filtrare, riordinare, espandere o
    /// aggregare righe.
    Unavailable,
}

/// Requisito sul CRS degli ingressi di un'operazione geo, verificato
/// dall'analisi del contratto con [`crate::crs::validate_requirement`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CrsRequirement {
    /// Un CRS risolto qualunque, geografico o proiettato.
    Known,
    /// CRS proiettato con unità lineare nota su ogni ingresso: distanze e
    /// aree sono in unità di mappa.
    Projected,
    /// CRS geografico su ogni ingresso (misure geodetiche).
    Geographic,
    /// CRS proiettato con unità nota, lo stesso (per equivalenza semantica)
    /// su tutti gli ingressi; con un solo ingresso vale come `Projected`.
    SameProjected,
    /// CRS d'origine e di destinazione entrambi risolti (`geo.reproject`).
    Reprojection,
}

/// Politica d'ordine delle righe d'uscita: stesso ingresso, stesso ordine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeterminismPolicy {
    /// Ordine definito dalla semantica dell'operazione: quello d'ingresso
    /// per le 1:1, quello della chiave per gli ordinamenti, quello descritto
    /// nella scheda per le altre.
    DefinedOrder,
    /// Ordine d'arrivo delle righe e, con più ingressi, degli ingressi
    /// nell'ordine in cui sono dati (`table.concat`, `table.concat_by_name`,
    /// `table.limit`).
    InputOrder,
    /// Ordinamento stabile su una chiave dichiarata. Nessuna operazione del
    /// catalogo la dichiara oggi.
    StableKeyOrder,
    /// Ordine canonico dei valori, indipendente dall'ordine d'ingresso
    /// (operazioni insiemistiche e aggregazioni senza ordine proprio).
    CanonicalOrder,
}

/// Base del fattore di espansione di un'operazione con due ingressi.
///
/// Il runner confronta le righe d'uscita con la base scelta qui e con la
/// soglia `max_expansion_factor` dei limiti (o il fattore di
/// [`ExpansionConstraint::Custom`]); per le unarie la base è sempre l'unico
/// ingresso e questo campo non conta.
///
/// `PartialEq`/`Eq`/`Hash` sono scritti a mano: il fattore di `Custom` si
/// confronta e si hasha per bit (`f64::to_bits`), senza ambiguità su NaN e
/// `-0.0`.
#[derive(Debug, Clone, Copy)]
pub enum ExpansionConstraint {
    /// `uscita / (sinistra + destra)`: il default, e il vincolo degli
    /// abbinamenti molti-a-molti (`table.join`, `table.cross_join`,
    /// `table.fuzzy_join`, `geo.sjoin`, `geo.overlay`). È la base su cui è tarato
    /// `max_expansion_factor`, quindi un'operazione che non dichiara un
    /// vincolo proprio è misurata come le soglie.
    ///
    /// Con questa base un abbinamento in cui la chiave è unica su almeno un
    /// lato (1:1, 1:N, N:1) non supera mai 1: l'uscita di un inner join è al
    /// più il lato maggiore, quella di un left o outer join al più la somma
    /// dei lati. Supera la soglia solo un molti-a-molti che moltiplica le
    /// righe (chiave sbagliata, prodotto cartesiano involontario).
    SumRelative,
    /// `uscita / sinistra`: operazioni guidate dal lato sinistro, con al più
    /// una riga o poche righe per riga sinistra (join semi, anti e asof,
    /// `except`, `intersect`, `assert_foreign_key`, `geo.nearest`,
    /// `geo.within`, `geo.count_points_in_polygons`, `geo.clip` e le
    /// booleane geo allineate).
    LeftRelative,
    /// `uscita / destra`. Nessuna operazione del catalogo la dichiara oggi.
    RightRelative,
    /// `max(uscita / sinistra, uscita / destra)`, cioè l'uscita sul lato
    /// minore. Nessuna operazione del catalogo la dichiara oggi: misura
    /// l'asimmetria dei lati, non l'espansione, e rifiutava i join
    /// legittimi fra una tabella grande e una piccola (un left join di
    /// 10 600 righe su una dimensione di 100 righe con chiave unica vale
    /// 106 volte il lato destro senza duplicare una sola riga).
    MaxRelative,
    /// Soglia propria dell'operazione, per un'uscita senza una base fissa
    /// caratterizzabile.
    ///
    /// La metrica resta `output_over_sum_inputs`, ma la soglia è il fattore
    /// dichiarato, che sostituisce `max_expansion_factor` per l'operazione
    /// ([`ExpansionConstraint::binding_threshold`]). Il fattore, costante di
    /// catalogo, dev'essere finito e positivo. Nessuna operazione del
    /// catalogo la dichiara oggi.
    Custom(f64),
}

impl PartialEq for ExpansionConstraint {
    fn eq(&self, other: &Self) -> bool {
        match (*self, *other) {
            (Self::SumRelative, Self::SumRelative)
            | (Self::LeftRelative, Self::LeftRelative)
            | (Self::RightRelative, Self::RightRelative)
            | (Self::MaxRelative, Self::MaxRelative) => true,
            (Self::Custom(a), Self::Custom(b)) => a.to_bits() == b.to_bits(),
            _ => false,
        }
    }
}

impl Eq for ExpansionConstraint {}

impl std::hash::Hash for ExpansionConstraint {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::SumRelative => state.write_u8(0),
            Self::LeftRelative => state.write_u8(1),
            Self::RightRelative => state.write_u8(2),
            Self::MaxRelative => state.write_u8(3),
            Self::Custom(factor) => {
                state.write_u8(4);
                state.write_u64(factor.to_bits());
            }
        }
    }
}

impl ExpansionConstraint {
    /// Soglia effettiva del fattore di espansione per questo vincolo: il
    /// fattore di [`ExpansionConstraint::Custom`] (che sostituisce
    /// `max_expansion_factor` per la singola operazione), altrimenti il
    /// `max_expansion_factor` dei limiti effettivi passato dal chiamante.
    #[must_use]
    pub const fn binding_threshold(self, max_expansion_factor: f64) -> f64 {
        match self {
            Self::Custom(factor) => factor,
            _ => max_expansion_factor,
        }
    }

    /// `true` se l'espansione osservata supera la soglia di questo vincolo.
    ///
    /// Decide in aritmetica intera ([`crate::limits::expansion_exceeded`]),
    /// non sulle metriche `f64` di [`JoinExpansion`], che oltre `2^53`
    /// arrotondano i conteggi e potrebbero non far scattare il limite.
    ///
    /// Base: la somma degli input (`SumRelative`, `Custom`), il solo lato
    /// sinistro o destro, e per `MaxRelative` il massimo delle metriche, che
    /// supera la soglia se e solo se almeno una la supera. Con denominatore
    /// nullo vale la convenzione di [`JoinExpansion::compute`]: scatta solo se
    /// l'output non e' vuoto.
    #[must_use]
    pub fn exceeded(
        self,
        output_rows: u64,
        left_rows: u64,
        right_rows: u64,
        max_expansion_factor: f64,
    ) -> bool {
        let threshold = self.binding_threshold(max_expansion_factor);
        let left = u128::from(left_rows);
        let right = u128::from(right_rows);
        let exceeded =
            |base: u128| crate::limits::expansion_exceeded_wide(output_rows, base, threshold);
        match self {
            // La somma in `u128` non satura: due conteggi a 64 bit ci stanno
            // sempre, e saturare abbasserebbe il denominatore proprio dove
            // deciderebbe il limite.
            Self::SumRelative | Self::Custom(_) => exceeded(left + right),
            Self::LeftRelative => exceeded(left),
            Self::RightRelative => exceeded(right),
            Self::MaxRelative => exceeded(left) || exceeded(right),
        }
    }
}

/// Metriche di espansione di un'operazione binaria.
///
/// Il vincolo dichiarato in catalogo ([`ExpansionConstraint`]) seleziona
/// quella vincolante. Sono metriche da riportare, in `f64`: il limite si
/// decide in aritmetica esatta con [`ExpansionConstraint::exceeded`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JoinExpansion {
    /// Righe d'uscita / (righe sinistre + righe destre).
    pub output_over_sum_inputs: f64,
    /// Righe d'uscita / righe sinistre.
    pub output_over_left: f64,
    /// Righe d'uscita / righe destre.
    pub output_over_right: f64,
}

impl JoinExpansion {
    /// Calcola le tre metriche dalle righe d'uscita, sinistre e destre.
    ///
    /// Denominatore nullo: la metrica è infinita se l'uscita non è vuota
    /// (espansione da un ingresso vuoto), zero altrimenti.
    #[must_use]
    #[allow(clippy::cast_precision_loss)] // Metriche f64 per contratto: sotto 2^53 righe sono esatte, e il limite non le usa.
    pub fn compute(output_rows: u64, left_rows: u64, right_rows: u64) -> Self {
        fn ratio(numerator: u64, denominator: u64) -> f64 {
            if denominator == 0 {
                if numerator == 0 {
                    0.0
                } else {
                    f64::INFINITY
                }
            } else {
                numerator as f64 / denominator as f64
            }
        }
        Self {
            // Somma saturante: avvolgere abbasserebbe il denominatore, cioè
            // alzerebbe il rapporto riportato (e in debug farebbe abortire
            // con `overflow-checks`).
            output_over_sum_inputs: ratio(output_rows, left_rows.saturating_add(right_rows)),
            output_over_left: ratio(output_rows, left_rows),
            output_over_right: ratio(output_rows, right_rows),
        }
    }

    /// Restituisce la metrica vincolante per il vincolo dichiarato in
    /// catalogo. `MaxRelative` è il massimo delle tre (la metrica sulla
    /// somma è sempre dominata dalle altre due, quindi includerla non
    /// cambia il risultato). `Custom` usa la metrica sulla somma degli
    /// ingressi: la specificità del vincolo è nella soglia
    /// ([`ExpansionConstraint::binding_threshold`]), non nella base.
    #[must_use]
    pub const fn binding_metric(&self, constraint: ExpansionConstraint) -> f64 {
        match constraint {
            ExpansionConstraint::SumRelative | ExpansionConstraint::Custom(_) => {
                self.output_over_sum_inputs
            }
            ExpansionConstraint::LeftRelative => self.output_over_left,
            ExpansionConstraint::RightRelative => self.output_over_right,
            ExpansionConstraint::MaxRelative => self
                .output_over_sum_inputs
                .max(self.output_over_left)
                .max(self.output_over_right),
        }
    }
}

/// Livello di maturità dell'operazione, dal progetto d'origine.
///
/// Nessun codice di questo repository lo consulta: il runner non filtra per
/// maturità (esegue le operazioni che hanno un dispatch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Maturity {
    /// Dichiarata, senza kernel. Nessuna operazione del catalogo è in
    /// questo stato.
    Planned,
    /// Kernel scritto ma in attesa di un backend esterno (GEOS o PROJ nel
    /// progetto d'origine). Nessuna operazione del catalogo è in questo
    /// stato: qui non ci sono backend esterni.
    BackendPending,
    /// Kernel validato dai propri test; nel progetto d'origine non era
    /// ancora esposto dal protocollo pubblico dell'engine.
    KernelValidated,
    /// Kernel validato ed esposto dal protocollo pubblico dell'engine del
    /// progetto d'origine. Qui non c'è un protocollo pubblico: la
    /// distinzione da `KernelValidated` è storica.
    PublicProtocol,
}

/// Descrizione di un'operazione del catalogo.
///
/// Le quattro versioni per componente dicono che cosa è cambiato in modo
/// osservabile: ogni modifica incompatibile incrementa quella pertinente.
#[derive(Debug, Clone)]
pub struct OperationDescriptor {
    /// Id canonico con il prefisso della famiglia: `table.*` o `geo.*`.
    pub id: &'static str,
    /// Famiglia, coerente con il prefisso dell'id.
    pub family: Family,
    /// Compatibile con Manipola o estensione propria.
    pub origin: Origin,
    /// Numero e ruolo degli ingressi; il runner rifiuta un passo con un
    /// numero di ingressi diverso.
    pub arity: Arity,
    /// Quanto dell'ingresso serve prima dell'uscita (vedi
    /// [`ExecutionClass`]: il runner non la consulta).
    pub execution_class: ExecutionClass,
    /// Dove l'operazione può essere annullata (vedi
    /// [`CancellationBehavior`]: qui non c'è cancellazione).
    pub cancellation_behavior: CancellationBehavior,
    /// Fondibilità con i passi geo vicini (vedi [`GeoFusion`]): proprietà
    /// fisica, fuori dalla compatibilità semantica. `NotFusible` per tutte
    /// le tabellari.
    pub geo_fusion: GeoFusion,
    /// Forma del risultato rispetto alle righe d'ingresso; `None` per le
    /// tabellari, `Some` per tutte le geo.
    pub result_shape: Option<ResultShape>,
    /// Requisito sul CRS degli ingressi; `None` per le tabellari, `Some`
    /// per tutte le geo (l'analisi geo rifiuta con `InvalidPlan` una geo
    /// senza requisito).
    pub crs_requirement: Option<CrsRequirement>,
    /// Backend esterni richiesti. Vuoto per tutte le operazioni: GEOS e
    /// PROJ sono stati sostituiti da kernel in Rust puro.
    pub required_capabilities: &'static [&'static str],
    /// Politica d'ordine delle righe d'uscita.
    pub determinism: DeterminismPolicy,
    /// Base del fattore di espansione per le operazioni con due ingressi;
    /// per le unarie la base è l'unico ingresso e il campo non conta
    /// (default `SumRelative`).
    pub expansion_constraint: ExpansionConstraint,
    /// Esenzione da `max_expansion_factor`: le operazioni che per contratto
    /// producono dall'intero ingresso un numero di righe che non dipende
    /// dalle righe (`WholeToMany`: griglie generate, problemi di copertura,
    /// tratti condivisi, fusione di linee, poligonizzazione) o un numero di
    /// righe fisso (una per `geo.dissolve` e i costruttori di linee e
    /// poligoni, cinque per `table.reconcile`, una per regola per il
    /// riepilogo di `table.validate_rules`, anche da ingressi vuoti) non
    /// sono soggette al fattore; restano vincolate da `max_rows_per_edge` e
    /// dagli altri limiti di righe.
    pub expansion_factor_exempt: bool,
    /// Livello di maturità (vedi [`Maturity`]: nessun codice lo consulta).
    pub maturity: Maturity,
    /// Versione della semantica osservabile: si incrementa quando cambia
    /// l'uscita per lo stesso ingresso e la stessa config.
    pub semantic_version: u32,
    /// Versione dello schema della config: si incrementa quando cambiano i
    /// parametri accettati.
    pub config_schema_version: u32,
    /// Versione dell'analisi del contratto: si incrementa quando cambia lo
    /// schema d'uscita inferito o una regola di validazione.
    pub contract_analysis_version: u32,
    /// Versione del kernel: si incrementa quando cambia l'implementazione,
    /// anche a semantica invariata (per esempio il passaggio da GEOS a Rust
    /// puro).
    pub kernel_version: u32,
}

impl OperationDescriptor {
    /// Dichiara se la posizione sorgente resta osservabile per tutte le
    /// configurazioni valide dell'operazione.
    ///
    /// La classificazione è conservativa: una sola modalità capace di
    /// selezionare, riordinare, espandere o aggregare rende il descrittore
    /// `Unavailable`, così nessun indice di riga riportato è inventato.
    #[must_use]
    pub fn source_row_provenance(&self) -> SourceRowProvenance {
        match self.family {
            Family::Table => {
                if matches!(
                    self.id,
                    "table.bin"
                        | "table.add_row_number"
                        | "table.assert_unique"
                        | "table.assert_foreign_key"
                ) || (matches!(self.arity, Arity::Unary)
                    && !matches!(
                        self.id,
                        "table.aggregate"
                            | "table.dedup_advanced"
                            | "table.distinct"
                            | "table.filter"
                            | "table.limit"
                            | "table.melt"
                            | "table.pivot"
                            | "table.sample"
                            | "table.sort"
                            | "table.statistics"
                            | "table.top_n"
                            | "table.transpose"
                            | "table.validate_rules"
                            | "table.window_function"
                            | "table.rolling_window"
                            | "table.explode"
                            | "table.unnest"
                    ))
                {
                    SourceRowProvenance::Preserved
                } else {
                    SourceRowProvenance::Unavailable
                }
            }
            // `OneToOne` vale anche per le binarie: una riga per riga del primo
            // ingresso, nella sua posizione.
            Family::Geo => {
                if matches!(self.arity, Arity::Unary | Arity::BinaryOrdered)
                    && matches!(
                        self.result_shape,
                        Some(ResultShape::OneToOne | ResultShape::FromCoords)
                    )
                {
                    SourceRowProvenance::Preserved
                } else {
                    SourceRowProvenance::Unavailable
                }
            }
        }
    }

    /// Dichiara se l'operazione, nella configurazione data, può rifiutare
    /// righe con diagnostica per riga (`plenora-row-diagnostics-v1`).
    ///
    /// Descrive il catalogo (schede e audit); il runner non ne dipende:
    /// riscrive la base degli indici di qualunque payload di un passo il cui
    /// ingresso non conserva le righe della sorgente
    /// ([`Self::source_row_provenance`]), dichiarato qui o no. Un nuovo
    /// percorso di rifiuto per riga si dichiara comunque qui.
    ///
    /// Dipende dalla config: `table.type_cast` solo per i target con
    /// conversione fallibile ed `errors` assente, `coerce` o `raise`; gli hash
    /// solo con `null_policy=error`; `table.formula` solo con
    /// `on_division_by_zero=error` (di default la divisione per zero vale
    /// null); `table.hmac_sha256` mai. Le geo sono
    /// quelle i cui kernel raccolgono diagnostica per riga.
    #[must_use]
    pub fn emits_row_diagnostics(&self, config: &serde_json::Value) -> bool {
        match self.family {
            Family::Table => match self.id {
                "table.flatten_json"
                | "table.date_extract"
                | "table.date_format"
                | "table.date_add"
                | "table.date_diff"
                | "table.timezone_convert"
                | "table.expression"
                | "table.assert_not_null"
                | "table.assert_unique"
                | "table.assert_range"
                | "table.assert_regex"
                | "table.assert_foreign_key" => true,
                "table.type_cast" => {
                    matches!(
                        config
                            .get("target_type")
                            .and_then(serde_json::Value::as_str),
                        Some(
                            "int"
                                | "float"
                                | "bool"
                                | "uint64"
                                | "date"
                                | "datetime"
                                | "date32"
                                | "timestamp_millis"
                                | "decimal128"
                        )
                    ) && matches!(
                        config.get("errors").and_then(serde_json::Value::as_str),
                        None | Some("coerce" | "raise")
                    )
                }
                // L'unico rifiuto per riga di `formula` e' la divisione per
                // zero, che di default vale null: solo con `error`.
                "table.formula" => matches!(
                    config
                        .get("on_division_by_zero")
                        .and_then(serde_json::Value::as_str),
                    Some("error")
                ),
                "table.md5_hash" | "table.sha256_hash" => matches!(
                    config
                        .get("null_policy")
                        .and_then(serde_json::Value::as_str),
                    Some("error")
                ),
                _ => false,
            },
            Family::Geo => matches!(
                self.id,
                "geo.from_wkt"
                    | "geo.centroid"
                    | "geo.convex_hull"
                    | "geo.envelope"
                    | "geo.buffer"
                    | "geo.simplify"
                    | "geo.boundary"
                    | "geo.point_on_surface"
                    | "geo.make_valid"
                    | "geo.reproject"
                    | "geo.affine_transform"
                    | "geo.translate"
                    | "geo.scale"
                    | "geo.rotate"
                    | "geo.concave_hull"
                    | "geo.densify"
                    | "geo.snap_to_grid"
                    | "geo.line_substring"
                    | "geo.line_interpolate_point"
                    | "geo.area"
                    | "geo.length"
                    | "geo.perimeter"
                    | "geo.vertex_count"
                    | "geo.to_wkt"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Il catalogo.
//
// Le voci sono raggruppate per famiglia e versione di estensione; i
// separatori dentro `CATALOG` sono l'unico indice. Gli alias legacy stanno
// in `ALIASES`.
//
// Criteri dei metadati geo, scelti conservativi:
// - `arity`: `BinaryOrdered` solo per le operazioni su due ingressi; due
//   colonne dello stesso ingresso restano `Unary`;
// - `execution_class`: da `result_shape` (1:1 -> Streaming, aggregazioni e
//   tessellazioni -> Blocking, overlay e join -> BinaryBlocking);
// - `cancellation_behavior`: `Cooperative` per i kernel streaming,
//   `BoundaryOnly` per i bloccanti, `NonInterruptible` per i kernel senza
//   punti di cancellazione (`make_valid`, `polygonize`, `split`,
//   `reproject`);
// - `result_shape`: quella che il runner rende, non quella del progetto
//   d'origine (`verifica_forma` in `plenora-io/tests/operazioni_doc.rs` la
//   prova sull'esempio di ogni scheda); gli abbinamenti delle binarie geo
//   sono `OneToMany`, le binarie con una riga per riga sinistra `OneToOne`;
// - `determinism`: `DefinedOrder` di default, `CanonicalOrder` per le
//   operazioni insiemistiche e le aggregazioni senza ordine, `InputOrder`
//   per `concat`.
// ---------------------------------------------------------------------------

include!("catalog_op.rs");

/// Controllo del macro `op!` (`catalog_op.rs`): chiavi opzionali in
/// qualsiasi ordine, e una chiave ripetuta che non compila invece di valere
/// l'ultima. Non è usata: porta i due doctest.
///
/// ```
/// use plenora_core::catalog::*;
/// include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog_op.rs"));
/// const VOCE: OperationDescriptor = op!(
///     "table.prova", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[],
///     DefinedOrder, PublicProtocol, kernel_version = 4, semantic_version = 3,
///     expansion_constraint = LeftRelative
/// );
/// fn main() {
///     assert_eq!((VOCE.semantic_version, VOCE.config_schema_version), (3, 1));
///     assert_eq!(VOCE.kernel_version, 4);
///     assert_eq!(VOCE.expansion_constraint, ExpansionConstraint::LeftRelative);
///     assert_eq!(VOCE.geo_fusion, GeoFusion::NotFusible);
/// }
/// ```
///
/// La stessa voce con `kernel_version` ripetuta:
///
/// ```compile_fail
/// use plenora_core::catalog::*;
/// include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog_op.rs"));
/// const VOCE: OperationDescriptor = op!(
///     "table.prova", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[],
///     DefinedOrder, PublicProtocol, kernel_version = 4, semantic_version = 3,
///     expansion_constraint = LeftRelative, kernel_version = 5
/// );
/// fn main() {}
/// ```
///
/// `expansion_constraint` ripetuto, la seconda volta con `Custom`:
///
/// ```compile_fail
/// use plenora_core::catalog::*;
/// include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog_op.rs"));
/// const VOCE: OperationDescriptor = op!(
///     "table.prova", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[],
///     DefinedOrder, PublicProtocol, expansion_constraint = LeftRelative,
///     expansion_constraint = Custom(2.0)
/// );
/// fn main() {}
/// ```
///
/// `geo_fusion` ripetuta:
///
/// ```compile_fail
/// use plenora_core::catalog::*;
/// include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog_op.rs"));
/// const VOCE: OperationDescriptor = op!(
///     "table.prova", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[],
///     DefinedOrder, PublicProtocol, geo_fusion = TransformInPlace, geo_fusion = NotFusible
/// );
/// fn main() {}
/// ```
///
/// Una chiave sconosciuta:
///
/// ```compile_fail
/// use plenora_core::catalog::*;
/// include!(concat!(env!("CARGO_MANIFEST_DIR"), "/src/catalog_op.rs"));
/// const VOCE: OperationDescriptor = op!(
///     "table.prova", Table, Extension, Unary, Blocking, BoundaryOnly, None, None, &[],
///     DefinedOrder, PublicProtocol, kernel_version = 2, colore = 1
/// );
/// fn main() {}
/// ```
const _CONTROLLO_OP: () = ();

/// Catalogo unificato delle operazioni, tabellari e geografiche.
pub static CATALOG: &[OperationDescriptor] = &[
    // --- Tabellari Manipola-compat -----------------------------------
    op!(
        "table.add_row_number",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.aggregate",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.bin",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.concat",
        Table,
        ManipolaCompat,
        NAry,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        InputOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.concat_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.conditional",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.cross_join",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = SumRelative,
        semantic_version = 2
    ),
    op!(
        "table.date_extract",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.dedup_advanced",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        config_schema_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.distinct",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.drop_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.fill_na",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.filter",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.flatten_json",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.formula",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 3,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    // join generico: molti-a-molti possibile. Vincolo `SumRelative` come
    // ogni operazione a due ingressi (vedi `ExpansionConstraint`).
    op!(
        "table.join",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = SumRelative,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.lookup",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        contract_analysis_version = 2
    ),
    op!(
        "table.melt",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.pivot",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.rename",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.reorder_columns",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.replace",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.sample",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.sort",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.split_column",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.statistics",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.string_extract",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 3
    ),
    op!(
        "table.string_length",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.string_pad",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    // diff: l'uscita (added/removed/changed) è proporzionale a entrambi gli
    // ingressi -> SumRelative.
    op!(
        "table.table_diff",
        Table,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = SumRelative,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.text_normalize",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.transpose",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.type_cast",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.uuid_generator",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.window_function",
        Table,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.mask_data",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.md5_hash",
        Table,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    // --- Tabellari estensioni -----------------------------------------
    // anti_join: output <= left -> LeftRelative.
    op!(
        "table.anti_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    // asof_join: una corrispondenza per riga left (lookup-style) -> LeftRelative.
    op!(
        "table.asof_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = LeftRelative,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_not_null",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_range",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.assert_regex",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_schema",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    op!(
        "table.assert_unique",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.coalesce",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2
    ),
    op!(
        "table.date_add",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.date_diff",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    op!(
        "table.date_format",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    // except: output <= left -> LeftRelative.
    op!(
        "table.except",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.explode",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 2
    ),
    // intersect: output <= left (e <= right) -> LeftRelative.
    op!(
        "table.intersect",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.rolling_window",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        kernel_version = 3
    ),
    // semi_join: output <= left -> LeftRelative.
    op!(
        "table.semi_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "table.sha256_hash",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 3
    ),
    op!(
        "table.timezone_convert",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 4
    ),
    // union_distinct: output <= left + right -> SumRelative (esplicito).
    op!(
        "table.union_distinct",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        CanonicalOrder,
        PublicProtocol,
        kernel_version = 2,
        expansion_constraint = SumRelative
    ),
    op!(
        "table.unnest",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    // `table.expression`: la grammatica comprende substring, regex_replace,
    // between, in, greatest, least, floor, ceil e power, e `date_trunc` rende
    // Date32/TimestampMs nativi. Ciascuna di quelle capacità è osservabile
    // da fuori, quindi tutte e quattro le componenti di versione sono
    // dichiarate esplicitamente invece di restare al default.
    op!(
        "table.expression",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 4,
        config_schema_version = 3,
        contract_analysis_version = 3,
        kernel_version = 5
    ),
    op!(
        "table.assert_cardinality",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.assert_metadata",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol
    ),
    // assert_foreign_key: validazione, output = left -> LeftRelative.
    op!(
        "table.assert_foreign_key",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        semantic_version = 2,
        kernel_version = 3,
        expansion_constraint = LeftRelative
    ),
    // reconcile: sempre cinque righe, una per metrica, qualunque siano gli
    // ingressi: esente dal fattore d'espansione, che da due tabelle vuote (o
    // con un fattore sotto 2,5 da una riga per lato) rifiutava il resoconto.
    // Semantica 2, analisi 2.
    op!(
        "table.reconcile",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    // --- Geografiche Manipola-compat -----------------------------------
    op!(
        "geo.centroid",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.convex_hull",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.envelope",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        PublicProtocol,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    // sjoin: una geometria sinistra può intersecarne molte destre
    // (molti-a-molti); vincolo `SumRelative` (vedi `ExpansionConstraint`).
    op!(
        "geo.sjoin",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        PublicProtocol,
        expansion_constraint = SumRelative,
        semantic_version = 2
    ),
    op!(
        "geo.area",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.boundary",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.bounds_extractor",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.buffer",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    // Una riga d'uscita per riga, nella stessa posizione (una riga coperta
    // dalle precedenti diventa null): 1:1. Il progetto d'origine dichiarava
    // 1:N, piu' largo del kernel; analisi 2 per la forma corretta.
    op!(
        "geo.clean_topology",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2
    ),
    // clip e le quattro booleane allineate: una sola geometria (anche Multi)
    // per riga sinistra, nella sua posizione -> 1:1 e `LeftRelative`. Il
    // progetto d'origine dichiarava `OneToMany` e `MaxRelative`, piu' larghi
    // del kernel: `MaxRelative` rifiutava un ritaglio di piu' di
    // `max_expansion_factor` righe su una maschera di una riga (semantica 2
    // per clip: quell'uscita ora si produce). Analisi 2 per la forma.
    op!(
        "geo.clip",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    // count_points_in_polygons e within: una colonna in piu' sulla sinistra,
    // una riga per riga sinistra -> 1:1 (il progetto d'origine dichiarava
    // 1:N); analisi 2 per la forma.
    op!(
        "geo.count_points_in_polygons",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "geo.difference",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    // dissolve, line_builder, polygon_builder: sempre una riga, anche da una
    // tabella vuota. Il numero di righe non dipende dall'ingresso: esenti dal
    // fattore d'espansione, che da zero righe rifiutava la riga dichiarata
    // (`ResourceLimit`). Semantica 2 (la tabella vuota ora da' la sua
    // riga), analisi 2.
    op!(
        "geo.dissolve",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        CanonicalOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "geo.distance",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.explode",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.from_coords",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::FromCoords),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.intersection",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "geo.length",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.line_builder",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    // nearest: per ogni riga sinistra tutte le righe destre alla distanza
    // minima (a pari distanza più righe); uscita guidata dal lato sinistro
    // -> LeftRelative.
    op!(
        "geo.nearest",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = LeftRelative
    ),
    // overlay: un left puo' produrre piu' pezzi (OneToMany).
    op!(
        "geo.overlay",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = SumRelative,
        semantic_version = 2
    ),
    op!(
        "geo.perimeter",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    op!(
        "geo.point_on_surface",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.polygon_builder",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "geo.simplify",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.symmetric_difference",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "geo.to_wkt",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    // union: riga `i` con riga `i` (lati con le stesse righe), come le altre
    // booleane allineate. Dichiarava `SumRelative` (uscita / (sinistra +
    // destra) = 1/2): con `max_expansion_factor` fra 1/2 e 1 il passo era
    // accettato e ora si rifiuta, quindi semantica 2. Per difference,
    // intersection e symmetric_difference `MaxRelative` e `LeftRelative`
    // decidono uguale (lati di righe uguali, uscita = sinistra): semantica 1.
    op!(
        "geo.union",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    op!(
        "geo.vertex_count",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TerminalMeasure,
        semantic_version = 2
    ),
    // Triangolazione caricata in blocco (spade `bulk_load`) al posto
    // dell'inserimento incrementale di `geo`: kernel 2; celle a qualche ulp
    // da prima e rifiuti espliciti di precisione (`PrecisionInsufficient`,
    // `VerticeMalCondizionato`): semantica 2. Una cella per riga, nella
    // stessa posizione: 1:1 (il progetto d'origine dichiarava 1:N, piu'
    // largo del kernel); analisi 2 per la forma corretta.
    op!(
        "geo.voronoi",
        Geo,
        ManipolaCompat,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "geo.within",
        Geo,
        ManipolaCompat,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        contract_analysis_version = 2,
        expansion_constraint = LeftRelative
    ),
    // `make_valid` è TransformInPlace come le altre trasformazioni 1:1 sulla
    // stessa colonna: che accetti ingressi OGC-invalidi è una proprietà
    // della sua decodifica, non del raggruppamento.
    //
    // Backend Rust puro (`plenora_kernels_geo::rust_backend`) al posto di
    // GEOS: nessuna capability richiesta, `kernel_version` 2 per il cambio
    // di kernel. Resta `NonInterruptible`: il kernel non ha punti di
    // cancellazione. Stesso trattamento per `polygonize` e `split`.
    op!(
        "geo.make_valid",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2,
        kernel_version = 2
    ),
    // Riproiezione in Rust puro (`plenora_core::crs::riproiezione`,
    // `plenora_kernels_geo::riproiezione`) al posto di PROJ: nessuna
    // capability, `kernel_version` 2 per il cambio di kernel e
    // `config_schema_version` 2 per i parametri nuovi
    // (`accuratezza_accettata_m`, `trasformazioni`, `griglie`). Resta
    // `NonInterruptible` come in `plenora-data-tools@190c493`: il kernel non
    // ha punti di cancellazione.
    op!(
        "geo.reproject",
        Geo,
        ManipolaCompat,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Reprojection),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2,
        config_schema_version = 2,
        kernel_version = 2
    ),
    // --- Predicati DE-9IM, estensioni geo ------------------------------
    op!(
        "geo.predicate_intersects",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_disjoint",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_contains",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_within",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_equals_topo",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_covers",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_covered_by",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_contains_properly",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_touches",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_crosses",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.predicate_overlaps",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo -------------------------------------------------
    op!(
        "geo.affine_transform",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.translate",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.scale",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.rotate",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.concave_hull",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.hausdorff_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.haversine_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_line_length",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.densify",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    op!(
        "geo.snap_to_grid",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        geo_fusion = TransformInPlace,
        semantic_version = 2
    ),
    // Triangolazione caricata in blocco (spade `bulk_load`): kernel 2; ordine
    // d'uscita canonico (triangoli per prima comparsa dei vertici, ognuno dal
    // vertice comparso per primo) al posto dell'ordine interno di spade, e
    // su ingressi degeneri un'altra triangolazione valida: semantica 2.
    op!(
        "geo.delaunay",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2,
        kernel_version = 2
    ),
    op!(
        "geo.polygonize",
        Geo,
        Extension,
        Unary,
        Blocking,
        NonInterruptible,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    // line_merge e polygonize: dall'intera tabella, una riga per percorso o
    // per faccia, anche piu' delle righe d'ingresso -> `WholeToMany`, esenti
    // dal fattore d'espansione (restano sotto il limite di righe dell'arco,
    // che il kernel riceve). Il progetto d'origine dichiarava N:1, piu'
    // stretto del kernel: una riga di linee disgiunte ne da' piu' d'una.
    // Semantica 2 (un'uscita oltre il fattore ora si produce), analisi 2.
    op!(
        "geo.line_merge",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "geo.split",
        Geo,
        Extension,
        Unary,
        Streaming,
        NonInterruptible,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        kernel_version = 2
    ),
    op!(
        "geo.line_substring",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.line_interpolate_point",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.frechet_distance",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.bearing",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.geodesic_area",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Geographic),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 2
    ),
    op!(
        "geo.geometry_diagnostics",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::Diagnostic),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.1 ---------------------------------------------
    op!(
        "geo.from_wkt",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::FromCoords),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        semantic_version = 3,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "geo.geometry_accessors",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "geo.collect",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::ManyToOne),
        Some(CrsRequirement::Known),
        &[],
        CanonicalOrder,
        KernelValidated
    ),
    op!(
        "geo.line_locate_point",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.2 ---------------------------------------------
    op!(
        "geo.generate_grid",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "geo.subdivide",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToMany),
        Some(CrsRequirement::Known),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // `snap`: il riferimento da config (`reference_wkb`) si assume nello
    // stesso CRS dell'ingresso, come ogni geometria letterale di una config:
    // requisito SameProjected per l'unica colonna, come le distanze
    // "unarie".
    op!(
        "geo.snap",
        Geo,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni geo v1.3 ---------------------------------------------
    // Coperture poligonali (piantine di edifici): entrambe consumano l'intero
    // input (Blocking) e producono una riga per issue/tratto condiviso
    // (WholeToMany, schema nuovo); aree e lunghezze in unità di mappa,
    // quindi SameProjected. Esenti da `max_expansion_factor`, per
    // dichiarazione di catalogo.
    op!(
        "geo.coverage_validate",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true
    ),
    op!(
        "geo.shared_paths",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::WholeToMany),
        Some(CrsRequirement::SameProjected),
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true
    ),
    // `cluster_dbscan`: clustering globale per densita' (vicinati R-tree
    // sull'intero input) ma output allineato alle righe (un'etichetta UInt64
    // nullable per riga, noise -> null): Blocking con shape OneToOne; eps in
    // unita' di mappa, quindi Projected.
    op!(
        "geo.cluster_dbscan",
        Geo,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        Some(ResultShape::OneToOne),
        Some(CrsRequirement::Projected),
        &[],
        DefinedOrder,
        KernelValidated
    ),
    // --- Estensioni table v1.1 -------------------------------------------
    op!(
        "table.limit",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        InputOrder,
        KernelValidated,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    op!(
        "table.select_columns",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated
    ),
    op!(
        "table.stable_fingerprint",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        kernel_version = 2
    ),
    op!(
        "table.top_n",
        Table,
        Extension,
        Unary,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        config_schema_version = 2,
        contract_analysis_version = 2,
        kernel_version = 2
    ),
    // --- Estensioni table v1.2 -------------------------------------------
    op!(
        "table.align_schema",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        config_schema_version = 2,
        contract_analysis_version = 2
    ),
    op!(
        "table.concat_by_name",
        Table,
        Extension,
        NAry,
        Blocking,
        BoundaryOnly,
        None,
        None,
        &[],
        InputOrder,
        KernelValidated
    ),
    op!(
        "table.hmac_sha256",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        kernel_version = 3
    ),
    // validate_rules: `annotate` 1:1, `summary` una riga per regola, anche da
    // un ingresso vuoto: righe fissate dalla config, non dagli ingressi.
    // Esente dal fattore d'espansione, che da una tabella vuota (o con piu'
    // regole che `max_expansion_factor` volte le righe) rifiutava il
    // riepilogo. Semantica 2, analisi 2.
    op!(
        "table.validate_rules",
        Table,
        Extension,
        Unary,
        Streaming,
        Cooperative,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_factor_exempt = true,
        semantic_version = 2,
        contract_analysis_version = 2
    ),
    // --- Estensioni table v1.3 -------------------------------------------
    // fuzzy_join: build/probe sui blocchi (prefix/soundex) come i join
    // esatti, ma scoring per coppia candidata -> BinaryBlocking; ordine di
    // output definito (scansione sinistra, indice destro). Piu' candidati
    // per riga left possibili (vincolo `SumRelative`).
    op!(
        "table.fuzzy_join",
        Table,
        Extension,
        BinaryOrdered,
        BinaryBlocking,
        BoundaryOnly,
        None,
        None,
        &[],
        DefinedOrder,
        KernelValidated,
        expansion_constraint = SumRelative,
        semantic_version = 2
    ),
];

/// Alias legacy degli id, dai piani e dal protocollo del progetto d'origine.
///
/// Forma: `(schema_version, legacy_alias, canonical_id)`. Un alias
/// introdotto non si riassegna mai a un altro id. `schema_version` 3 copre
/// sia i piani tabellari legacy sia gli id storici del protocollo geo
/// (v2/v3, `TransformArrowSchema`). [`find_operation`] li risolve; il
/// runner li rifiuta e vuole l'id canonico.
pub static ALIASES: &[(u16, &str, &str)] = &[
    // --- Piani nogeo legacy: id storico -> table.<id> -----------------
    (3, "add_row_number", "table.add_row_number"),
    (3, "aggregate", "table.aggregate"),
    (3, "bin", "table.bin"),
    (3, "concat", "table.concat"),
    (3, "concat_columns", "table.concat_columns"),
    (3, "conditional", "table.conditional"),
    (3, "cross_join", "table.cross_join"),
    (3, "date_extract", "table.date_extract"),
    (3, "dedup_advanced", "table.dedup_advanced"),
    (3, "distinct", "table.distinct"),
    (3, "drop_columns", "table.drop_columns"),
    (3, "fill_na", "table.fill_na"),
    (3, "filter", "table.filter"),
    (3, "flatten_json", "table.flatten_json"),
    (3, "formula", "table.formula"),
    (3, "join", "table.join"),
    (3, "lookup", "table.lookup"),
    (3, "melt", "table.melt"),
    (3, "pivot", "table.pivot"),
    (3, "rename", "table.rename"),
    (3, "reorder_columns", "table.reorder_columns"),
    (3, "replace", "table.replace"),
    (3, "sample", "table.sample"),
    (3, "sort", "table.sort"),
    (3, "split_column", "table.split_column"),
    (3, "statistics", "table.statistics"),
    (3, "string_extract", "table.string_extract"),
    (3, "string_length", "table.string_length"),
    (3, "string_pad", "table.string_pad"),
    (3, "table_diff", "table.table_diff"),
    (3, "text_normalize", "table.text_normalize"),
    (3, "transpose", "table.transpose"),
    (3, "type_cast", "table.type_cast"),
    (3, "uuid_generator", "table.uuid_generator"),
    (3, "window_function", "table.window_function"),
    (3, "mask_data", "table.mask_data"),
    (3, "md5_hash", "table.md5_hash"),
    (3, "anti_join", "table.anti_join"),
    (3, "asof_join", "table.asof_join"),
    (3, "assert_not_null", "table.assert_not_null"),
    (3, "assert_range", "table.assert_range"),
    (3, "assert_regex", "table.assert_regex"),
    (3, "assert_schema", "table.assert_schema"),
    (3, "assert_unique", "table.assert_unique"),
    (3, "coalesce", "table.coalesce"),
    (3, "date_add", "table.date_add"),
    (3, "date_diff", "table.date_diff"),
    (3, "date_format", "table.date_format"),
    (3, "except", "table.except"),
    (3, "explode", "table.explode"),
    (3, "intersect", "table.intersect"),
    (3, "rolling_window", "table.rolling_window"),
    (3, "semi_join", "table.semi_join"),
    (3, "sha256_hash", "table.sha256_hash"),
    (3, "timezone_convert", "table.timezone_convert"),
    (3, "union_distinct", "table.union_distinct"),
    (3, "unnest", "table.unnest"),
    (3, "expression", "table.expression"),
    (3, "assert_cardinality", "table.assert_cardinality"),
    (3, "assert_metadata", "table.assert_metadata"),
    (3, "assert_foreign_key", "table.assert_foreign_key"),
    (3, "reconcile", "table.reconcile"),
    // --- Id geo storici: geo_* -> geo.<id senza prefisso> -------------
    (3, "geo_centroid", "geo.centroid"),
    (3, "geo_convex_hull", "geo.convex_hull"),
    (3, "geo_envelope", "geo.envelope"),
    (3, "geo_area", "geo.area"),
    (3, "geo_boundary", "geo.boundary"),
    (3, "geo_bounds_extractor", "geo.bounds_extractor"),
    (3, "geo_buffer", "geo.buffer"),
    (3, "geo_clean_topology", "geo.clean_topology"),
    (3, "geo_clip", "geo.clip"),
    (
        3,
        "geo_count_points_in_polygons",
        "geo.count_points_in_polygons",
    ),
    (3, "geo_difference", "geo.difference"),
    (3, "geo_dissolve", "geo.dissolve"),
    (3, "geo_distance", "geo.distance"),
    (3, "geo_explode", "geo.explode"),
    (3, "geo_from_coords", "geo.from_coords"),
    (3, "geo_intersection", "geo.intersection"),
    (3, "geo_length", "geo.length"),
    (3, "geo_line_builder", "geo.line_builder"),
    (3, "geo_nearest", "geo.nearest"),
    (3, "geo_overlay", "geo.overlay"),
    (3, "geo_perimeter", "geo.perimeter"),
    (3, "geo_point_on_surface", "geo.point_on_surface"),
    (3, "geo_polygon_builder", "geo.polygon_builder"),
    (3, "geo_simplify", "geo.simplify"),
    (3, "geo_symmetric_difference", "geo.symmetric_difference"),
    (3, "geo_to_wkt", "geo.to_wkt"),
    (3, "geo_union", "geo.union"),
    (3, "geo_vertex_count", "geo.vertex_count"),
    (3, "geo_voronoi", "geo.voronoi"),
    (3, "geo_within", "geo.within"),
    (3, "geo_make_valid", "geo.make_valid"),
    (3, "geo_reproject", "geo.reproject"),
    // --- Predicati DE-9IM: id invariato sotto geo. --------------------
    (3, "predicate_intersects", "geo.predicate_intersects"),
    (3, "predicate_disjoint", "geo.predicate_disjoint"),
    (3, "predicate_contains", "geo.predicate_contains"),
    (3, "predicate_within", "geo.predicate_within"),
    (3, "predicate_equals_topo", "geo.predicate_equals_topo"),
    (3, "predicate_covers", "geo.predicate_covers"),
    (3, "predicate_covered_by", "geo.predicate_covered_by"),
    (
        3,
        "predicate_contains_properly",
        "geo.predicate_contains_properly",
    ),
    (3, "predicate_touches", "geo.predicate_touches"),
    (3, "predicate_crosses", "geo.predicate_crosses"),
    (3, "predicate_overlaps", "geo.predicate_overlaps"),
    // --- Estensioni geo nude: <id> -> geo.<id> ------------------------
    (3, "sjoin", "geo.sjoin"),
    (3, "affine_transform", "geo.affine_transform"),
    (3, "translate", "geo.translate"),
    (3, "scale", "geo.scale"),
    (3, "rotate", "geo.rotate"),
    (3, "concave_hull", "geo.concave_hull"),
    (3, "hausdorff_distance", "geo.hausdorff_distance"),
    (3, "haversine_distance", "geo.haversine_distance"),
    (3, "geodesic_distance", "geo.geodesic_distance"),
    (3, "geodesic_line_length", "geo.geodesic_line_length"),
    (3, "densify", "geo.densify"),
    (3, "snap_to_grid", "geo.snap_to_grid"),
    (3, "delaunay", "geo.delaunay"),
    (3, "polygonize", "geo.polygonize"),
    (3, "line_merge", "geo.line_merge"),
    (3, "split", "geo.split"),
    (3, "line_substring", "geo.line_substring"),
    (3, "line_interpolate_point", "geo.line_interpolate_point"),
    (3, "frechet_distance", "geo.frechet_distance"),
    (3, "bearing", "geo.bearing"),
    (3, "geodesic_area", "geo.geodesic_area"),
    (3, "geometry_diagnostics", "geo.geometry_diagnostics"),
];

/// Risolve un alias legacy per una data `schema_version` verso l'id
/// canonico; `None` se l'alias non esiste per quella versione.
#[must_use]
pub fn resolve_alias(schema_version: u16, alias: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(version, a, _)| *version == schema_version && *a == alias)
        .map(|(_, _, canonical)| *canonical)
}

/// Cerca un'operazione per id canonico o alias legacy.
///
/// Per un alias la `schema_version` non è nota al chiamante e si usa la
/// prima voce di tabella corrispondente. `None` se l'id è sconosciuto.
#[must_use]
pub fn find_operation(id: &str) -> Option<&'static OperationDescriptor> {
    CATALOG.iter().find(|op| op.id == id).or_else(|| {
        ALIASES
            .iter()
            .find(|(_, alias, _)| *alias == id)
            .and_then(|(_, _, canonical)| CATALOG.iter().find(|op| op.id == *canonical))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn catalog_has_146_unique_ids() {
        // 146, come in plenora-data-tools@190c493: `geo.reproject` è tornata
        // con la riproiezione in Rust puro.
        assert_eq!(CATALOG.len(), 146);
        let ids: HashSet<_> = CATALOG.iter().map(|op| op.id).collect();
        assert_eq!(ids.len(), CATALOG.len());
        assert_eq!(
            CATALOG
                .iter()
                .filter(|op| op.family == Family::Table)
                .count(),
            71
        );
        assert_eq!(
            CATALOG.iter().filter(|op| op.family == Family::Geo).count(),
            75
        );
    }

    #[test]
    fn every_alias_resolves_to_an_existing_catalog_id() {
        assert_eq!(ALIASES.len(), 127);
        for (schema_version, alias, canonical) in ALIASES {
            assert!(
                CATALOG.iter().any(|op| op.id == *canonical),
                "alias {alias} punta a un id assente: {canonical}"
            );
            assert_eq!(
                resolve_alias(*schema_version, alias),
                Some(*canonical),
                "resolve_alias fallisce per {alias}"
            );
        }
    }

    #[test]
    fn no_alias_collides_with_a_canonical_id_of_a_different_family() {
        for (_, alias, canonical) in ALIASES {
            let target = CATALOG
                .iter()
                .find(|op| op.id == *canonical)
                .expect("alias risolto nel test precedente");
            if let Some(conflicting) = CATALOG.iter().find(|op| op.id == *alias) {
                assert_eq!(
                    conflicting.family, target.family,
                    "alias {alias} collide con id canonico di altra famiglia"
                );
            }
        }
    }

    #[test]
    fn crs_requirement_implies_geo_family() {
        for op in CATALOG {
            if op.crs_requirement.is_some() {
                assert_eq!(op.family, Family::Geo, "{} non e' geo", op.id);
            }
            if op.family == Family::Table {
                assert!(op.result_shape.is_none(), "{} tabellare con shape", op.id);
                assert!(
                    op.required_capabilities.is_empty(),
                    "{} tabellare con capability",
                    op.id
                );
            }
        }
    }

    #[test]
    fn find_operation_accepts_canonical_ids_and_aliases() {
        assert_eq!(
            find_operation("table.filter").map(|op| op.id),
            Some("table.filter")
        );
        assert_eq!(
            find_operation("filter").map(|op| op.id),
            Some("table.filter")
        );
        assert_eq!(
            find_operation("geo_buffer").map(|op| op.id),
            Some("geo.buffer")
        );
        assert_eq!(
            find_operation("translate").map(|op| op.id),
            Some("geo.translate")
        );
        assert!(find_operation("nonexistent_op").is_none());
    }

    #[test]
    fn versions_default_to_one_and_expression_versions_are_explicit() {
        // Default: tutte e 4 le componenti a 1 per le op senza incrementi.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(filter.semantic_version, 1);
        assert_eq!(filter.config_schema_version, 2);
        assert_eq!(filter.contract_analysis_version, 2);
        assert_eq!(filter.kernel_version, 3);
        let select = find_operation("table.select_columns").expect("table.select_columns");
        assert_eq!(
            (
                select.semantic_version,
                select.config_schema_version,
                select.contract_analysis_version,
                select.kernel_version
            ),
            (1, 1, 1, 1)
        );
        // Le 4 componenti di table.expression restano esplicite e indipendenti
        // (diagnostica per riga, poi la divisione per zero che vale null, i
        // campi dei nodi rifiutati e i limiti dei letterali).
        let expression = find_operation("table.expression").expect("table.expression");
        assert_eq!(expression.semantic_version, 4);
        assert_eq!(expression.config_schema_version, 3);
        assert_eq!(expression.contract_analysis_version, 3);
        assert_eq!(expression.kernel_version, 5);
        // Nessuna versione puo' essere 0 in tutto il catalogo.
        for op in CATALOG {
            assert!(op.semantic_version >= 1, "{} semantic_version", op.id);
            assert!(
                op.config_schema_version >= 1,
                "{} config_schema_version",
                op.id
            );
            assert!(
                op.contract_analysis_version >= 1,
                "{} contract_analysis_version",
                op.id
            );
            assert!(op.kernel_version >= 1, "{} kernel_version", op.id);
        }
    }

    // Nessuna operazione è oggi `BackendPending`: il test fissa la regola per
    // una futura operazione dietro un backend esterno.
    #[test]
    fn backend_pending_ops_declare_their_capability() {
        for op in CATALOG {
            if op.maturity == Maturity::BackendPending {
                assert!(
                    !op.required_capabilities.is_empty(),
                    "{} backend_pending senza capability",
                    op.id
                );
                assert_eq!(
                    op.cancellation_behavior,
                    CancellationBehavior::NonInterruptible,
                    "{} backend_pending interrompibile",
                    op.id
                );
            }
        }
    }

    #[test]
    fn expansion_constraint_defaults_to_sum_relative() {
        // Le operazioni senza dichiarazione esplicita restano sulla base
        // sinistra + destra, su cui sono tarate le soglie, e non sono esenti.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(
            filter.expansion_constraint,
            ExpansionConstraint::SumRelative
        );
        assert!(!filter.expansion_factor_exempt);
        let reconcile = find_operation("table.reconcile").expect("table.reconcile");
        assert_eq!(
            reconcile.expansion_constraint,
            ExpansionConstraint::SumRelative
        );
    }

    #[test]
    fn row_provenance_audit_is_fail_closed_for_cardinality_and_order_changes() {
        for id in [
            "table.filter",
            "table.sample",
            "table.explode",
            "table.join",
            "table.aggregate",
            "table.sort",
            "table.melt",
            "table.pivot",
            "table.transpose",
            "table.table_diff",
            "table.top_n",
            "table.distinct",
            "table.dedup_advanced",
            "table.window_function",
            "table.concat",
            "table.concat_by_name",
            "table.cross_join",
            "table.fuzzy_join",
            "table.asof_join",
            "table.semi_join",
            "table.anti_join",
            "geo.collect",
            "geo.subdivide",
            "geo.sjoin",
            "geo.generate_grid",
        ] {
            assert_eq!(
                find_operation(id).map(OperationDescriptor::source_row_provenance),
                Some(SourceRowProvenance::Unavailable),
                "{id} non deve dichiarare provenance originale"
            );
        }
        for id in [
            "table.rename",
            "table.reorder_columns",
            "table.flatten_json",
            "table.type_cast",
            "table.bin",
            "table.add_row_number",
            "table.assert_unique",
            "table.assert_foreign_key",
            "geo.from_wkt",
        ] {
            assert_eq!(
                find_operation(id).map(OperationDescriptor::source_row_provenance),
                Some(SourceRowProvenance::Preserved),
                "{id} deve conservare posizione e cardinalita'"
            );
        }
    }

    #[test]
    fn binary_ops_declare_the_binding_constraint() {
        let expected: &[(&str, ExpansionConstraint)] = &[
            ("table.join", ExpansionConstraint::SumRelative),
            ("table.cross_join", ExpansionConstraint::SumRelative),
            ("table.fuzzy_join", ExpansionConstraint::SumRelative),
            ("table.table_diff", ExpansionConstraint::SumRelative),
            ("table.union_distinct", ExpansionConstraint::SumRelative),
            ("table.semi_join", ExpansionConstraint::LeftRelative),
            ("table.anti_join", ExpansionConstraint::LeftRelative),
            ("table.asof_join", ExpansionConstraint::LeftRelative),
            ("table.except", ExpansionConstraint::LeftRelative),
            ("table.intersect", ExpansionConstraint::LeftRelative),
            (
                "table.assert_foreign_key",
                ExpansionConstraint::LeftRelative,
            ),
            ("geo.sjoin", ExpansionConstraint::SumRelative),
            ("geo.clip", ExpansionConstraint::LeftRelative),
            ("geo.difference", ExpansionConstraint::LeftRelative),
            ("geo.intersection", ExpansionConstraint::LeftRelative),
            ("geo.overlay", ExpansionConstraint::SumRelative),
            (
                "geo.symmetric_difference",
                ExpansionConstraint::LeftRelative,
            ),
            ("geo.nearest", ExpansionConstraint::LeftRelative),
            ("geo.within", ExpansionConstraint::LeftRelative),
            (
                "geo.count_points_in_polygons",
                ExpansionConstraint::LeftRelative,
            ),
            ("geo.union", ExpansionConstraint::LeftRelative),
        ];
        for (id, constraint) in expected {
            let op = find_operation(id).expect(id);
            assert_eq!(
                op.arity,
                Arity::BinaryOrdered,
                "{id}: vincolo dichiarato su op non binaria"
            );
            assert_eq!(op.expansion_constraint, *constraint, "{id}");
        }
    }

    #[test]
    fn whole_to_many_exemption_is_declared_in_catalog() {
        // La classe di esenzione è dichiarata in catalogo, non riconosciuta
        // guardando l'uscita: le operazioni WholeToMany (generative,
        // diagnostiche, fusioni dell'intera tabella) e quelle con un numero
        // di righe fisso, che non dipende dall'ingresso.
        let exempt: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.expansion_factor_exempt)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            exempt,
            HashSet::from([
                "geo.generate_grid",
                "geo.coverage_validate",
                "geo.shared_paths",
                "geo.line_merge",
                "geo.polygonize",
                "geo.dissolve",
                "geo.line_builder",
                "geo.polygon_builder",
                "table.reconcile",
                "table.validate_rules",
            ])
        );
        // Righe fisse: una per `dissolve` e i costruttori, cinque per
        // `reconcile`, una per regola per `validate_rules` in `summary`.
        let righe_fisse = [
            "geo.dissolve",
            "geo.line_builder",
            "geo.polygon_builder",
            "table.reconcile",
            "table.validate_rules",
        ];
        for op in CATALOG {
            assert_eq!(
                op.expansion_factor_exempt,
                op.result_shape == Some(ResultShape::WholeToMany) || righe_fisse.contains(&op.id),
                "{}: esenzione non allineata alla shape WholeToMany o alle righe fisse",
                op.id
            );
        }
    }

    #[test]
    // Confronti float esatti intenzionali: le metriche sono rapporti di
    // piccoli interi con rappresentazione binaria esatta (es. 6/5, 6/3);
    // il test verifica il valore per costruzione, non un'approssimazione.
    #[allow(clippy::float_cmp)]
    fn join_expansion_binding_metric_selects_the_declared_constraint() {
        let expansion = JoinExpansion::compute(6, 3, 2);
        assert_eq!(expansion.output_over_sum_inputs, 1.2);
        assert_eq!(expansion.output_over_left, 2.0);
        assert_eq!(expansion.output_over_right, 3.0);
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::SumRelative),
            1.2
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::LeftRelative),
            2.0
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::RightRelative),
            3.0
        );
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::MaxRelative),
            3.0
        );
        // Denominatore nullo: infinito se l'output e' non nullo, zero se
        // anche l'output e' nullo.
        let from_empty = JoinExpansion::compute(1, 0, 0);
        assert!(from_empty.output_over_left.is_infinite());
        assert!(from_empty.output_over_sum_inputs.is_infinite());
        let all_empty = JoinExpansion::compute(0, 0, 0);
        assert_eq!(
            all_empty.binding_metric(ExpansionConstraint::MaxRelative),
            0.0
        );
    }

    #[test]
    fn la_decisione_binaria_e_esatta_anche_dove_le_metriche_arrotondano() {
        // `left = right = 2^53`, `output = 2^53+1`,
        // fattore 1: il rapporto reale e' > 1, ma `output as f64` arrotonda a
        // 2^53 e la metrica diventa esattamente 1.0. Se decidesse la
        // metrica, il limite NON scatterebbe.
        const DUE_53: u64 = 1 << 53;
        let output = DUE_53 + 1;
        let metrica = JoinExpansion::compute(output, DUE_53, DUE_53);
        // La metrica osservabile resta arrotondata: per questo non decide.
        assert!(
            metrica.binding_metric(ExpansionConstraint::MaxRelative) <= 1.0,
            "il rapporto in f64 dovrebbe collassare su 1.0"
        );
        // La decisione, invece, e' esatta.
        assert!(
            ExpansionConstraint::MaxRelative.exceeded(output, DUE_53, DUE_53, 1.0),
            "un'espansione oltre la soglia deve essere rifiutata anche sopra 2^53"
        );
        // E non e' diventata rigida: la stessa cardinalita' pari alla soglia
        // resta accettata, in tutte le basi.
        for constraint in [
            ExpansionConstraint::MaxRelative,
            ExpansionConstraint::LeftRelative,
            ExpansionConstraint::RightRelative,
        ] {
            assert!(
                !constraint.exceeded(DUE_53, DUE_53, DUE_53, 1.0),
                "{constraint:?}: output uguale alla soglia non e' un superamento"
            );
        }
        // Base «somma degli input»: la somma non deve saturare prima del
        // confronto. Con due lati a `u64::MAX` la soglia vale 2^65, che
        // nessun output a 64 bit puo' superare.
        assert!(!ExpansionConstraint::SumRelative.exceeded(u64::MAX, u64::MAX, u64::MAX, 1.0));
        // Denominatore nullo, coerente con le metriche: output non nullo da
        // input vuoti e' espansione infinita, output nullo non lo e'.
        assert!(ExpansionConstraint::MaxRelative.exceeded(1, 0, 0, 1.0));
        assert!(!ExpansionConstraint::MaxRelative.exceeded(0, 0, 0, 1.0));
        // `Custom` sovrascrive la soglia, non la base.
        assert!(ExpansionConstraint::Custom(2.0).exceeded(7, 3, 0, 100.0));
        assert!(!ExpansionConstraint::Custom(2.0).exceeded(6, 3, 0, 100.0));
        // Un fattore frazionario resta esatto: 3 righe da 2 con soglia 1.5
        // e' il limite, 4 lo supera.
        assert!(!ExpansionConstraint::LeftRelative.exceeded(3, 2, 0, 1.5));
        assert!(ExpansionConstraint::LeftRelative.exceeded(4, 2, 0, 1.5));
    }

    #[test]
    // Come sopra: i confronti esatti sui fattori custom verificano
    // l'uguaglianza per bit di `PartialEq`.
    #[allow(clippy::float_cmp)]
    fn custom_constraint_overrides_the_threshold_not_the_metric() {
        // `Custom(fattore)` è la soglia propria di un'operazione la cui
        // uscita non ha una base fissa. La metrica vincolante resta
        // `output_over_sum_inputs`; il fattore sostituisce
        // `max_expansion_factor` come soglia per la singola operazione.
        let expansion = JoinExpansion::compute(6, 3, 2);
        assert_eq!(
            expansion.binding_metric(ExpansionConstraint::Custom(2.5)),
            expansion.output_over_sum_inputs
        );
        assert_eq!(ExpansionConstraint::Custom(2.5).binding_threshold(4.0), 2.5);
        assert_eq!(ExpansionConstraint::MaxRelative.binding_threshold(4.0), 4.0);
        // Uguaglianza per bit: nessuna ambiguita' float fra due costanti di catalogo.
        assert_eq!(
            ExpansionConstraint::Custom(2.5),
            ExpansionConstraint::Custom(2.5)
        );
        assert_ne!(
            ExpansionConstraint::Custom(2.5),
            ExpansionConstraint::Custom(2.6)
        );
        assert_ne!(
            ExpansionConstraint::Custom(1.2),
            ExpansionConstraint::SumRelative
        );

        // Sintassi della macro `op!`: `expansion_constraint = Custom(f)`
        // coesiste con le altre chiavi in qualunque ordine.
        let descriptor = op!(
            "table.__custom_test",
            Table,
            Extension,
            BinaryOrdered,
            BinaryBlocking,
            BoundaryOnly,
            None,
            None,
            &[],
            DefinedOrder,
            KernelValidated,
            expansion_constraint = Custom(2.5),
            kernel_version = 2
        );
        assert_eq!(
            descriptor.expansion_constraint,
            ExpansionConstraint::Custom(2.5)
        );
        assert_eq!(descriptor.kernel_version, 2);
    }

    #[test]
    fn geo_fusion_matches_the_adr_0012_perimeter() {
        // Perimetro fondibile (il nome del test ricorda la decisione del
        // progetto d'origine): le trasformazioni 1:1 sul posto (più
        // `reproject` e `make_valid`) sono TransformInPlace, le misure
        // terminali TerminalMeasure, tutto il resto (tabellari comprese)
        // NotFusible. La lista chiusa qui sotto è il contratto.
        let transforms: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.geo_fusion == GeoFusion::TransformInPlace)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            transforms,
            HashSet::from([
                "geo.buffer",
                "geo.simplify",
                "geo.centroid",
                "geo.convex_hull",
                "geo.envelope",
                "geo.boundary",
                "geo.point_on_surface",
                "geo.affine_transform",
                "geo.translate",
                "geo.scale",
                "geo.rotate",
                "geo.concave_hull",
                "geo.densify",
                "geo.snap_to_grid",
                "geo.reproject",
                "geo.make_valid",
            ])
        );
        let terminals: HashSet<_> = CATALOG
            .iter()
            .filter(|op| op.geo_fusion == GeoFusion::TerminalMeasure)
            .map(|op| op.id)
            .collect();
        assert_eq!(
            terminals,
            HashSet::from([
                "geo.area",
                "geo.length",
                "geo.perimeter",
                "geo.vertex_count",
                "geo.to_wkt",
            ])
        );
        for op in CATALOG {
            // Fuori dal perimetro della fusione: fonderle richiederebbe un
            // controllo di tipo per-riga, che il runner non fa.
            if matches!(op.id, "geo.line_substring" | "geo.line_interpolate_point") {
                assert_eq!(op.geo_fusion, GeoFusion::NotFusible, "{}", op.id);
            }
            // La fondibilita' riguarda solo la famiglia geo: ogni tabellare
            // resta NotFusible e nessuna tabellare puo' dichiararsi fondibile.
            if op.family == Family::Table {
                assert_eq!(
                    op.geo_fusion,
                    GeoFusion::NotFusible,
                    "{} tabellare fondibile",
                    op.id
                );
            }
            // Invariante della fusione: solo op unarie streaming possono fondersi.
            if op.geo_fusion != GeoFusion::NotFusible {
                assert_eq!(op.family, Family::Geo, "{}", op.id);
                assert_eq!(op.arity, Arity::Unary, "{}", op.id);
                assert_eq!(op.execution_class, ExecutionClass::Streaming, "{}", op.id);
            }
        }
        // Default di macro: senza chiave `geo_fusion` il campo e' NotFusible.
        let filter = find_operation("table.filter").expect("table.filter");
        assert_eq!(filter.geo_fusion, GeoFusion::NotFusible);
    }

    #[test]
    fn geo_fusion_names_are_stable_snake_case() {
        // Nomi stabili per contratto, mai derivati dal `Debug` di Rust.
        assert_eq!(GeoFusion::NotFusible.as_str(), "not_fusible");
        assert_eq!(GeoFusion::TransformInPlace.as_str(), "transform_in_place");
        assert_eq!(GeoFusion::TerminalMeasure.as_str(), "terminal_measure");
    }

    /// Config di sonda generiche per `emits_row_diagnostics`: applicate a
    /// TUTTE le operazioni (non sono una lista di operazioni, coprono lo
    /// spazio di config che conta: target di cast e policy null degli hash).
    fn row_diagnostics_probes() -> Vec<serde_json::Value> {
        let mut probes = vec![serde_json::json!({})];
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
            "str",
        ] {
            probes.push(serde_json::json!({"target_type": target}));
            probes.push(serde_json::json!({"target_type": target, "errors": "coerce"}));
            probes.push(serde_json::json!({"target_type": target, "errors": "raise"}));
        }
        for policy in ["error", "empty", "literal"] {
            probes.push(serde_json::json!({"null_policy": policy}));
        }
        probes.push(serde_json::json!({"on_division_by_zero": "error"}));
        probes
    }

    #[test]
    fn row_diagnostics_authority_locks_config_sensitive_operations() {
        let type_cast = find_operation("table.type_cast").expect("type_cast");
        for target in [
            "int",
            "float",
            "bool",
            "uint64",
            "date",
            "datetime",
            "date32",
            "timestamp_millis",
            "decimal128",
        ] {
            for errors in [None, Some("coerce"), Some("raise")] {
                let config = errors.map_or_else(
                    || serde_json::json!({"target_type": target}),
                    |mode| serde_json::json!({"target_type": target, "errors": mode}),
                );
                assert!(
                    type_cast.emits_row_diagnostics(&config),
                    "type_cast {target}/{errors:?} rifiuta righe: deve emettere"
                );
            }
        }
        // Target senza conversione fallibile row-scoped: nessuna emissione.
        assert!(!type_cast.emits_row_diagnostics(&serde_json::json!({"target_type": "str"})));
        assert!(!type_cast.emits_row_diagnostics(&serde_json::json!({})));

        // md5/sha256 rifiutano row-scoped solo con null_policy=error;
        // le altre policy hanno semantica storica dichiarata.
        for id in ["table.md5_hash", "table.sha256_hash"] {
            let hash = find_operation(id).expect(id);
            assert!(hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "error"})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "empty"})));
            assert!(!hash.emits_row_diagnostics(&serde_json::json!({"null_policy": "literal"})));
        }

        // hmac_sha256 non emette MAI (le null_policy legacy producono
        // output dichiarato, nessun rifiuto row-scoped).
        let hmac = find_operation("table.hmac_sha256").expect("hmac");
        for config in [
            serde_json::json!({}),
            serde_json::json!({"null_policy": "error"}),
            serde_json::json!({"null_policy": "empty"}),
            serde_json::json!({"null_policy": "null"}),
            serde_json::json!({"null_policy": "skip"}),
        ] {
            assert!(
                !hmac.emits_row_diagnostics(&config),
                "hmac_sha256 non deve mai emettere diagnostica row-scoped"
            );
        }

        // formula emette solo con `on_division_by_zero=error` (la divisione
        // per zero e' il suo unico rifiuto per riga, e di default vale null);
        // expression con qualunque configurazione (numeri non finiti).
        let formula = find_operation("table.formula").expect("formula");
        assert!(!formula.emits_row_diagnostics(&serde_json::json!({})));
        assert!(!formula.emits_row_diagnostics(&serde_json::json!({"on_division_by_zero": "null"})));
        assert!(formula.emits_row_diagnostics(&serde_json::json!({"on_division_by_zero": "error"})));
        assert!(find_operation("table.expression")
            .expect("expression")
            .emits_row_diagnostics(&serde_json::json!({})));
    }

    #[test]
    fn row_diagnostics_emitting_operations_are_a_closed_catalog_set() {
        // Il perimetro delle operazioni che emettono diagnostica per riga è
        // chiuso e contato: cambiarlo richiede un diff esplicito di questo
        // test.
        let probes = row_diagnostics_probes();
        let emitting: Vec<&str> = CATALOG
            .iter()
            .filter(|op| probes.iter().any(|config| op.emits_row_diagnostics(config)))
            .map(|op| op.id)
            .collect();
        // 40, come in plenora-data-tools@190c493 (`geo.reproject` compresa).
        assert_eq!(
            emitting.len(),
            40,
            "perimetro row-diagnostics: {emitting:?}"
        );
        for id in &emitting {
            let descriptor = find_operation(id).expect("risolta");
            assert_eq!(
                descriptor.source_row_provenance(),
                SourceRowProvenance::Preserved,
                "{id}: emette diagnostica ma non preserva la provenance sorgente"
            );
        }
    }

    #[test]
    fn row_diagnostics_changes_carry_the_declared_version_bumps() {
        // Ogni operazione il cui comportamento osservabile, kernel o
        // controllo di validazione è cambiato con la diagnostica per riga
        // dichiara l'incremento nelle componenti di versione. La tabella è
        // scritta a mano, non letta dal catalogo, perché non sia una
        // tautologia: (id, semantic, config_schema, contract_analysis,
        // kernel).
        let expected: &[(&str, u32, u32, u32, u32)] = &[
            // Tabellari: nuovo rifiuto per riga nel kernel.
            ("table.date_extract", 2, 2, 2, 4),
            ("table.flatten_json", 2, 1, 1, 4),
            ("table.type_cast", 2, 2, 2, 4),
            ("table.md5_hash", 2, 2, 2, 3),
            ("table.sha256_hash", 2, 2, 2, 3),
            ("table.assert_not_null", 2, 1, 1, 2),
            ("table.assert_range", 2, 2, 1, 3),
            ("table.assert_regex", 2, 1, 1, 2),
            ("table.assert_unique", 2, 1, 1, 3),
            ("table.assert_foreign_key", 2, 1, 1, 3),
            ("table.date_add", 2, 2, 2, 4),
            ("table.date_diff", 2, 2, 2, 4),
            ("table.date_format", 2, 2, 2, 4),
            ("table.timezone_convert", 2, 2, 2, 4),
            ("table.explode", 2, 1, 1, 2),
            ("table.formula", 3, 2, 2, 4),
            ("table.expression", 4, 3, 3, 5),
            // `from_wkt`: raccolta nel kernel geo. La successiva dichiarazione
            // di encoding e tipi geometrici del produttore cambia anche
            // semantica e analisi del contratto.
            ("geo.from_wkt", 3, 1, 2, 2),
            // Geo: il rifiuto per riga porta il payload
            // `plenora-row-diagnostics-v1` (comportamento osservabile; kernel
            // invariato -> solo incremento semantico).
            ("geo.affine_transform", 2, 1, 1, 1),
            ("geo.area", 2, 1, 1, 1),
            ("geo.boundary", 2, 1, 1, 1),
            ("geo.bounds_extractor", 2, 1, 1, 1),
            ("geo.buffer", 2, 1, 1, 1),
            ("geo.centroid", 2, 1, 1, 1),
            ("geo.concave_hull", 2, 1, 1, 1),
            ("geo.convex_hull", 2, 1, 1, 1),
            ("geo.densify", 2, 1, 1, 1),
            ("geo.envelope", 2, 1, 1, 1),
            ("geo.from_coords", 2, 1, 1, 1),
            ("geo.geodesic_area", 2, 1, 1, 1),
            ("geo.geodesic_line_length", 2, 1, 1, 1),
            ("geo.length", 2, 1, 1, 1),
            ("geo.line_interpolate_point", 2, 1, 1, 1),
            ("geo.line_substring", 2, 1, 1, 1),
            // Triangolazione caricata in blocco: kernel 2, uscita osservabile
            // cambiata (ordine di delaunay, rifiuti di precisione di voronoi).
            ("geo.delaunay", 2, 1, 1, 2),
            ("geo.voronoi", 2, 1, 2, 2),
            // Backend Rust al posto di GEOS: kernel 2 (vedi il descrittore).
            ("geo.make_valid", 2, 1, 1, 2),
            ("geo.perimeter", 2, 1, 1, 1),
            ("geo.point_on_surface", 2, 1, 1, 1),
            // Rust puro al posto di PROJ: kernel 2, config 2 (vedi il
            // descrittore).
            ("geo.reproject", 2, 2, 1, 2),
            ("geo.rotate", 2, 1, 1, 1),
            ("geo.scale", 2, 1, 1, 1),
            ("geo.simplify", 2, 1, 1, 1),
            ("geo.snap_to_grid", 2, 1, 1, 1),
            ("geo.to_wkt", 2, 1, 1, 1),
            ("geo.translate", 2, 1, 1, 1),
            ("geo.vertex_count", 2, 1, 1, 1),
        ];
        for (id, semantic, config_schema, contract_analysis, kernel) in expected {
            let descriptor = find_operation(id).expect(id);
            assert_eq!(
                (
                    descriptor.semantic_version,
                    descriptor.config_schema_version,
                    descriptor.contract_analysis_version,
                    descriptor.kernel_version,
                ),
                (*semantic, *config_schema, *contract_analysis, *kernel),
                "{id}: versioni non allineate al bump dichiarato"
            );
        }
    }

    #[test]
    fn shape_alignment_carries_the_declared_version_bumps() {
        // Forme, vincoli ed esenzioni allineati a quello che il runner rende
        // (analisi 2 per tutti; semantica 2 dove un'uscita prima rifiutata
        // ora si produce). Tabella scritta a mano: (id, semantic,
        // config_schema, contract_analysis, kernel).
        let expected: &[(&str, u32, u32, u32, u32)] = &[
            // 1:N dichiarata, 1:1 resa.
            ("geo.clean_topology", 1, 1, 2, 1),
            ("geo.voronoi", 2, 1, 2, 2),
            ("geo.count_points_in_polygons", 1, 1, 2, 1),
            ("geo.within", 1, 1, 2, 1),
            // 1:N e MaxRelative (o SumRelative) dichiarate, una riga per riga
            // sinistra resa: il ritaglio su una maschera piccola non si
            // rifiuta piu'.
            ("geo.clip", 2, 1, 2, 1),
            ("geo.difference", 1, 1, 2, 1),
            ("geo.intersection", 1, 1, 2, 1),
            ("geo.symmetric_difference", 1, 1, 2, 1),
            // SumRelative -> LeftRelative: fattori fra 1/2 e 1 ora rifiutati.
            ("geo.union", 2, 1, 2, 1),
            // N:1 dichiarata, da tutta la tabella a molte righe resa.
            ("geo.line_merge", 2, 1, 2, 1),
            ("geo.polygonize", 2, 1, 2, 2),
            // Righe fisse, esenti: la tabella vuota da' la sua riga.
            ("geo.dissolve", 2, 1, 2, 1),
            ("geo.line_builder", 2, 1, 2, 1),
            ("geo.polygon_builder", 2, 1, 2, 1),
            ("table.reconcile", 2, 1, 2, 2),
            ("table.validate_rules", 2, 1, 2, 1),
        ];
        for (id, semantic, config_schema, contract_analysis, kernel) in expected {
            let descriptor = find_operation(id).expect(id);
            assert_eq!(
                (
                    descriptor.semantic_version,
                    descriptor.config_schema_version,
                    descriptor.contract_analysis_version,
                    descriptor.kernel_version,
                ),
                (*semantic, *config_schema, *contract_analysis, *kernel),
                "{id}: versioni non allineate al bump dichiarato"
            );
        }
    }

    #[test]
    fn parametri_senza_effetto_limiti_e_divisione_portano_i_loro_incrementi() {
        // Incrementi di difetti-b, derivati dai motivi e non dai valori: per
        // ogni operazione le versioni prima del ciclo (main `816df5f`) e quali
        // componenti il ciclo tocca, secondo le definizioni dei campi:
        // semantica se cambia l'uscita per lo stesso ingresso e la stessa
        // config (anche un'uscita prima rifiutata e ora prodotta); schema
        // della config se cambiano i parametri accettati (nuovi, rifiutati
        // se scritti senza effetto, `null` esplicito rifiutato); analisi se
        // cambia una regola di validazione; kernel se cambia
        // l'implementazione.
        type Versioni = (u32, u32, u32, u32);
        let motivi: &[(&str, Versioni, Versioni)] = &[
            // ignore_index scritto rifiutato
            ("table.concat", (1, 1, 1, 1), (0, 1, 1, 1)),
            // n con fraction, random_state su campione vuoto, n null
            ("table.sample", (1, 1, 1, 1), (0, 1, 1, 1)),
            // null_literal fuori da literal
            ("table.md5_hash", (2, 1, 1, 2), (0, 1, 1, 1)),
            // null_literal fuori da literal
            ("table.sha256_hash", (2, 1, 1, 2), (0, 1, 1, 1)),
            // invalid rifiutato; output_format contro max_string_bytes
            ("table.date_format", (2, 1, 1, 3), (0, 1, 1, 1)),
            // invalid rifiutato; output_format contro max_string_bytes
            ("table.date_add", (2, 1, 1, 3), (0, 1, 1, 1)),
            // invalid/ambiguous rifiutati; output_format contro max_string_bytes
            ("table.timezone_convert", (2, 1, 1, 3), (0, 1, 1, 1)),
            // invalid rifiutato
            ("table.date_diff", (2, 1, 1, 3), (0, 1, 1, 1)),
            // invalid rifiutato; parts vuoto o ripetuto
            ("table.date_extract", (2, 1, 1, 3), (0, 1, 1, 1)),
            // keep_extra null; default contro max_string_bytes
            ("table.align_schema", (1, 1, 1, 1), (0, 1, 1, 0)),
            // value con isnull/notnull
            ("table.filter", (1, 1, 1, 2), (0, 1, 1, 1)),
            // value con isnull/notnull; testi e numeri non finiti dei risultati
            ("table.conditional", (1, 1, 1, 1), (0, 1, 1, 1)),
            // divisione per zero null di default; on_division_by_zero; campi dei nodi; limiti; divisore letterale zero in analisi
            ("table.expression", (3, 2, 2, 4), (1, 1, 1, 1)),
            // divisione per zero null di default; on_division_by_zero; numeri non finiti rifiutati; testo contro max_string_bytes
            ("table.formula", (2, 1, 1, 3), (1, 1, 1, 1)),
            // errors su target infallibili; regole anche nel kernel; campi null
            ("table.type_cast", (2, 1, 1, 3), (0, 1, 1, 1)),
            // separator con una colonna
            ("table.concat_columns", (1, 1, 1, 1), (0, 1, 1, 1)),
            // delimiter con una uscita; max_splits che non riduce
            ("table.split_column", (1, 1, 1, 1), (0, 1, 1, 1)),
            // width 0
            ("table.string_pad", (1, 1, 1, 1), (0, 1, 1, 1)),
            // separator con una colonna in compare_columns; testi contro max_string_bytes
            ("table.table_diff", (1, 1, 1, 2), (0, 1, 1, 1)),
            // min_rows 0; regole anche nel kernel
            ("table.assert_cardinality", (1, 1, 1, 1), (0, 1, 1, 1)),
            // n 0
            ("table.top_n", (1, 1, 1, 1), (0, 1, 1, 1)),
            // offset con n 0
            ("table.limit", (1, 1, 1, 1), (0, 1, 1, 1)),
            // nomi d'uscita ripetuti o uguali a una chiave; campi null; concat contro max_string_bytes
            ("table.aggregate", (1, 1, 1, 2), (0, 1, 1, 1)),
            // stats vuoto o ripetuto
            ("table.statistics", (1, 1, 1, 2), (0, 1, 1, 1)),
            // colonna ripetuta senza overwrite; campi null; testo contro max_string_bytes
            ("table.mask_data", (1, 1, 1, 2), (0, 1, 1, 1)),
            // renames vuoto, su se stessa
            ("table.rename", (1, 1, 1, 1), (0, 1, 1, 1)),
            // columns vuoto
            ("table.drop_columns", (1, 1, 1, 1), (0, 1, 1, 1)),
            // columns vuoto senza alphabetical; alphabetical null
            ("table.reorder_columns", (1, 1, 1, 1), (0, 1, 1, 1)),
            // fuori dal fattore di espansione (uscite prima rifiutate); type_policy null
            ("table.melt", (1, 1, 1, 2), (1, 1, 0, 1)),
            // type_policy null
            ("table.transpose", (1, 1, 1, 1), (0, 1, 0, 1)),
            // voce vuota in index_col; concat contro max_string_bytes
            ("table.pivot", (1, 1, 1, 2), (0, 1, 1, 1)),
            // tolerance 0 senza allow_exact
            ("table.asof_join", (1, 1, 1, 1), (0, 1, 1, 1)),
            // inclusive_* null; regole anche nel kernel
            ("table.assert_range", (2, 1, 1, 2), (0, 1, 0, 1)),
            // old_value senza regex contro max_string_bytes; celle sostituite contro max_string_bytes
            ("table.replace", (1, 1, 1, 1), (0, 1, 1, 1)),
            // extract_all contro max_string_bytes
            ("table.string_extract", (1, 1, 1, 2), (0, 0, 0, 1)),
            // testi contro max_string_bytes
            ("table.flatten_json", (2, 1, 1, 3), (0, 0, 0, 1)),
            // testi della config contro max_string_bytes
            ("table.lookup", (1, 1, 1, 1), (0, 0, 1, 0)),
            // value contro max_string_bytes
            ("table.fill_na", (1, 1, 1, 2), (0, 0, 1, 0)),
            // labels ed etichette contro max_string_bytes
            ("table.bin", (1, 1, 1, 1), (0, 0, 1, 1)),
            // chiave letta da una funzione sola; non UTF-8 rifiutata
            ("table.hmac_sha256", (1, 1, 1, 2), (0, 0, 0, 1)),
            // offset/buckets null; overflow rifiutato
            ("table.window_function", (1, 1, 1, 2), (0, 1, 0, 1)),
            // ddof null; overflow rifiutato
            ("table.rolling_window", (1, 1, 1, 2), (0, 1, 0, 1)),
            // ascending null
            ("table.dedup_advanced", (1, 1, 1, 2), (0, 1, 0, 0)),
            // SumRelative: uscite prima rifiutate
            ("table.join", (1, 1, 1, 2), (1, 0, 0, 0)),
            // SumRelative: uscite prima rifiutate
            ("table.cross_join", (1, 1, 1, 1), (1, 0, 0, 0)),
            // SumRelative: uscite prima rifiutate
            ("table.fuzzy_join", (1, 1, 1, 1), (1, 0, 0, 0)),
            // SumRelative: uscite prima rifiutate
            ("geo.sjoin", (1, 1, 1, 1), (1, 0, 0, 0)),
            // SumRelative: uscite prima rifiutate
            ("geo.overlay", (1, 1, 1, 1), (1, 0, 0, 0)),
        ];
        for (id, prima, incrementi) in motivi {
            let attese = (
                prima.0 + incrementi.0,
                prima.1 + incrementi.1,
                prima.2 + incrementi.2,
                prima.3 + incrementi.3,
            );
            let descriptor = find_operation(id).expect(id);
            assert_eq!(
                (
                    descriptor.semantic_version,
                    descriptor.config_schema_version,
                    descriptor.contract_analysis_version,
                    descriptor.kernel_version,
                ),
                attese,
                "{id}: versioni non allineate ai motivi dichiarati"
            );
        }
    }
}
