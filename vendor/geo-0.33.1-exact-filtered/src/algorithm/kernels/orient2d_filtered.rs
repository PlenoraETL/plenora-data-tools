//! Filtro veloce sperimentale per il segno di `orient2d`, separato dal
//! riferimento sempre esatto (`exact_orientation.rs`, non modificato da
//! questo file — qui e' importato e usato solo come fallback).
//!
//! # Perche' non e' un ritorno al vecchio kernel
//!
//! Il kernel precedente al diff 1 chiamava direttamente `robust::orient2d`
//! (crate `robust` v1.2.0, la vera implementazione adattiva di Shewchuk) e
//! si fidava del suo segno. Quella crate ha un difetto riproducibile: sulla
//! famiglia di reperti "subnormal" del banco storico del laboratorio
//! (`exact-sign-matrix.json`), fallisce su tutte le sei permutazioni — un
//! punto all'origine esatta e due punti con coordinate minuscole, una delle
//! quali un subnormale al limite inferiore rappresentabile (bit pattern
//! `0x0000000000000001`, cioe' 2^-1074). Su 1266 casi verificati contro
//! l'oracolo razionale, il vecchio kernel ne sbaglia 220. Non e' quindi
//! lecito fidarsi del segno di `robust::orient2d` come "percorso veloce
//! gia' pronto": va costruito un filtro la cui correttezza sia verificabile
//! da soli, senza appoggiarsi alla correttezza di quell'implementazione.
//!
//! # Struttura del filtro
//!
//! Segue lo schema classico di Shewchuk (Adaptive Precision Floating-Point
//! Arithmetic and Fast Robust Geometric Predicates,
//! <https://www.cs.cmu.edu/~quake/robust.html>): una stima veloce in
//! virgola mobile, corredata da un limite d'errore dimostrato che certifica
//! quando il segno della stima coincide col segno del determinante esatto.
//! A differenza dell'algoritmo originale di Shewchuk (che, quando il
//! filtro non certifica, escalation su livelli intermedi di precisione
//! "expansion arithmetic" scritti a mano), qui l'escalation e' diretta al
//! kernel esatto gia' qualificato (`exact_orientation::orient2d_sign_bits`,
//! aritmetica a interi a precisione fissa, 1266/1266 sull'oracolo
//! razionale) — non serve reimplementare i livelli intermedi perche' il
//! fallback e' gia' totalmente esatto, non solo "piu' preciso".
//!
//! Il determinante: `det = (qx-px)*(ry-py) - (qy-py)*(rx-px)` — stessa
//! formula (a meno di riordino algebrico verificato) di
//! `exact_orientation::orient2d_sign_bits`, che usa
//! `ax*by + bx*cy + cx*ay - ay*bx - by*cx - cy*ax` con a=p, b=q, c=r.
//!
//! # Derivazione del limite d'errore (giustificazione numerica)
//!
//! Questa sezione sostituisce una versione precedente basata su `≈`/`≲` e
//! su un "margine doppio" non dimostrato: ogni passo qui e' una
//! disuguaglianza esplicita, con la costante numerica che la rende vera
//! dichiarata e verificabile a mano.
//!
//! ## Modello d'errore IEEE 754 (misto, valido ovunque incluso il
//! sottoflusso)
//!
//! Per ogni operazione binaria di base `∘ ∈ {+,-,×}` fra valori finiti il
//! cui risultato non eccede in overflow:
//! `fl(x∘y) = (x∘y)(1+δ) + η`, con `|δ| ≤ u`, `|η| ≤ e₀`, **e `δ·η = 0`**
//! (al piu' uno dei due termini e' non nullo). Qui `u = 2^-53`
//! (arrotondamento unitario, 53 bit di precisione) ed
//! `e₀ = 2^-1075` (meta' del passo fra `0` e il piu' piccolo subnormale
//! `2^-1074`). Il termine `η` e' non nullo **solo** quando il risultato
//! arrotondato e' esso stesso subnormale o zero — se il risultato
//! arrotondato e' normale, `η = 0` e vale il modello puramente relativo. Fatto
//! standard di aritmetica IEEE 754 a precisione fissa (si veda es. Muller et
//! al., *Handbook of Floating-Point Arithmetic*, cap. sull'errore di
//! arrotondamento con sottonormali); non e' specifico di questo filtro.
//!
//! ## Fatto 1 — la sottrazione non produce mai uno zero spurio
//!
//! Se `a ≠ b` sono due `f64` finiti distinti, `fl(a-b) ≠ 0`.
//! *Dimostrazione:* la spaziatura minima fra due `f64` distinti qualsiasi
//! (normali o subnormali) e' `2^-1074` (i subnormali sono spaziati
//! uniformemente da `2^-1074`, e i normali piu' piccoli hanno la stessa
//! spaziatura per continuita' del sottoflusso graduale). Quindi
//! `|a-b| ≥ 2^-1074` come numero reale. Il valore `2^-1074` e' esso stesso
//! rappresentabile esattamente (e' il piu' piccolo subnormale positivo), e
//! l'arrotondamento a zero avviene solo per valori reali di modulo
//! **minore o uguale** a `2^-1075` (il punto medio fra `0` e `2^-1074`: con
//! l'arrotondamento ties-to-even anche il pareggio esatto va a `0`, che ha
//! l'ultimo bit pari — precisazione dovuta a revisione, non cambia la
//! conclusione). Siccome `2^-1074 > 2^-1075`, cioe' la spaziatura minima e'
//! **strettamente sopra** il punto di pareggio, `fl(a-b)` non puo'
//! arrotondare a zero se `a≠b`. ∎
//!
//! Conseguenza: se una delle sottrazioni `d1..d4` calcola esattamente
//! `0.0`, il valore reale corrispondente (`D1..D4`) e' **esattamente**
//! zero — non un sottoflusso. Non serve quindi cautelare lo zero per le
//! sottrazioni: e' sempre genuino, e la funzione [`sicuro`] tratta `0.0`
//! come sicuro senza ulteriori controlli.
//!
//! ## Fatto 2 — il prodotto PUO' produrre uno zero spurio
//!
//! Il Fatto 1 e' specifico della sottrazione: un prodotto di due fattori
//! **non nulli** puo' sottofluire esattamente a `0.0` quando il vero
//! prodotto ha modulo `< 2^-1075` — trovato per davvero durante la
//! qualifica (`1e-200 * 1e-200`, modulo vero `~1e-400 ≪ 2^-1075 ≈ 4.9e-324`).
//! Per un prodotto, quindi, uno zero e' sicuro solo se **almeno uno dei due
//! fattori e' esattamente zero** (moltiplicare per zero esatto e' sempre
//! esatto, qualunque sia l'altro fattore) — vedi [`prodotto_sicuro`], che
//! e' per questo diversa da [`sicuro`].
//!
//! ## Composizione degli errori relativi (Lemma di Higham)
//!
//! *Lemma (Higham, Accuracy and Stability of Numerical Algorithms, 2a ed.,
//! Lemma 3.1).* Se `|δᵢ| ≤ u` per `i=1..k` e `ku<1`, allora
//! `|∏(1+δᵢ) - 1| ≤ γₖ := ku/(1-ku)`.
//!
//! Con `d1,d2` entrambi `sicuro` (quindi ciascuno o esattamente zero — Fatto
//! 1 — o con errore relativo puro `|δ|≤u`, `η=0`) e `detleft`
//! `prodotto_sicuro`: se `d1,d2` sono entrambi non nulli e `detleft` e'
//! normale, `detleft = D1·D2·(1+δ1)(1+δ2)(1+δ5) = DL·(1+εL)` con
//! `|εL| ≤ γ₃ = 3u/(1-3u)`. Analogamente `detright = DR·(1+εR)`,
//! `|εR| ≤ γ₃`.
//!
//! **Disuguaglianza esplicita 1** (vale per `u=2^-53`, quindi `3u<1/6`):
//! `1/(1-3u) ≤ 1+6u` — dimostrazione diretta: `(1-3u)(1+6u) = 1+3u-18u² ≥ 1`
//! sse `3u≥18u²` sse `u≤1/6`, vero. Quindi
//! `γ₃ = 3u/(1-3u) ≤ 3u(1+6u) = 3u+18u² < 3.1u`
//! (l'ultimo passo: `18u² < 0.1u` sse `u<1/180`, vero per `u=2^-53`).
//!
//! ## Casi senza cancellazione (sette delle nove combinazioni di segno)
//!
//! Se `detleft` e `detright` hanno segno **discorde, o uno dei due e'
//! zero**: la somma (per segni discordi, `detleft-detright` e' una somma di
//! due termini dello stesso segno una volta tolto il segno) o la
//! sottrazione da zero esatto non ha cancellazione, e per il Fatto 1
//! applicato al risultato finale il segno di `det` e' **esattamente**
//! quello di `DET`, senza bisogno di alcun limite d'errore — vedi
//! [`segno_senza_cancellazione`] per i sette casi.
//!
//! ## Caso concorde (cancellazione possibile): la disuguaglianza finale
//!
//! Restano concorde-positivo e concorde-negativo. Sia
//! `det = fl(detleft-detright) = (detleft-detright)(1+δ7)+η7`,
//! `|δ7|≤u`, `|η7|≤e₀`, `δ7η7=0`. Allora, usando
//! `detleft-detright = DET + DL·εL - DR·εR`:
//!
//! `det - DET = DET·δ7 + (DL·εL - DR·εR)(1+δ7) + η7`
//!
//! `|det-DET| ≤ |DET|·u + (|DL|·γ₃+|DR|·γ₃)(1+u) + e₀`
//!
//! Con `|DET|=|DL-DR|≤|DL|+|DR|` (disuguaglianza triangolare, esatta):
//!
//! `|det-DET| ≤ (|DL|+|DR|)·[u + γ₃(1+u)] + e₀`
//!
//! **Disuguaglianza esplicita 2** (costante `C`): con `γ₃<3.1u`,
//! `C := u+γ₃(1+u) < u+3.1u(1+u) = 4.1u+3.1u² < 4.1u+0.1u = 4.2u`
//! (`3.1u²<0.1u` sse `u<1/31`, vero).
//!
//! **Disuguaglianza esplicita 3** (`|DL|+|DR|` in termini di `detsum`):
//! da `detleft=DL(1+εL)`, `|DL|=|detleft|/|1+εL|≤|detleft|/(1-γ₃)`.
//! Con `γ₃<3.1u<1/2`: `1/(1-γ₃) ≤ 1/(1-3.1u) < 1+4u`
//! (`(1-3.1u)(1+4u)=1+0.9u-12.4u²≥1` sse `0.9≥12.4u`, vero).
//! Quindi `|DL|<|detleft|(1+4u)`, `|DR|<|detright|(1+4u)`, e con
//! `detsum := |detleft|+|detright|`: `|DL|+|DR| < detsum·(1+4u)`.
//!
//! **Disuguaglianza esplicita 4** (combinazione): `(|DL|+|DR|)·C <
//! detsum·(1+4u)·4.2u = detsum·(4.2u+16.8u²) < detsum·4.4u`
//! (`16.8u²<0.2u` sse `u<1/84`, vero).
//!
//! **Limite finale, dimostrato senza approssimazioni:**
//! `|det-DET| < 4.4u·S + e₀`, dove `S := |detleft|+|detright|` e' la somma
//! reale **calcolata senza arrotondamento** (non `|DL|+|DR|`: quella e' una
//! quantita' diversa, gia' messa in relazione con `S` dalla disuguaglianza
//! esplicita 3 sopra — `S` e `|DL|+|DR|` non vanno confuse).
//!
//! ## Dal limite esatto al confronto che il codice esegue davvero
//!
//! Il passo sopra usa `S`, la somma **esatta**. Il codice pero' calcola
//! `detsum = detleft.abs()+detright.abs()` in virgola mobile: il valore
//! usato dal confronto e' `s := fl(S)`, non `S`. Una versione precedente di
//! questo commento liquidava lo scarto fra `S` e `s` come "trascurabile"
//! senza chiuderlo — corretto qui con una disuguaglianza esplicita
//! (osservazione dovuta a revisione):
//!
//! - `s = fl(S)` soddisfa `s ≥ (1-u)S` (modello relativo standard, `S` e'
//!   una somma di due valori non negativi quindi non subnormale se
//!   `S≥SOGLIA_SICURA`, vedi sotto — nessuna cautela ulteriore serve qui).
//! - `ERRBOUND_REL = 8u = 2^-50` e' una **potenza di due esatta**:
//!   moltiplicare per essa non introduce arrotondamento finche' il
//!   risultato non sottoflusce ne' eccede in overflow, quindi
//!   `fl(8u·s) = 8u·s` esattamente.
//! - Il limite che il codice calcola per davvero,
//!   `B := fl(ERRBOUND_REL·s + E0)`, soddisfa `B ≥ ERRBOUND_REL·s = 8u·s`
//!   per **monotonicita' dell'arrotondamento corretto**: `8u·s` e' gia'
//!   esattamente rappresentato, e sommargli `E0≥0` prima di arrotondare non
//!   puo' mai dare un risultato arrotondato **minore** di `8u·s` stesso —
//!   questo vale indipendentemente dal fatto che `E0` sopravviva o meno
//!   all'arrotondamento (non serve dimostrare che sopravviva: la
//!   disuguaglianza `B≥8u·s` non dipende da questo).
//! - Quindi `B ≥ 8u·s ≥ 8u(1-u)S = 8uS - 8u²S`.
//!
//! Resta da mostrare `8uS-8u²S > 4.4uS+e₀`, cioe' `(3.6-8u)uS > e₀` — **il
//! fattore `2^54` che segue confronta questo residuo `(3.6-8u)uS` con `e₀`
//! soltanto, non con l'intero limite `4.4uS+e₀`**: e' la parte che serve
//! dimostrare per chiudere la disuguaglianza, non una misura di quanto B
//! superi il limite complessivo. Nel ramo concorde
//! `S ≥ 2·SOGLIA_SICURA = 2^-968` (vedi sotto), quindi
//! `(3.6-8u)uS ≥ (3.6-8u)·2^-53·2^-968`, che per `u=2^-53` e' maggiore di
//! `3·2^-1021` (il fattore `8u≈8.9e-16` toglie una frazione irrilevante di
//! `3.6`). Confrontato con `e₀=2^-1075`: quel residuo supera `e₀` di oltre
//! `2^54` volte. Quindi **`B > 4.4uS+e₀ > |det-DET|`**: il confronto
//! `det>B` che il codice esegue davvero (con `s` arrotondato) implica
//! comunque `|det|>|det-DET|`, cioe' la conclusione di segno del paragrafo
//! successivo.
//!
//! ## Overflow di `detsum`
//!
//! `detsum = detleft.abs()+detright.abs()` e' una somma di due valori
//! finiti non negativi: se entrambi vicini a `f64::MAX`, la somma stessa
//! puo' arrotondare a `+∞` (overflow dell'addizione, non dei fattori).
//! In tal caso `errbound = ERRBOUND_REL*detsum+E0 = +∞`, e nessun `det`
//! finito puo' soddisfare `det>errbound` o `-det>errbound`: il filtro
//! ricade sempre e correttamente sul kernel esatto, mai su un segno
//! certificato sbagliato. Non serve un controllo esplicito d'overflow: il
//! confronto in virgola mobile con `+∞` e' gia' quello che serve.
//!
//! ## Uguaglianza alla soglia — perche' il codice usa `>` stretto
//!
//! Dal limite dimostrato sopra, se `|det| > B ≥ |det-DET|` (con `B` il
//! valore che il codice calcola davvero, `≥` per il margine ampio appena
//! mostrato), allora `DET` ha lo stesso segno di `det` — fatto elementare:
//! `|det-DET|<|det|` con `det≠0` implica che `DET` non puo' essere
//! dall'altra parte dello zero rispetto a `det`, ne' essere esso stesso
//! zero.
//!
//! Il confronto usa `>` stretto, non `>=`, per una ragione di **principio
//! della dimostrazione**, non perche' sia stato costruito un caso concreto
//! in cui `>=` dia un segno sbagliato: la disuguaglianza che il teorema
//! fornisce e' `|det|>B ⟹ segno(det)=segno(DET)`; non e' stato dimostrato
//! (ne' escluso) il caso `|det|=B` esatto, e col margine ampio ricavato
//! sopra (il residuo `(3.6-8u)uS` supera `e₀` di piu' di `2^54` volte nel
//! caso peggiore ammesso) non e' chiaro che quell'uguaglianza sia
//! raggiungibile affatto per input reali. `>` resta la scelta corretta
//! perche' e' cio'
//! che il teorema autorizza direttamente e non costa nulla: se `|det|=B`
//! capitasse, la sola conseguenza di `>` e' una ricaduta sul kernel esatto
//! — mai un segno certificato senza copertura dimostrata. Non si presenta
//! quindi `>=` come un difetto numerico dimostrato: e' semplicemente una
//! scelta non coperta dal teorema, mentre `>` lo e' per costruzione.
//!
//! ## Perche' `SOGLIA_SICURA = 2^-969`
//!
//! Nel ramo concorde, `detleft` e `detright` sono entrambi non nulli (lo
//! zero e' gia' intercettato da [`segno_senza_cancellazione`]) e
//! `prodotto_sicuro`, quindi ciascuno ha modulo `≥ SOGLIA_SICURA`, dando
//! `S ≥ 2·SOGLIA_SICURA = 2^-968`. Con `ERRBOUND_REL=8u=2^-50`:
//! `ERRBOUND_REL·2^-968 = 2^-1018` (non `2^-1017`: correzione aritmetica
//! dovuta a revisione). Il margine `(3.6-8u)uS` usato sopra per chiudere
//! `B>4.4uS+e₀` vale allora almeno `~3·2^-53·2^-968=3·2^-1021`, contro
//! `e₀=2^-1075`: quel residuo supera `e₀` di oltre `2^54` volte.
//!
//! Questa dimostrazione **non si estende** a `f64::is_normal()` puro
//! (soglia `2^-1022`): li' `s` potrebbe essere piccolo abbastanza che
//! `8u·s` (con `8u=2^-50`) cada in territorio subnormale, e in quel caso
//! la moltiplicazione per la potenza di due **non e' piu' garantita
//! esatta** — l'arrotondamento perderebbe bit di mantissa, il passo
//! `fl(8u·s)=8u·s` usato sopra smetterebbe di valere cosi' com'e' scritto,
//! e andrebbe rifatto con il modello misto (`δ,η`) anche per quel
//! prodotto. Non e' stato fatto: il perimetro resta `SOGLIA_SICURA=2^-969`,
//! non un'estensione a soglie piu' basse.
//!
//! # Condizioni di applicabilita' (esplicite, non euristiche)
//!
//! Il percorso veloce e' applicabile — cioe' puo' restituire un segno,
//! anziche' `None` (fallback) — se e solo se `d1,d2,d3,d4` sono ciascuno
//! `0.0` oppure normale con `|valore| ≥ SOGLIA_SICURA`, **e** `detleft`,
//! `detright` soddisfano la stessa condizione con una precisazione: uno
//! zero e' sicuro solo se **genuino** (un fattore esattamente zero), mai se
//! e' un sottoflusso di un prodotto in realta' non nullo — la qualifica ha
//! trovato un controesempio reale (`1e-200 * 1e-200` sottoflusce a `0.0`)
//! prima che questa distinzione fosse introdotta: vedi
//! [`prodotto_sicuro`]. Per le sottrazioni non serve questa cautela: uno
//! zero da sottrazione e' sempre genuino (`fl(a-b)==0.0 ⟹ a==b` esatto).
//! Non e' una scelta di comodo sui casi "facili": e' la condizione
//! dimostrata sopra sotto cui il modello d'errore IEEE standard si applica
//! senza eccezioni. Input non finiti sono gia' esclusi a monte da
//! `exact_orientation::decode` (che ritorna `None` su NaN/infinito) — qui
//! non vengono ricontrollati.

