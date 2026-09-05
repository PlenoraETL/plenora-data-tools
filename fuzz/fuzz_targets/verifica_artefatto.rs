#![no_main]

use libfuzzer_sys::fuzz_target;
use plenora_engine::interni::verifica_artefatto_ostile;

// Byte arbitrari come artefatto Arrow IPC **file format**: il verificatore
// deve concludere senza panicare. Accettare e rifiutare sono entrambi esiti
// legittimi; panicare no, e un guasto dell'harness nemmeno.
//
// CHE COSA QUESTO TARGET COPRE DAVVERO.
//
// Da byte arbitrari il fuzzer raggiunge con continuita' i passi 3, 4 e 5:
// presenza, sigillo, framing del footer e tetto cumulativo sui dizionari. Sono
// i passi che si possono far fallire senza costruire un Arrow IPC ben formato,
// ed e' li' che vive il confine ostile.
//
// Del passo 5-bis esercita il **percorso di lettura e hash** — la passata a
// blocchi sull'intero file — e non i suoi rifiuti: algoritmo e digest sono
// costruiti correttamente dall'harness, quindi «algoritmo non ammesso» e
// «digest non canonico» restano nella suite, dove sono casi negativi scritti
// apposta.
//
// I passi 6, 7, 8 e 8-bis — schema, contratto, conteggi, token — richiedono un
// file ben formato con lo schema, i conteggi e il token che l'harness fissa. Il
// fuzzer ci arriva solo quando il corpus contiene gia' un artefatto del genere,
// e il corpus **non e' versionato** (`/fuzz/corpus` e' ignorato): una campagna
// da zero non garantisce di produrlo. Che l'harness sappia arrivarci e'
// dimostrato dalla suite, non da qui.
//
// Il target esistente `arrow_ipc_decode` non sostituisce questo: esercita
// `decode_ipc`, cioe' lo stream format in memoria, e non tocca footer, tetto
// sui dizionari, digest ne' commit token.
fuzz_target!(|data: &[u8]| {
    if let Err(rottura) = verifica_artefatto_ostile(data) {
        panic!("guasto dell'harness del verificatore: {rottura}");
    }
});
