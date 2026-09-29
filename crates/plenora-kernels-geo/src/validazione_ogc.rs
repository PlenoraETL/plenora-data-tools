//! Validazione OGC di `Polygon`, `MultiPolygon`, `GeometryCollection` e
//! `Geometry` con la ricerca delle auto-intersezioni **sub-quadratica nel
//! caso tipico**, a verdetto identico a quello di `geo` 0.33.1 vendorizzato.
//!
//! **Perche' qui e non nel vendor.** In `geo` la ricerca e'
//! `validation::utils::linestring_has_self_intersection`: un doppio ciclo su
//! tutte le coppie di segmenti, O(n²) chiamate a `Line::intersects` per
//! anello, eseguito da ogni `geometry_from_wkb` e da ogni validazione
//! d'uscita. Il vendor e' la copia ricostruita del candidato di memory-lab
//! (pacchetto verificato + patch con provenienza citata, vedi
//! `vendor/*/PROVENANCE*.md`): una patch scritta in questo repository
//! romperebbe quel modello. Qui si rifa' quindi, con le API pubbliche di
//! `geo`, la stessa sequenza di controlli di `Validation::visit_validation`
//! per i tipi che contengono anelli, cambiando **solo** come si trovano le
//! coppie di segmenti candidate; i tipi senza anelli restano a `geo`.
//!
//! **Perche' il verdetto e' lo stesso.** Il predicato per coppia e' quello di
//! `geo`, riusato tale e quale ([`coppia_si_interseca`]): `Line::intersects`
//! col kernel di `geo` (`RobustKernel`, nel vendor filtro certificato +
//! ricaduta esatta, cioe' segno esatto di `orient2d` su ogni `f64` finito),
//! piu' le due esclusioni sugli estremi condivisi. Si scartano soltanto le
//! coppie i cui rettangoli chiusi di ingombro sono disgiunti, e su quelle
//! `intersects` rende `false` in entrambi i versi:
//!
//! - un segmento degenere interseca solo se il punto sta nel rettangolo
//!   dell'altro (`point_in_rect`): i rettangoli si toccano;
//! - il ramo collineare risponde con `point_in_rect` di un estremo nel
//!   rettangolo dell'altro: idem, senza bisogno di aritmetica;
//! - il ramo trasversale risponde `true` solo se ciascun segmento separa (o
//!   tocca) la retta dell'altro; con segni **esatti** i due segmenti hanno
//!   allora un punto comune, che sta in entrambi i rettangoli.
//!
//! L'ultimo punto e' l'unico che dipende dall'esattezza di `orient2d`: se il
//! kernel di `geo` smettesse di essere esatto, il filtro potrebbe scartare
//! una coppia che il doppio ciclo dichiara intersecante. Per questo gli
//! anelli con una coordinata non finita (dove `orient2d` non ha un segno
//! esatto) passano dal doppio ciclo, e l'oracolo differenziale dei test
//! confronta questo modulo con `check_validation` di `geo` (il percorso
//! generico) sull'errore completo, non solo sull'esito.
//!
//! **Coppie di poligoni e di buchi.** `geo` chiama `relate` su ogni coppia
//! di poligoni di un `MultiPolygon` e di buchi di un `Polygon`. Qui le
//! coppie si trovano con una scansione sui rettangoli chiusi
//! ([`CoppieCandidate`]) e si saltano quelle su cui `relate` renderebbe il
//! suo ramo disgiunto ([`relate_non_e_disgiunta`]), che non produce errori;
//! le altre si visitano nell'ordine `(i, j)` del doppio ciclo, quindi la
//! sequenza degli errori e' la stessa. Questo scarto non dipende da
//! aritmetica: e' la stessa condizione di `geo` sugli stessi rettangoli.
//!
//! **Il limite dichiarato.** La scansione e' sub-quadratica quando pochi
//! rettangoli di segmenti si sovrappongono (poligoni reali, cerchi, pettini
//! con denti allineati a un asse); nel caso peggiore, molti segmenti lunghi
//! con rettangoli sovrapposti, resta O(n²) come il doppio ciclo. Vedi
//! `README.md`, sezione «Limiti dichiarati».