use super::exact_orientation;

/// 2^-53: arrotondamento unitario per `f64` (52 bit di mantissa memorizzati
/// + 1 implicito = 53 bit di precisione).
const U: f64 = 1.110_223_024_625_156_5e-16;

/// 8u: margine dimostrato di quasi 2x sopra il termine 4.4u provato nella
/// derivazione del commento di modulo (non un'affermazione a occhio).
const ERRBOUND_REL: f64 = 8.0 * U;

/// Limite assoluto sull'errore di arrotondamento quando un risultato
/// sottoflusce, derivato nel commento di modulo come `2^-1075` (meta' del
/// passo fra `0` e il piu' piccolo subnormale). `2^-1075` non e' pero'
/// rappresentabile in `f64` (sottoflusce a `0.0`): si usa qui il piu'
/// piccolo subnormale positivo rappresentabile, `2^-1074`, un limite ancora
/// valido e leggermente piu' largo (`2^-1074 > 2^-1075`). Sommato
/// esplicitamente, non nascosto dentro `ERRBOUND_REL`.
const E0: f64 = 5.0e-324;

/// 2^-969 = 2^-1022 * 2^53: soglia di sicurezza per i valori intermedi,
/// derivata sopra (rende trascurabile l'errore assoluto di arrotondamento
/// dell'ultima sottrazione rispetto a `ERRBOUND*detsum` anche al caso
/// limite).
const SOGLIA_SICURA: f64 = 2.004_168_360_008_973e-292;

