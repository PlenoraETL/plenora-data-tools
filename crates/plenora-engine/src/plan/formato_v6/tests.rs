//! Regressioni del formato v6.
//!
//! In un file proprio e non in un modulo `#[cfg(test)]` in linea: il gate
//! `verifica_nome_budget_memoria` e' esplicito nel non esentare i moduli in
//! linea, perche' «un nome che rientra da un modulo di test in linea rientra
//! comunque nel file sbagliato». Qui c'e' bisogno di nominare
//! `max_memory_bytes` — il nome della v4 — per verificare che la v6 lo
//! rifiuti, e questo e' il posto dove farlo senza aggirare la regola.

use super::{PlanV6, PLAN_SCHEMA_VERSION_V6};
use crate::plan::{PlanV5, PLAN_SCHEMA_VERSION_V5};
use plenora_core::limits::PlanLimits;
use serde_json::json;

/// Lo stesso piano nelle due versioni, con i limiti che si vogliono.
fn piano(versione: u16, limiti: &serde_json::Value) -> String {
    json!({
        "schema_version": versione,
        "limits": limiti,
        "inputs": ["main"],
        "nodes": [{"id": "a", "op": "table.filter", "in": ["main"], "config": {}}],
        "output": "a",
    })
    .to_string()
}

#[test]
fn un_piano_v6_e_accettato() {
    // Il caso positivo va esercitato: una validazione strutturale che
    // pretendesse `schema_version == 5` non accetterebbe nessun v6, e senza
    // un test che ne costruisca uno la suite resterebbe verde lo stesso.
    // Verde senza copertura non e' una verifica.
    let validato = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": 1_073_741_824}),
    ))
    .expect("un piano v6 valido deve essere accettato");
    assert_eq!(validato.max_domain_memory_bytes(), Some(1_073_741_824));
}

#[test]
fn il_parser_della_v6_pretende_la_v6() {
    // Il controllo di versione dentro `PlanV6::parse` non ha altra
    // copertura: tutti gli altri test gli passano documenti che dichiarano 6,
    // e chi entra dal dispatch e' gia' stato smistato. Toglierlo resterebbe
    // invisibile senza questo caso.
    //
    // E' l'invariante che regge se qualcuno chiama il parser della v6 senza
    // passare dal dispatch — un chiamante della libreria, un fuzzer, un test.
    // Senza, un documento v5 verrebbe accettato dal parser sbagliato e
    // canonicalizzato nel dominio sbagliato: stesso piano, identita' di un
    // altro formato.
    for versione in [
        PLAN_SCHEMA_VERSION_V5,
        crate::plan::PLAN_SCHEMA_VERSION_V4,
        7,
    ] {
        let errore = PlanV6::parse_default(&piano(versione, &json!({})))
            .expect_err("il parser della v6 accetta solo la v6");
        assert!(
            errore.to_string().contains("non e' un piano v6"),
            "versione {versione}: {errore}"
        );
    }
    // E la stessa forma, dichiarata 6, passa: il rifiuto e' della versione,
    // non della struttura.
    assert!(
        PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).is_ok(),
        "la struttura e' valida: a essere rifiutata deve essere solo la versione"
    );
}

#[test]
fn il_tetto_e_facoltativo_e_la_sua_assenza_non_e_un_default() {
    let senza = PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({})))
        .expect("il campo e' facoltativo (PLAN-009)");
    assert_eq!(
        senza.max_domain_memory_bytes(),
        None,
        "assente significa «profilo isolato non selezionabile», mai un tetto implicito"
    );
}

#[test]
fn il_campo_non_esiste_nella_v5() {
    // `PLAN-007`, e per costruzione: `LimitsOverride` ha
    // `deny_unknown_fields` e non conosce quel nome.
    let errore = PlanV5::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V5,
        &json!({"max_domain_memory_bytes": 1_073_741_824}),
    ))
    .expect_err("un v5 che dichiara il campo va rifiutato");
    assert!(
        errore.to_string().contains("max_domain_memory_bytes"),
        "l'errore deve nominare il campo estraneo: {errore}"
    );
}

#[test]
fn lo_zero_non_e_nessun_tetto() {
    // `PLAN-008`. Zero descrive un dominio che non puo' allocare nulla;
    // «nessun tetto» si dichiara omettendo il campo, e i due non devono
    // collassare.
    let errore = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": 0}),
    ))
    .expect_err("zero va rifiutato");
    assert!(errore.to_string().contains("zero"), "{errore}");
}

#[test]
fn il_tetto_sotto_il_budget_governato_effettivo_e_rifiutato() {
    // `PLAN-011` col budget DICHIARATO.
    let errore = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_governed_memory_bytes": 268_435_456,
                "max_domain_memory_bytes": 134_217_728}),
    ))
    .expect_err("un dominio sotto il budget dichiarato va rifiutato");
    assert!(
        errore.to_string().contains("budget governato effettivo"),
        "{errore}"
    );
}