use geo::algorithm::validation::{
    CoordIndex, GeometryIndex, InvalidGeometry, InvalidGeometryCollection, InvalidMultiPolygon,
    InvalidPolygon, RingRole, Validation,
};
use geo::coordinate_position::CoordPos;
use geo::dimensions::Dimensions;
use geo::{
    BoundingRect, Geometry, GeometryCollection, HasDimensions, Intersects, Line, LineString,
    MultiPolygon, Polygon, PreparedGeometry, Rect, Relate, RemoveRepeatedPoints,
};

/// La coppia di segmenti conta come auto-intersezione, **esattamente** come
/// nel doppio ciclo di `geo`.
///
/// `geo` prova le coppie ordinate `(i, j)` e `(j, i)`: la condizione sugli
/// estremi e' simmetrica (`a.start != b.end && a.end != b.start` e' la stessa
/// scambiando i ruoli), `intersects` si prova nei due versi per non dipendere
/// dalla sua simmetria.
fn coppia_si_interseca(a: &Line<f64>, b: &Line<f64>) -> bool {
    a.start != b.end && a.end != b.start && (a.intersects(b) || b.intersects(a))
}

/// Il doppio ciclo di `geo`, sulle stesse coppie e nello stesso ordine.
fn autointersezione_doppio_ciclo(segmenti: &[Line<f64>]) -> bool {
    segmenti.iter().enumerate().any(|(i, a)| {
        segmenti
            .iter()
            .enumerate()
            .any(|(j, b)| i != j && a.intersects(b) && a.start != b.end && a.end != b.start)
    })
}

/// Il rettangolo chiuso di ingombro di un segmento.
#[derive(Clone, Copy)]
struct Ingombro {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

impl Ingombro {
    const fn di(segmento: &Line<f64>) -> Self {
        Self {
            min_x: segmento.start.x.min(segmento.end.x),
            max_x: segmento.start.x.max(segmento.end.x),
            min_y: segmento.start.y.min(segmento.end.y),
            max_y: segmento.start.y.max(segmento.end.y),
        }
    }