/// Vero se `x` e' `0.0` oppure normale con margine di sicurezza — la
/// condizione sotto cui il modello d'errore relativo IEEE standard si
/// applica senza le cautele del sottoflusso graduale (vedi derivazione nel
/// commento di modulo).
///
/// Valida per **sottrazioni**: la sottrazione di due float finiti non puo'
/// mai produrre uno zero spurio — `fl(a-b) == 0.0` implica `a == b` esatto
/// (fatto classico dell'aritmetica IEEE 754, la stessa proprieta' su cui
/// poggiano i rami "senza cancellazione" sotto). Uno zero da sottrazione e'
/// quindi sempre genuino.
#[inline]
fn sicuro(x: f64) -> bool {
    x == 0.0 || (x.is_normal() && x.abs() >= SOGLIA_SICURA)
}

/// Come [`sicuro`], ma per un **prodotto**: a differenza della sottrazione,
/// un prodotto di due fattori non nulli PUO' sottofluire a `0.0` (trovato
/// durante la qualifica: `1e-200 * 1e-200` sottoflusce a `0.0`, ben sotto
/// il piu' piccolo subnormale — non e' uno zero genuino, e' un valore non
/// nullo arrotondato via). Uno zero e' quindi sicuro solo se **genuino**:
/// un fattore esattamente zero (moltiplicare per zero esatto non introduce
/// mai arrotondamento). Se nessuno dei due fattori e' zero ma il prodotto
/// lo e', il prodotto ha sottoflusso: non sicuro, va in ricaduta.
#[inline]
fn prodotto_sicuro(fattore1: f64, fattore2: f64, prodotto: f64) -> bool {
    if prodotto == 0.0 {
        fattore1 == 0.0 || fattore2 == 0.0
    } else {
        prodotto.is_normal() && prodotto.abs() >= SOGLIA_SICURA
    }
}