#[test]
fn il_confronto_usa_il_default_quando_il_piano_omette_il_budget() {
    // `PLAN-012`: e' il caso che il contratto nomina esplicitamente, ed
    // e' quello che un confronto con l'override DICHIARATO lascerebbe
    // fuori — li' il budget e' `None` e non ci sarebbe niente da
    // confrontare.
    let default = plenora_core::DEFAULT_MAX_GOVERNED_MEMORY_BYTES;
    let sotto = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": default - 1}),
    ));
    assert!(
        sotto.is_err(),
        "sotto il default, col budget omesso, va rifiutato"
    );
    let pari = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": default}),
    ));
    assert!(pari.is_ok(), "pari al default e' accettato: {pari:?}");
}

#[test]
fn il_canonico_v6_dichiara_la_propria_versione_e_il_tetto() {
    // `PLAN-017`: il campo entra nel canonico SOLO quando e' presente.
    let con = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": 1_073_741_824}),
    ))
    .expect("valido");
    let canonico = con.canonical_json();
    assert_eq!(canonico["schema_version"], json!(PLAN_SCHEMA_VERSION_V6));
    assert_eq!(
        canonico["limits"]["max_domain_memory_bytes"],
        json!(1_073_741_824)
    );

    let senza = PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).expect("valido");
    assert!(
        senza.canonical_json()["limits"]
            .get("max_domain_memory_bytes")
            .is_none(),
        "assente non deve comparire"
    );
    assert_ne!(
        canonico,
        senza.canonical_json(),
        "un v6 che lo dichiara e uno che lo omette sono piani diversi"
    );
}

#[test]
fn i_nomi_delle_altre_versioni_non_funzionano_nella_v6() {
    // Il rifiuto e' **simmetrico**, e per costruzione: `LimitsOverrideV6`
    // ha `deny_unknown_fields` e non conosce `max_memory_bytes`, che e'
    // il nome della v4. Un alias lo avrebbe fatto funzionare ovunque; una
    // traduzione lo fa funzionare in un formato solo — il suo.
    let errore = PlanV6::parse_default(&piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_memory_bytes": 4_194_304}),
    ))
    .expect_err("il nome della v4 non esiste nella v6")
    .to_string();
    assert!(errore.contains("max_memory_bytes"), "{errore}");
}

#[test]
fn il_tetto_del_dominio_non_esiste_nella_v4() {
    // L'altro verso del confine: `LimitsOverrideV4` non conosce il nome
    // della v6. La v4 e' letta solo dalla migrazione, quindi il rifiuto
    // arriva da li'.
    let errore = crate::plan::valida_per_versione(
        &piano(
            crate::plan::PLAN_SCHEMA_VERSION_V4,
            &json!({"max_domain_memory_bytes": 1_073_741_824_u64}),
        ),
        &PlanLimits::default(),
    )
    .expect_err("il nome della v6 non esiste nella v4")
    .to_string();
    assert!(
        errore.contains("max_domain_memory_bytes"),
        "l'errore deve nominare il campo estraneo: {errore}"
    );
}

#[test]
fn un_tetto_oltre_u64_e_rifiutato() {
    // `PLAN-008`: rappresentabile in `u64`. Oltre quel confine il
    // documento non e' deserializzabile, e la categoria e' `data_mapping`
    // — un JSON che non entra nel tipo — non `invalid_plan`. Cio' che
    // conta e' che non venga saturato a `u64::MAX`: un tetto silenziosamente
    // ridotto a un altro numero e' peggio di un rifiuto.
    let oltre = format!("{}0", u64::MAX);
    let testo = piano(PLAN_SCHEMA_VERSION_V6, &json!({})).replace(
        r#""limits":{}"#,
        &format!(r#""limits":{{"max_domain_memory_bytes":{oltre}}}"#),
    );
    assert!(
        testo.contains(&oltre),
        "la sostituzione deve aver inserito il valore: {testo}"
    );
    let errore = PlanV6::parse_default(&testo).expect_err("un tetto oltre u64 va rifiutato");
    // `is_err()` da solo non basta: sarebbe soddisfatto da un rifiuto
    // avvenuto altrove — il tetto sui byte, una chiave duplicata — e il
    // commento qui sopra afferma una categoria precisa. Verificarla e' cio'
    // che lega il test a cio' che dice.
    assert_eq!(
        errore.category(),
        plenora_core::ErrorCategory::DataMapping,
        "un numero che non entra nel tipo e' un difetto di mapping, non un piano invalido: {errore}"
    );
    // La fase e' `Write`, non `Validate`, e non e' una svista: e'
    // l'approssimazione dichiarata per un `DataMapping` NON taggato — il
    // lato con possibile effetto, scelto conservativamente quando la variante
    // non distingue il momento. Il parse del piano non tagga la fase ai
    // confini, quindi qui la derivazione resta quella.
    //
    // Il test la fissa per non lasciarla cambiare in silenzio; raffinarla
    // richiederebbe un tagging al confine del parser, che non c'e'.
    assert_eq!(errore.phase(), plenora_core::ErrorPhase::Write);
    // Che il valore non sia stato saturato lo dimostra gia' l'`expect_err`:
    // saturare significherebbe ACCETTARE il documento con un tetto diverso da
    // quello scritto. Cercare `u64::MAX` nel messaggio sarebbe per giunta
    // fragile — il valore originale lo contiene come prefisso, quindi un
    // errore che lo riportasse legittimamente farebbe cadere il test.
}

#[test]
fn il_canonico_v6_riattraversa_il_dispatch() {
    // E' l'invariante che il target fuzz asserisce sul dispatch DAG. Li' e'
    // esercitata da payload casuali; qui e' fissata in modo deterministico,
    // cosi' se si rompesse non servirebbe una campagna per accorgersene.
    //
    // Ripassare dal DISPATCH e non dal parser della v6 e' il punto: e' cosi'
    // che si scopre un canonico che dichiara una versione che il dispatch non
    // riconosce piu', o che ne sceglie un'altra.
    let testo = piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": 1_073_741_824_u64}),
    );
    let validato =
        crate::plan::valida_per_versione(&testo, &PlanLimits::default()).expect("un v6 valido");
    let canonico = validato.canonical_json().to_string();

    let riletto = crate::plan::valida_per_versione(&canonico, &PlanLimits::default())
        .expect("la forma canonica deve ri-attraversare il dispatch");
    assert_eq!(
        riletto.schema_version(),
        PLAN_SCHEMA_VERSION_V6,
        "il canonico deve dichiarare la versione da cui proviene"
    );
    assert_eq!(
        riletto.canonical_json().to_string(),
        canonico,
        "canonicalizzazione non idempotente attraverso il dispatch"
    );
    assert_eq!(
        riletto.max_domain_memory_bytes(),
        Some(1_073_741_824),
        "il tetto del dominio non deve perdersi nel giro canonico"
    );
}