    /// Estremi sull'asse di scansione, poi sull'altro.
    const fn assi(self, scansione_su_x: bool) -> (f64, f64, f64, f64) {
        if scansione_su_x {
            (self.min_x, self.max_x, self.min_y, self.max_y)
        } else {
            (self.min_y, self.max_y, self.min_x, self.max_x)
        }
    }
}

/// Se l'anello ha un'auto-intersezione secondo la regola di `geo`
/// (`linestring_has_self_intersection`), senza provare tutte le coppie.
///
/// Scansione su un asse: segmenti ordinati per estremo minimo, e per ciascuno
/// si provano solo i successivi il cui minimo non supera il suo massimo, poi
/// si filtra sull'altro asse. Ogni coppia a rettangoli sovrapposti viene
/// raggiunta dal primo dei due nell'ordine: chi sta fra loro ha un minimo
/// compreso fra i due, quindi non interrompe la scansione.
fn anello_con_autointersezione(anello: &LineString<f64>) -> bool {
    autointersezione_con(anello, false, None)
}

/// Il corpo di [`anello_con_autointersezione`], con ramo e asse impostabili
/// dai test.
///
/// Nessuna soglia sotto cui tornare al doppio ciclo: misurata, la scansione
/// vince gia' a 10 vertici (0,29 us contro 21 us sul cerchio,
/// `bench_validazione_ogc`).
fn autointersezione_con(anello: &LineString<f64>, doppio_ciclo: bool, asse: Option<bool>) -> bool {
    let segmenti: Vec<Line<f64>> = anello.lines().collect();
    let tutti_finiti = anello
        .0
        .iter()
        .all(|punto| punto.x.is_finite() && punto.y.is_finite());
    if doppio_ciclo || !tutti_finiti {
        return autointersezione_doppio_ciclo(&segmenti);
    }

    let ingombri: Vec<Ingombro> = segmenti.iter().map(Ingombro::di).collect();
    let su_x = asse.unwrap_or_else(|| scansione_su_x(&ingombri));
    // (min e max sull'asse di scansione, min e max sull'altro, indice).
    let mut ordine: Vec<(f64, f64, f64, f64, usize)> = ingombri
        .iter()
        .enumerate()
        .map(|(indice, ingombro)| {
            let (min_s, max_s, min_a, max_a) = ingombro.assi(su_x);
            (min_s, max_s, min_a, max_a, indice)
        })
        .collect();
    // `total_cmp` e' un ordine totale; su valori finiti e' coerente con `<=`
    // salvo `-0.0 < +0.0`, che nei confronti sotto sono uguali: la monotonia
    // che serve all'interruzione resta. L'ordinamento e' stabile, quindi
    // deterministico anche a pari minimo.
    ordine.sort_by(|a, b| a.0.total_cmp(&b.0));

    for (posizione, &(_, max_i, min_altro_i, max_altro_i, i)) in ordine.iter().enumerate() {
        for &(min_j, _, min_altro_j, max_altro_j, j) in &ordine[posizione + 1..] {
            if min_j > max_i {
                break;
            }
            if min_altro_i <= max_altro_j
                && min_altro_j <= max_altro_i
                && coppia_si_interseca(&segmenti[i], &segmenti[j])
            {
                return true;
            }
        }
    }
    false
}

/// L'asse su cui scandire: quello dove i segmenti sono piu' sottili rispetto
/// all'estensione dell'anello, cioe' dove ci si aspettano meno coppie
/// candidate. Solo prestazioni: la scansione e' completa su entrambi gli assi.
fn scansione_su_x(ingombri: &[Ingombro]) -> bool {
    let mut larghezze = 0.0_f64;
    let mut altezze = 0.0_f64;
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for ingombro in ingombri {
        larghezze += ingombro.max_x - ingombro.min_x;
        altezze += ingombro.max_y - ingombro.min_y;
        min_x = min_x.min(ingombro.min_x);
        max_x = max_x.max(ingombro.max_x);
        min_y = min_y.min(ingombro.min_y);
        max_y = max_y.max(ingombro.max_y);
    }
    // Confronto di larghezze/estensione_x con altezze/estensione_y senza
    // divisioni. Un overflow (infinito, o NaN da `inf * 0`) sceglie un asse
    // qualunque: il verdetto non cambia.
    larghezze * (max_y - min_y) <= altezze * (max_x - min_x)
}

/// Come si trovano le coppie di parti (o di buchi) da confrontare con
/// `relate`. In produzione [`Percorso::PRODUZIONE`]; i test forzano gli
/// altri rami per confrontarli fra loro e con `geo`.
#[derive(Clone, Copy, Debug)]
struct Percorso {
    /// Tutte le coppie, senza filtro: il doppio ciclo di `geo`.
    doppio_ciclo: bool,
    /// Coppie trovate dalla scansione oltre cui si passa al doppio ciclo
    /// filtrato; `None` e' il limite di produzione ([`limite_coppie`]).
    limite_coppie: Option<usize>,
}

impl Percorso {
    const PRODUZIONE: Self = Self {
        doppio_ciclo: false,
        limite_coppie: None,
    };
}

/// Il limite di produzione sulle coppie candidate tenute in memoria: 32 per
/// elemento (un reticolo ne ha 4, una tassellatura reale meno di 10), e
/// almeno 2^16. Oltre, niente elenco: il doppio ciclo col solo filtro sui
/// rettangoli, quadratico nei confronti ma senza memoria aggiuntiva.
const fn limite_coppie(elementi: usize) -> usize {
    let limite = elementi.saturating_mul(32);
    if limite < 1 << 16 {
        1 << 16
    } else {
        limite
    }
}

/// Se `relate` prosegue oltre il suo primo test. **E' la condizione di
/// `RelateOperation::compute_intersection_matrix` di `geo` 0.33.1, copiata
/// tale e quale**: con un rettangolo assente (geometria vuota) o con
/// rettangoli chiusi disgiunti secondo `Rect::intersects`, `relate` rende
/// `compute_disjoint`, che scrive soltanto le celle Interno/Confine contro
/// Esterno; Interno-Interno e Confine-Confine restano `Empty`, cioe' ne'
/// `TwoDimensional` ne' `OneDimensional`, e la coppia non produce errori.
/// Quel ramo non esegue altro (niente grafo, niente aritmetica sulle
/// coordinate), quindi non va nemmeno in panico: saltare la chiamata non
/// cambia la sequenza osservabile.
///
/// I rettangoli devono essere quelli che `relate` calcola: `bounding_rect`
/// del `Polygon`, cioe' del suo anello esterno; `PreparedGeometry` ne tiene
/// una copia calcolata dalla stessa funzione.
fn relate_non_e_disgiunta(a: Option<Rect<f64>>, b: Option<Rect<f64>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.intersects(&b),
        _ => false,
    }
}