/// Segno del determinante ordinario nei sette casi senza cancellazione
/// (segno discorde o uno dei due termini esattamente zero): il segno e'
/// provato corretto senza il limite d'errore. `None` per i due casi
/// concorde-positivo/concorde-negativo, dove la cancellazione e' possibile
/// e serve il limite d'errore.
#[inline]
fn segno_senza_cancellazione(detleft: f64, detright: f64) -> Option<i8> {
    if detleft > 0.0 {
        if detright <= 0.0 {
            return Some(1); // det = detleft - detright >= detleft > 0
        }
    } else if detleft < 0.0 {
        if detright >= 0.0 {
            return Some(-1); // det = detleft - detright <= detleft < 0
        }
    } else {
        // detleft == 0.0
        return if detright > 0.0 {
            Some(-1)
        } else if detright < 0.0 {
            Some(1)
        } else {
            Some(0)
        };
    }
    None
}

/// Esito del filtro: quale ramo e' stato preso, per la qualificazione (non
/// usato dal percorso di produzione, solo dalle copie diagnostiche — vedi
/// `orient2d_filtered_con_contatore`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ramo {
    VeloceSenzaCancellazione,
    VeloceConLimiteDErrore,
    RicadutaIntermedioNonSicuro,
    RicadutaCancellazioneNonCertificata,
}