#[test]
fn il_documento_v6_fa_il_giro_completo() {
    // Il tipo e' pubblico e parallelo alla v5: si legge e si scrive. Se
    // il giro perdesse un campo, chi costruisce un v6 a mano e lo emette
    // otterrebbe un piano diverso da quello che ha scritto — e, passando
    // dal parser, un `plan_hash` diverso.
    let testo = piano(
        PLAN_SCHEMA_VERSION_V6,
        &json!({"max_domain_memory_bytes": 1_073_741_824,
                "max_governed_memory_bytes": 536_870_912}),
    );
    let letto: PlanV6 = serde_json::from_str(&testo).expect("v6 leggibile");
    let riscritto = serde_json::to_string(&letto).expect("v6 scrivibile");
    let riletto: PlanV6 = serde_json::from_str(&riscritto).expect("v6 rileggibile");
    assert_eq!(
        serde_json::to_value(&letto).expect("valore"),
        serde_json::to_value(&riletto).expect("valore"),
        "il giro non deve perdere nulla"
    );
    assert_eq!(
        riletto.limits.max_domain_memory_bytes,
        Some(1_073_741_824),
        "il campo della v6 sopravvive al giro"
    );
    // Un limite assente non compare, come nella v5: altrimenti il
    // documento riscritto dichiarerebbe `null` dove l'originale taceva.
    let scarno: PlanV6 =
        serde_json::from_str(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).expect("v6");
    let emesso = serde_json::to_value(&scarno).expect("valore");
    assert!(
        emesso["limits"].get("max_domain_memory_bytes").is_none(),
        "un limite assente non deve comparire: {emesso}"
    );
}

#[test]
fn lo_stesso_piano_in_v5_e_in_v6_ha_identita_diverse() {
    // `PLAN-018`. Le due strutture sono identiche — stessi nodi, stessi
    // archi, stessi limiti — e i due documenti no: il canonico dichiara
    // la propria versione, quindi i byte che si hashano differiscono, e
    // il separatore di dominio li tiene distinti anche se non
    // differissero.
    //
    // Da NON generalizzare: la v4 condivide l'identita' con la v5
    // (`PLAN-019`), e le regressioni della migrazione lo verificano.
    let come_v5 =
        PlanV5::parse_default(&piano(PLAN_SCHEMA_VERSION_V5, &json!({}))).expect("un v5 valido");
    let come_v6 =
        PlanV6::parse_default(&piano(PLAN_SCHEMA_VERSION_V6, &json!({}))).expect("un v6 valido");
    let canonico_v5 = come_v5.canonical_json();
    let canonico_v6 = come_v6.canonical_json();
    assert_eq!(canonico_v5["schema_version"], json!(PLAN_SCHEMA_VERSION_V5));
    assert_eq!(canonico_v6["schema_version"], json!(PLAN_SCHEMA_VERSION_V6));
    assert_ne!(
        canonico_v5, canonico_v6,
        "un v5 e un v6 per il resto identici sono piani diversi"
    );
    // E la sola differenza e' la versione: la struttura non e' cambiata.
    let mut senza_versione_v6 = canonico_v6;
    senza_versione_v6["schema_version"] = json!(PLAN_SCHEMA_VERSION_V5);
    assert_eq!(
        canonico_v5, senza_versione_v6,
        "la struttura condivisa deve restare identica"
    );
}