/// Le coppie `(i, j)`, `i < j`, su cui chiamare `relate`, nell'ordine del
/// doppio ciclo di `geo`: per `i` crescente, `j` crescente.
enum CoppieCandidate {
    /// Tutte: il doppio ciclo di `geo`.
    Tutte(usize),
    /// Tutte quelle per cui [`relate_non_e_disgiunta`]: il doppio ciclo col
    /// filtro, quando la scansione troverebbe troppe coppie.
    Filtrate(Vec<Option<Rect<f64>>>),
    /// Quelle trovate dalla scansione, ordinate per `(i, j)`.
    Elenco(Vec<(usize, usize)>),
}

impl CoppieCandidate {
    /// Le coppie per gli elementi con questi rettangoli.
    ///
    /// `coordinate_finite` dice se **tutte** le coordinate degli elementi
    /// sono finite: se no, il doppio ciclo senza filtro, come per le
    /// auto-intersezioni (con un NaN `bounding_rect` puo' nascondere la
    /// coordinata o rendere un rettangolo non ordinabile, e `relate` non e'
    /// definito: si lascia a `geo` ogni comportamento, panico compreso).
    ///
    /// **Perche' la scansione e' completa.** Gli elementi si ordinano per
    /// `min.x` (`total_cmp`: su valori finiti coerente con `<=`, salvo
    /// `-0.0 < +0.0` che nei confronti sotto sono uguali); per ciascuno si
    /// provano i successivi finche' `min.x` dell'altro supera il proprio
    /// `max.x`. Chi viene dopo ha `min.x` non minore, quindi anche per lui
    /// `max.x < min.x` dell'altro: `Rect::intersects` e' falso. Ogni coppia
    /// con `Rect::intersects` vero e' raggiunta dal primo dei due
    /// nell'ordine, perche' chi sta fra loro ha `min.x` non maggiore di
    /// quello del secondo, che non supera il `max.x` del primo. Le coppie
    /// raggiunte passano poi da [`relate_non_e_disgiunta`] con gli argomenti
    /// nell'ordine di `relate` (`i` il minore): l'elenco e' **esattamente**
    /// l'insieme delle coppie su cui `relate` non prende il ramo disgiunto.
    fn di(ingombri: Vec<Option<Rect<f64>>>, coordinate_finite: bool, percorso: Percorso) -> Self {
        if percorso.doppio_ciclo || !coordinate_finite {
            return Self::Tutte(ingombri.len());
        }
        let limite = percorso
            .limite_coppie
            .unwrap_or_else(|| limite_coppie(ingombri.len()));
        let mut ordine: Vec<(Rect<f64>, usize)> = ingombri
            .iter()
            .enumerate()
            .filter_map(|(indice, ingombro)| ingombro.map(|ingombro| (ingombro, indice)))
            .collect();
        // Stabile: a pari minimo resta l'ordine degli indici.
        ordine.sort_by(|a, b| a.0.min().x.total_cmp(&b.0.min().x));
        let mut coppie: Vec<(usize, usize)> = Vec::new();
        for (posizione, &(ingombro_a, a)) in ordine.iter().enumerate() {
            for &(ingombro_b, b) in &ordine[posizione + 1..] {
                if ingombro_b.min().x > ingombro_a.max().x {
                    break;
                }
                let (i, j, ingombro_i, ingombro_j) = if a < b {
                    (a, b, ingombro_a, ingombro_b)
                } else {
                    (b, a, ingombro_b, ingombro_a)
                };
                if relate_non_e_disgiunta(Some(ingombro_i), Some(ingombro_j)) {
                    if coppie.len() >= limite {
                        return Self::Filtrate(ingombri);
                    }
                    coppie.push((i, j));
                }
            }
        }
        // Le coppie sono distinte: l'ordinamento instabile e' deterministico.
        coppie.sort_unstable();
        Self::Elenco(coppie)
    }