/// Segno di `orient2d(p,q,r)` con `p,q,r` dati come sei bit pattern f64
/// (stesso ordine e stessa convenzione di
/// `exact_orientation::orient2d_sign_bits`: ax,ay,bx,by,cx,cy con a=p,
/// b=q, c=r). Ritorna `None` solo per input non finiti (propagato da
/// `exact_orientation::decode`), mai per incertezza del filtro: quando il
/// filtro non certifica, ricade sul kernel esatto invece di propagare
/// `None`.
pub fn orient2d_sign_bits_filtered(bits: [u64; 6]) -> Option<i8> {
    orient2d_sign_bits_filtered_con_ramo(bits).0
}

/// Come [`orient2d_sign_bits_filtered`], ma espone anche quale ramo e'
/// stato preso — usato dalle copie diagnostiche per contare chiamate e
/// ricadute senza toccare il percorso di produzione.
pub fn orient2d_sign_bits_filtered_con_ramo(bits: [u64; 6]) -> (Option<i8>, Ramo) {
    let floats = bits.map(f64::from_bits);
    if floats.iter().any(|v| !v.is_finite()) {
        // Coerente con exact_orientation::decode: non finito -> None,
        // senza tentare il filtro.
        return (None, Ramo::RicadutaIntermedioNonSicuro);
    }
    let [px, py, qx, qy, rx, ry] = floats;

    let d1 = qx - px;
    let d2 = ry - py;
    let d3 = qy - py;
    let d4 = rx - px;
    if !(sicuro(d1) && sicuro(d2) && sicuro(d3) && sicuro(d4)) {
        return (
            exact_orientation::orient2d_sign_bits(bits),
            Ramo::RicadutaIntermedioNonSicuro,
        );
    }

    let detleft = d1 * d2;
    let detright = d3 * d4;
    if !(prodotto_sicuro(d1, d2, detleft) && prodotto_sicuro(d3, d4, detright)) {
        return (
            exact_orientation::orient2d_sign_bits(bits),
            Ramo::RicadutaIntermedioNonSicuro,
        );
    }

    if let Some(segno) = segno_senza_cancellazione(detleft, detright) {
        return (Some(segno), Ramo::VeloceSenzaCancellazione);
    }

    // Concorde-positivo o concorde-negativo: cancellazione possibile.
    let det = detleft - detright;
    let detsum = detleft.abs() + detright.abs();
    let errbound = ERRBOUND_REL * detsum + E0;
    // Confronto stretto (`>`, non `>=`): scelta conservativa, non perche'
    // sia dimostrato un caso concreto in cui `>=` dia un segno sbagliato --
    // vedi "Uguaglianza alla soglia" sopra.
    if det > errbound {
        (Some(1), Ramo::VeloceConLimiteDErrore)
    } else if -det > errbound {
        (Some(-1), Ramo::VeloceConLimiteDErrore)
    } else {
        (
            exact_orientation::orient2d_sign_bits(bits),
            Ramo::RicadutaCancellazioneNonCertificata,
        )
    }
}