    /// I `j > i` da confrontare con `i`, in ordine crescente.
    fn di_indice(&self, i: usize) -> Partner<'_> {
        match self {
            Self::Tutte(quanti) => Partner::Intervallo(i + 1..*quanti),
            Self::Filtrate(ingombri) => Partner::Filtrati {
                altri: i + 1..ingombri.len(),
                ingombro: ingombri.get(i).copied().flatten(),
                ingombri,
            },
            Self::Elenco(coppie) => {
                let inizio = coppie.partition_point(|&(primo, _)| primo < i);
                Partner::Elenco {
                    coppie: coppie[inizio..].iter(),
                    i,
                }
            }
        }
    }
}

/// Iteratore dei `j` di [`CoppieCandidate::di_indice`].
enum Partner<'a> {
    Intervallo(std::ops::Range<usize>),
    Filtrati {
        altri: std::ops::Range<usize>,
        ingombro: Option<Rect<f64>>,
        ingombri: &'a [Option<Rect<f64>>],
    },
    Elenco {
        coppie: std::slice::Iter<'a, (usize, usize)>,
        i: usize,
    },
}

impl Iterator for Partner<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        match self {
            Self::Intervallo(altri) => altri.next(),
            Self::Filtrati {
                altri,
                ingombro,
                ingombri,
            } => altri
                .find(|&j| relate_non_e_disgiunta(*ingombro, ingombri.get(j).copied().flatten())),
            Self::Elenco { coppie, i } => match coppie.next() {
                Some(&(primo, j)) if primo == *i => Some(j),
                _ => None,
            },
        }
    }
}

/// Se tutte le coordinate degli anelli sono finite.
fn anelli_finiti<'a>(anelli: impl IntoIterator<Item = &'a LineString<f64>>) -> bool {
    anelli.into_iter().all(|anello| {
        anello
            .0
            .iter()
            .all(|punto| punto.x.is_finite() && punto.y.is_finite())
    })
}

/// `Polygon::visit_validation` di `geo` 0.33.1, stessa sequenza di
/// controlli ed errori, con [`anello_con_autointersezione`] al posto del
/// doppio ciclo e le coppie di buchi a rettangoli disgiunti saltate
/// ([`CoppieCandidate`]).
fn visita_poligono<T>(
    poligono: &Polygon<f64>,
    gestisci: &mut dyn FnMut(InvalidPolygon) -> Result<(), T>,
) -> Result<(), T> {
    visita_poligono_con(poligono, Percorso::PRODUZIONE, gestisci)
}

/// Il corpo di [`visita_poligono`], col percorso delle coppie di buchi
/// impostabile dai test.
fn visita_poligono_con<T>(
    poligono: &Polygon<f64>,
    percorso: Percorso,
    gestisci: &mut dyn FnMut(InvalidPolygon) -> Result<(), T>,
) -> Result<(), T> {
    if HasDimensions::is_empty(poligono) {
        return Ok(());
    }

    for (indice_anello, anello) in std::iter::once(poligono.exterior())
        .chain(poligono.interiors().iter())
        .enumerate()
    {
        if HasDimensions::is_empty(anello) {
            continue;
        }
        let ruolo = if indice_anello == 0 {
            RingRole::Exterior
        } else {
            RingRole::Interior(indice_anello - 1)
        };

        // `utils::check_too_few_points(anello, true)` di `geo`.
        if anello.remove_repeated_points().0.len() < 4 {
            gestisci(InvalidPolygon::TooFewPointsInRing(ruolo))?;
        }

        if anello_con_autointersezione(anello) {
            gestisci(InvalidPolygon::SelfIntersection(ruolo))?;
        }

        for (indice_punto, punto) in anello.0.iter().enumerate() {
            // `utils::check_coord_is_not_finite` di `geo`.
            if !(punto.x.is_finite() && punto.y.is_finite()) {
                gestisci(InvalidPolygon::NonFiniteCoord(
                    ruolo,
                    CoordIndex(indice_punto),
                ))?;
            }
        }
    }

    let ha_interni = poligono
        .interiors()
        .iter()
        .any(|anello| !HasDimensions::is_empty(anello));
    if !ha_interni {
        return Ok(());
    }

    let esterno = Polygon::new(poligono.exterior().clone(), vec![]);
    let esterno_preparato = PreparedGeometry::from(&esterno);

    // Ogni buco come poligono, costruito una volta: `geo` lo ricostruisce a
    // ogni coppia, con lo stesso contenuto.
    let buchi: Vec<Polygon<f64>> = poligono
        .interiors()
        .iter()
        .map(|interno| Polygon::new(interno.clone(), vec![]))
        .collect();
    let coppie = CoppieCandidate::di(
        buchi.iter().map(BoundingRect::bounding_rect).collect(),
        anelli_finiti(poligono.interiors()),
        percorso,
    );

    for (indice_1, (interno_1, interno_1_poligono)) in
        poligono.interiors().iter().zip(&buchi).enumerate()
    {
        let ruolo_1 = RingRole::Interior(indice_1);
        if HasDimensions::is_empty(interno_1) {
            continue;
        }

        let interno_1_preparato = PreparedGeometry::from(interno_1_poligono);
        let esterno_contro_interno = esterno_preparato.relate(&interno_1_preparato);

        if !esterno_contro_interno.is_contains() {
            gestisci(InvalidPolygon::InteriorRingNotContainedInExteriorRing(
                ruolo_1,
            ))?;
        }

        if esterno_contro_interno.get(CoordPos::OnBoundary, CoordPos::OnBoundary)
            == Dimensions::OneDimensional
        {
            gestisci(InvalidPolygon::IntersectingRingsOnALine(
                RingRole::Exterior,
                ruolo_1,
            ))?;
        }

        for indice_2 in coppie.di_indice(indice_1) {
            let (interno_2, interno_2_poligono) =
                (&poligono.interiors()[indice_2], &buchi[indice_2]);
            let ruolo_2 = RingRole::Interior(indice_2);
            if HasDimensions::is_empty(interno_2) {
                continue;
            }

            let matrice = interno_1_preparato.relate(interno_2_poligono);

            if matrice.get(CoordPos::Inside, CoordPos::Inside) == Dimensions::TwoDimensional {
                gestisci(InvalidPolygon::IntersectingRingsOnAnArea(ruolo_1, ruolo_2))?;
            }
            if matrice.get(CoordPos::OnBoundary, CoordPos::OnBoundary) == Dimensions::OneDimensional
            {
                gestisci(InvalidPolygon::IntersectingRingsOnALine(ruolo_1, ruolo_2))?;
            }
        }
    }
    Ok(())
}

/// `MultiPolygon::visit_validation` di `geo` 0.33.1, con le coppie di
/// parti a rettangoli disgiunti saltate ([`CoppieCandidate`]). Le coppie
/// restanti si visitano nell'ordine di `geo`, intercalate come in `geo` alla
/// validazione di ciascuna parte: stessa sequenza di errori.
fn visita_multipoligono<T>(
    poligoni: &MultiPolygon<f64>,
    gestisci: &mut dyn FnMut(InvalidMultiPolygon) -> Result<(), T>,
) -> Result<(), T> {
    visita_multipoligono_con(poligoni, Percorso::PRODUZIONE, gestisci)
}

/// Il corpo di [`visita_multipoligono`], col percorso delle coppie (di parti
/// e di buchi) impostabile dai test.
fn visita_multipoligono_con<T>(
    poligoni: &MultiPolygon<f64>,
    percorso: Percorso,
    gestisci: &mut dyn FnMut(InvalidMultiPolygon) -> Result<(), T>,
) -> Result<(), T> {
    let coppie = CoppieCandidate::di(
        poligoni.0.iter().map(BoundingRect::bounding_rect).collect(),
        poligoni.0.iter().all(|poligono| {
            anelli_finiti(std::iter::once(poligono.exterior()).chain(poligono.interiors()))
        }),
        percorso,
    );
    for (i, poligono) in poligoni.0.iter().enumerate() {
        visita_poligono_con(poligono, percorso, &mut |errore| {
            gestisci(InvalidMultiPolygon::InvalidPolygon(
                GeometryIndex(i),
                errore,
            ))
        })?;

        for j in coppie.di_indice(i) {
            let altro = &poligoni.0[j];
            let matrice = poligono.relate(altro);
            if matrice.get(CoordPos::Inside, CoordPos::Inside) == Dimensions::TwoDimensional {
                gestisci(InvalidMultiPolygon::ElementsOverlaps(
                    GeometryIndex(i),
                    GeometryIndex(j),
                ))?;
            }
            if matrice.get(CoordPos::OnBoundary, CoordPos::OnBoundary) == Dimensions::OneDimensional
            {
                gestisci(InvalidMultiPolygon::ElementsTouchOnALine(
                    GeometryIndex(i),
                    GeometryIndex(j),
                ))?;
            }
        }
    }
    Ok(())
}

/// `GeometryCollection::visit_validation` di `geo` 0.33.1.
fn visita_collezione<T>(
    collezione: &GeometryCollection<f64>,
    gestisci: &mut dyn FnMut(InvalidGeometryCollection) -> Result<(), T>,
) -> Result<(), T> {
    for (i, geometria) in collezione.0.iter().enumerate() {
        visita_geometria(geometria, &mut |errore| {
            gestisci(InvalidGeometryCollection::InvalidGeometry(
                GeometryIndex(i),
                Box::new(errore),
            ))
        })?;
    }
    Ok(())
}

/// `Geometry::visit_validation` di `geo` 0.33.1: le varianti con anelli
/// passano da qui, le altre restano a `geo`.
fn visita_geometria<T>(
    geometria: &Geometry<f64>,
    gestisci: &mut dyn FnMut(InvalidGeometry) -> Result<(), T>,
) -> Result<(), T> {
    match geometria {
        Geometry::Polygon(g) => visita_poligono(g, &mut |errore| {
            gestisci(InvalidGeometry::InvalidPolygon(errore))
        }),
        Geometry::MultiPolygon(g) => visita_multipoligono(g, &mut |errore| {
            gestisci(InvalidGeometry::InvalidMultiPolygon(errore))
        }),
        Geometry::GeometryCollection(g) => visita_collezione(g, &mut |errore| {
            gestisci(InvalidGeometry::InvalidGeometryCollection(errore))
        }),
        Geometry::Point(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidPoint(errore))
        })),
        Geometry::Line(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidLine(errore))
        })),
        Geometry::LineString(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidLineString(errore))
        })),
        Geometry::MultiPoint(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidMultiPoint(errore))
        })),
        Geometry::MultiLineString(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidMultiLineString(errore))
        })),
        Geometry::Rect(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidRect(errore))
        })),
        Geometry::Triangle(g) => g.visit_validation(Box::new(|errore| {
            gestisci(InvalidGeometry::InvalidTriangle(errore))
        })),
    }
}

/// La validazione OGC che la barriera esegue: `check_validation` di `geo`,
/// oppure la stessa sequenza con la ricerca rapida delle auto-intersezioni.
pub trait ValidazioneOgc: Validation {
    /// Come `check_validation`: il primo errore, lo stesso che darebbe `geo`.
    ///
    /// # Errors
    ///
    /// Il primo errore di validazione, identico a quello di `geo`.
    fn valida_ogc_rapida(&self) -> Result<(), Self::Error> {
        self.check_validation()
    }
}

impl ValidazioneOgc for Polygon<f64> {
    fn valida_ogc_rapida(&self) -> Result<(), InvalidPolygon> {
        visita_poligono(self, &mut Err)
    }
}

impl ValidazioneOgc for MultiPolygon<f64> {
    fn valida_ogc_rapida(&self) -> Result<(), InvalidMultiPolygon> {
        visita_multipoligono(self, &mut Err)
    }
}

impl ValidazioneOgc for GeometryCollection<f64> {
    fn valida_ogc_rapida(&self) -> Result<(), InvalidGeometryCollection> {
        visita_collezione(self, &mut Err)
    }
}

impl ValidazioneOgc for Geometry<f64> {
    fn valida_ogc_rapida(&self) -> Result<(), InvalidGeometry> {
        visita_geometria(self, &mut Err)
    }
}

/// Tutti gli errori, come `Validation::validation_errors` di `geo`: serve
/// all'oracolo per confrontare anche cio' che segue il primo errore.
#[cfg(test)]
pub fn errori_di_validazione(geometria: &Geometry<f64>) -> Vec<InvalidGeometry> {
    let mut errori = Vec::new();
    let esito: Result<(), std::convert::Infallible> = visita_geometria(geometria, &mut |errore| {
        errori.push(errore);
        Ok(())
    });
    match esito {
        Ok(()) => errori,
        Err(infallibile) => match infallibile {},
    }
}

#[cfg(test)]
mod tests;
